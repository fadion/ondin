//! Shared chrome widgets (§9.2).
//!
//! The small vocabulary every panel is built from — cards, eyebrows, icon
//! buttons, fields, segmented controls, colour ramps. Nothing here knows about
//! the document.

#![allow(
    clippy::disallowed_methods,
    reason = "the primitives that wrap egui's raw controls live here"
)]

use crate::theme::{self, color, icon};
use eframe::egui;

/// A rotated coordinate frame on the screen: an origin and an angle.
///
/// Chrome that lines up with a rotated layer (the size badge and its lock) is laid
/// out in the layer's own frame and mapped to the screen here, because egui only
/// rotates `TextShape`.
#[derive(Clone, Copy)]
pub struct Frame2 {
    pub origin: egui::Pos2,
    /// Clockwise from the screen's +x, in radians (screen y points down).
    pub angle: f32,
    x: egui::Vec2,
    y: egui::Vec2,
}

impl Frame2 {
    pub fn rotated(angle: f32, origin: egui::Pos2) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self {
            origin,
            angle,
            x: egui::vec2(cos, sin),
            y: egui::vec2(-sin, cos),
        }
    }

    /// A point of this frame, in screen coordinates.
    pub fn to_screen(self, local: egui::Pos2) -> egui::Pos2 {
        self.origin + self.x * local.x + self.y * local.y
    }

    /// A screen point, in this frame's coordinates.
    ///
    /// Measurement only, from the badge tests, which check the frame is a whole
    /// number of quarter turns off the shape's.
    #[cfg(test)]
    pub fn to_local(self, screen: egui::Pos2) -> egui::Pos2 {
        let d = screen - self.origin;
        egui::pos2(d.dot(self.x), d.dot(self.y))
    }

    /// The oriented bounding box of `points`, in frame coordinates. Exact for a
    /// turned rectangle, loose for a sheared outline; test-only.
    #[cfg(test)]
    pub fn bounds_of(self, points: &[egui::Pos2]) -> egui::Rect {
        egui::Rect::from_points(&points.iter().map(|p| self.to_local(*p)).collect::<Vec<_>>())
    }

    /// A rounded rectangle of `size`, centred on the frame's origin, as screen
    /// points ready for `Shape::convex_polygon`.
    ///
    /// A polygon rather than a `RectShape`, which is axis-aligned.
    pub fn rounded_rect(self, size: egui::Vec2, radius: f32) -> Vec<egui::Pos2> {
        const STEPS: usize = 4;
        let (hw, hh) = (size.x * 0.5, size.y * 0.5);
        let r = radius.min(hw).min(hh).max(0.0);
        // Corner centres, clockwise from the top-left, with each arc's start angle.
        let corners = [
            (-hw + r, -hh + r, std::f32::consts::PI),
            (hw - r, -hh + r, -std::f32::consts::FRAC_PI_2),
            (hw - r, hh - r, 0.0),
            (-hw + r, hh - r, std::f32::consts::FRAC_PI_2),
        ];
        let mut out = Vec::with_capacity(corners.len() * (STEPS + 1));
        for (cx, cy, start) in corners {
            for i in 0..=STEPS {
                let t = start + std::f32::consts::FRAC_PI_2 * i as f32 / STEPS as f32;
                out.push(self.to_screen(egui::pos2(cx + r * t.cos(), cy + r * t.sin())));
            }
        }
        out
    }
}

/// A `Frame` for a chrome panel: a flat `fill` ground with symmetric padding.
pub fn panel_frame(fill: egui::Color32, mx: i8, my: i8) -> egui::Frame {
    egui::Frame::NONE
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(mx, my))
}

/// The floating ground a dropdown menu sits on: the card's fill and hairline
/// with a deeper shadow. `pad` is its inner margin.
pub fn menu_frame(pad: f32) -> egui::Frame {
    egui::Frame::NONE
        .fill(color::card())
        .stroke(egui::Stroke::new(MENU_BORDER, theme::color::text_a(15)))
        .shadow(MENU_SHADOW)
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::same(pad as i8))
}

/// The air inside a menu's card: [`menu_frame`]'s `pad` for a dropdown, a
/// context menu or a bar's menu, and egui's own popup menus' (`theme`'s
/// `menu_margin`), so a [`flyout`] beside any of them matches.
pub const MENU_PAD: f32 = 5.0;

/// The air inside a popover's card (the stroke, effect, type and grid popovers,
/// the colour picker): wider than a menu's, since it holds fields.
pub const POPOVER_PAD: f32 = 11.0;

/// How far a floating card keeps from the screen's edge when it is placed.
pub const SCREEN_MARGIN: f32 = 12.0;

/// The ground of a card floating over everything (a popup over the canvas, a
/// modal, Find, the variables browser and the insert picker): the card's fill,
/// a `color::divider()` hairline, a 10pt corner and a shadow deeper than a
/// menu's. No margin; the caller sets its own.
pub fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(color::card())
        // The token, not its value, so retuning `color::divider()` moves the
        // card's hairline too; no gate could catch a respelled copy.
        .stroke(egui::Stroke::new(1.0, color::divider()))
        .corner_radius(egui::CornerRadius::same(10))
        .shadow(egui::epaint::Shadow {
            offset: [0, 24],
            blur: 60,
            spread: 0,
            color: egui::Color32::from_black_alpha(128),
        })
}

/// A hairline across a floating card or a pane, between its sections: one
/// point of `color::divider()` the available width, on the device grid.
/// Answers its rect, for a divider that also drags.
pub fn card_rule(ui: &mut egui::Ui) -> egui::Rect {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    let ppp = ui.ctx().pixels_per_point();
    let y = hairline_in_column(rect.center().y, ppp, 1.0);
    ui.painter().line_segment(
        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(1.0, color::divider()),
    );
    rect
}

/// The width of a [`close_button`]'s box.
pub const CLOSE_BUTTON_W: f32 = 18.0;

/// The × that closes a popover, a dialog or a card over the canvas: one box,
/// one glyph size and the one tooltip, *Close*. Escape closes them all, so the
/// tooltip does not name it.
pub fn close_button(ui: &mut egui::Ui) -> egui::Response {
    icon_button(ui, icon::X, CLOSE_BUTTON_W, 13.0, false, true).on_hover_text("Close")
}

/// The head of a popover with no control to lead it (*Export settings*, *Repeat
/// around origin*): its name as an [`eyebrow`], the [`close_button`] against the
/// right margin. Answers whether the × was pressed.
pub fn popover_head(ui: &mut egui::Ui, title: &str) -> bool {
    ui.horizontal(|ui| {
        ui.label(eyebrow(title));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close_button(ui).clicked()
        })
        .inner
    })
    .inner
}

/// What a list says when it has nothing to show (a search that matched
/// nothing, an empty list): one line of [`SMALL_PT`] `text::dim()`, at the
/// rows' own inset, without a closing period.
pub fn empty_note(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(SMALL_PT)
                .color(theme::text::dim()),
        )
        .wrap(),
    )
}

/// What a whole surface says when it is empty, where [`empty_note`] is too
/// little: an optional glyph, a [`CONTROL_PT`] heading in `text::strong()` and a
/// [`SMALL_PT`] `text::dim()` line saying how to fill it, centred. `below` adds
/// what follows inside the same centred column, such as a button. The caller sets
/// the air above, which depends on the surface.
pub fn empty_state<R>(
    ui: &mut egui::Ui,
    glyph: Option<&str>,
    head: &str,
    sub: &str,
    below: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.vertical_centered(|ui| {
        if let Some(g) = glyph {
            ui.label(theme::icon_text(g, EMPTY_GLYPH_PT, theme::text::dim()));
        }
        ui.label(
            egui::RichText::new(head)
                .size(CONTROL_PT)
                .color(theme::text::strong()),
        );
        ui.add_space(-4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(sub)
                    .size(SMALL_PT)
                    .color(theme::text::dim()),
            )
            .wrap(),
        );
        below(ui)
    })
    .inner
}

/// An [`empty_state`]'s glyph: larger than a row's, since it stands alone.
pub const EMPTY_GLYPH_PT: f32 = 18.0;

/// An [`eyebrow`] painted rather than laid out, for a painter-drawn surface
/// (a canvas popup's section head, a list's group head): the same capitals,
/// size and ink. Answers the rect it inked.
pub fn paint_eyebrow(
    painter: &egui::Painter,
    at: egui::Pos2,
    align: egui::Align2,
    label: &str,
) -> egui::Rect {
    painter.text(
        at,
        align,
        label.to_uppercase(),
        egui::FontId::proportional(EYEBROW_PT),
        theme::text::faint(),
    )
}

/// The air around a [`canvas_pill`]'s word.
pub const PILL_PAD: egui::Vec2 = egui::vec2(7.0, 3.0);

/// The ink a [`canvas_pill`] on `hue` takes: each canvas mark's own.
pub fn pill_ink(hue: egui::Color32) -> egui::Color32 {
    if hue == color::COMPONENT {
        color::COMPONENT_INK
    } else if hue == color::MEASURE {
        color::MEASURE_INK
    } else {
        color::BADGE_INK
    }
}

/// The size a [`canvas_pill`] saying `text` takes, for a caller that places it
/// by its size.
pub fn canvas_pill_size(painter: &egui::Painter, text: &str) -> egui::Vec2 {
    let font = egui::FontId::proportional(BADGE_PT);
    let g = painter.layout_no_wrap(text.to_owned(), font, egui::Color32::PLACEHOLDER);
    g.size() + PILL_PAD * 2.0
}

/// A word on a coloured pill over the canvas (the size badge, a measure, a
/// spacing value, the pivot's name, the crop's *Done*, the drop label):
/// [`BADGE_PT`] in [`pill_ink`] on `hue`, [`PILL_PAD`] around it, 4pt corners,
/// placed at `at` by `align`. Answers its rect.
pub fn canvas_pill(
    painter: &egui::Painter,
    at: egui::Pos2,
    align: egui::Align2,
    text: &str,
    hue: egui::Color32,
) -> egui::Rect {
    canvas_pill_padded(painter, at, align, text, hue, PILL_PAD)
}

/// [`canvas_pill`] with its own air, for a pill that has to stay narrower
/// than the rest (a measure's label, which sits on the line it measures).
pub fn canvas_pill_padded(
    painter: &egui::Painter,
    at: egui::Pos2,
    align: egui::Align2,
    text: &str,
    hue: egui::Color32,
    pad: egui::Vec2,
) -> egui::Rect {
    let ink = pill_ink(hue);
    let g = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(BADGE_PT), ink);
    let rect = align.anchor_size(at, g.size() + pad * 2.0);
    painter.rect_filled(rect, egui::CornerRadius::same(4), hue);
    painter.galley(rect.min + pad, g, ink);
    rect
}

/// The shadow under every floating card: [`menu_frame`]'s, and egui's own popups
/// (a `ComboBox`'s list, a tooltip), which `theme::install` gives it so a
/// dropdown's list and a menu cast the same shadow.
pub const MENU_SHADOW: egui::epaint::Shadow = egui::epaint::Shadow {
    offset: [0, 10],
    blur: 24,
    spread: 0,
    color: egui::Color32::from_black_alpha(82),
};

/// The "heard you, working on it" pulse, as an opacity `t` seconds after the
/// click: down to 50% over [`PULSE_RAMP`], held for [`PULSE_HOLD`], back to
/// whole, held, and again. It starts on a ramp so a quick load barely flickers.
pub(crate) fn pulse_alpha(t: f32) -> f32 {
    let period = 2.0 * (PULSE_RAMP + PULSE_HOLD);
    let t = t.max(0.0) % period;
    let ease = |x: f32| x * x * (3.0 - 2.0 * x);
    let low = 0.5;
    if t < PULSE_RAMP {
        1.0 - (1.0 - low) * ease(t / PULSE_RAMP)
    } else if t < PULSE_RAMP + PULSE_HOLD {
        low
    } else if t < 2.0 * PULSE_RAMP + PULSE_HOLD {
        low + (1.0 - low) * ease((t - PULSE_RAMP - PULSE_HOLD) / PULSE_RAMP)
    } else {
        1.0
    }
}

/// [`pulse_alpha`]'s fade and its rest, in seconds.
pub(crate) const PULSE_RAMP: f32 = 0.3;
pub(crate) const PULSE_HOLD: f32 = 0.2;

/// One row of a dropdown menu: an icon, a label, and a hover ground across the
/// whole width. Returns the `Response` because a tooltip works in both the live
/// and disabled states only when hung on it: a wrapping `ui.scope` loses the hit
/// test and a `disable()`d `Ui` reports no hover.
pub fn menu_item(ui: &mut egui::Ui, glyph: &str, text: &str, height: f32) -> egui::Response {
    menu_row(ui, MenuRow::new(glyph, text), height)
}

/// One row of a menu, in the full shape a context menu needs
/// (`docs/context-menus.md` §8): [`menu_item`] plus an accelerator, a checked
/// state and a destructive tint. Dropdowns and context menus share it so their
/// sizes cannot drift.
#[derive(Clone, Copy)]
pub struct MenuRow<'a> {
    pub glyph: &'a str,
    pub label: &'a str,
    /// The chord this row answers to, right-aligned in [`theme::text::faint()`].
    pub accel: Option<&'a str>,
    /// `Some` makes this a checkable row. The glyph column carries the state (the
    /// glyph turns [`color::accent()`], the label full strength); a row with no
    /// glyph of its own shows [`icon::CHECK`] there when on. No tick column.
    pub checked: Option<bool>,
    /// A row that destroys something: [`theme::color::danger()`] on glyph and label
    /// over the ordinary neutral hover ground.
    pub danger: bool,
    /// A row that cannot run now: dimmed, no hover ground, not clickable. The
    /// caller owes it a tooltip saying why, hung off the returned `Response` (see
    /// `inspector::menu_action` for the spellings that do not work).
    pub enabled: bool,
    /// Who decides whether this row wears the ground. `None` means the pointer.
    /// `Some` is a keyboard highlight (`docs/context-menus.md` §8): the driven menu
    /// passes `Some(true)` on one row and `Some(false)` on the rest, so the pointer
    /// cannot paint a second ground.
    pub highlight: Option<bool>,
}

impl<'a> MenuRow<'a> {
    pub fn new(glyph: &'a str, label: &'a str) -> Self {
        Self {
            glyph,
            label,
            accel: None,
            checked: None,
            danger: false,
            enabled: true,
            highlight: None,
        }
    }
    pub fn accel(mut self, accel: Option<&'a str>) -> Self {
        self.accel = accel;
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = Some(on);
        self
    }
    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// Give the ground to the caller (see [`Self::highlight`]); pass it on every
    /// row of a menu once any row has it.
    pub fn highlight(mut self, on: bool) -> Self {
        self.highlight = Some(on);
        self
    }
}

pub fn menu_row(ui: &mut egui::Ui, row: MenuRow<'_>, height: f32) -> egui::Response {
    // A dimmed row senses nothing; its tooltip works because egui grants hover to a
    // non-interactive widget above the top interactive one (true in a menu `Area`).
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        if row.enabled {
            egui::Sense::click()
        } else {
            egui::Sense::empty()
        },
    );
    // `&& enabled` keeps a stale highlight off a row that went dim.
    let hovered = row.highlight.unwrap_or_else(|| resp.hovered()) && row.enabled;
    let on = row.checked == Some(true);
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, egui::CornerRadius::same(4), color::hover());
    }
    // Three tiers: disabled, resting, on or hovered.
    let (glyph_col, label_col, accel_col) = match () {
        _ if !row.enabled => (
            theme::text::disabled(),
            theme::text::disabled(),
            theme::text::disabled(),
        ),
        _ if row.danger => (
            color::danger(),
            color::danger(),
            color::danger().gamma_multiply(0.55),
        ),
        _ if on => (color::accent(), color::text(), theme::text::faint()),
        _ if hovered => (color::text(), color::text(), theme::text::faint()),
        _ => (
            theme::text::dim(),
            theme::text::muted(),
            theme::text::faint(),
        ),
    };
    // A checkable row handed `""` carries the tick in the glyph column, so every
    // row of its menu keeps one label column whether checked or not. Any other
    // row handed `""` sets its label at the row's own inset; no menu mixes the two.
    let glyph = match (row.glyph.is_empty(), on) {
        (true, true) => icon::CHECK,
        _ => row.glyph,
    };
    let bare = row.glyph.is_empty() && row.checked.is_none();
    let glyph_at = egui::pos2(rect.left() + 8.0 + 7.5, rect.center().y);
    if !glyph.is_empty() {
        p.text(
            glyph_at,
            egui::Align2::CENTER_CENTER,
            glyph,
            theme::icon_font(15.0),
            glyph_col,
        );
    }
    let label_x = if bare { 8.0 } else { MENU_LABEL_X };
    let accel_w = match row.accel {
        Some(accel) => {
            let r = p.text(
                egui::pos2(rect.right() - 8.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                crate::input::keys(accel),
                egui::FontId::proportional(CAPTION_PT),
                accel_col,
            );
            r.width() + MENU_ACCEL_GAP
        }
        None => 0.0,
    };
    // Cut with an ellipsis so labels built from the user's text (`Reset ‹layer›`)
    // do not run off the card; the full label is the tooltip.
    let mut job = egui::text::LayoutJob::single_section(
        row.label.to_owned(),
        egui::TextFormat::simple(egui::FontId::proportional(SMALL_PT), label_col),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.width() - label_x - 8.0 - accel_w).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = p.layout_job(job);
    let cut = galley.elided;
    p.galley(
        egui::pos2(
            rect.left() + label_x,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        label_col,
    );
    if cut {
        return resp.on_hover_text(row.label);
    }
    resp
}

/// Where a [`menu_row`]'s label starts after its glyph: the 8pt inset, the 15pt
/// glyph column, 9pt of air. A list row with a box in the glyph's place uses it too.
pub const MENU_LABEL_X: f32 = 8.0 + 15.0 + 9.0;

/// The air between a menu row's label and its accelerator; small so every fixed
/// label stays whole (`a_long_reset_label_is_cut_inside_the_row_and_fixed_labels_are_whole`).
const MENU_ACCEL_GAP: f32 = 2.0;

/// The total height one separator occupies — the rule plus its air, both sides.
///
/// [`menu_sep`] allocates it and `menu::height` sums it before the menu is shown;
/// `a_menus_computed_height_is_the_height_it_paints` pins the two together.
pub const MENU_SEP_H: f32 = SEP_ABOVE + 1.0 + SEP_BELOW;

/// [`menu_frame`]'s border, on every side.
///
/// A `Frame`'s stroke grows its outer rect, so a card paints `2 * MENU_BORDER`
/// wider and taller than content plus margins; `menu::height` counts it.
pub const MENU_BORDER: f32 = 1.0;

/// The width to give the `Ui` inside a [`menu_frame`] card that is to paint `card`
/// points wide, with `pad` of inner margin. Card widths are stated as the outer
/// card, which `menu_place`, [`OndinApp::popover_left`] and the right-aligning
/// anchors are all handed.
///
/// [`OndinApp::popover_left`]: crate::app::OndinApp::popover_left
pub const fn menu_inner_w(card: f32, pad: f32) -> f32 {
    card - pad * 2.0 - MENU_BORDER * 2.0
}

/// The air above a separator's rule.
pub const SEP_ABOVE: f32 = 5.0;
/// The air below it.
pub const SEP_BELOW: f32 = 4.0;
/// A separator's ink.
pub fn sep_ink() -> egui::Color32 {
    theme::color::text_a(26)
}

/// The hairline between two groups of any list: a menu (`docs/context-menus.md`
/// §8) or a dropdown's.
///
/// One point of [`sep_ink`] with [`SEP_ABOVE`] and [`SEP_BELOW`] around it, snapped
/// to whole device pixels and inside [`MENU_SEP_H`]
/// (`the_rule_still_fits_the_height_the_menu_reserved_for_it`). It allocates
/// exactly, so call it with `item_spacing.y` zero (a menu's rows are).
pub fn menu_sep(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), MENU_SEP_H),
        egui::Sense::empty(),
    );
    let ppp = ui.ctx().pixels_per_point();
    // A point, in whole device pixels, never thinner than one.
    let weight = (1.0 * ppp).round().max(1.0) / ppp;
    let y = ((rect.top() + SEP_ABOVE) * ppp).round() / ppp;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(rect.left(), y),
            egui::pos2(rect.right(), y + weight),
        ),
        0,
        sep_ink(),
    );
}

/// A sentence inside a menu card, as a confirmation says what goes: wrapped, in
/// the muted ink, inset as far as a row's glyph on both sides and with air above
/// and below, so it neither touches the card's edge nor crowds the rows.
pub fn menu_note(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(SMALL_PT)
                        .color(theme::text::muted()),
                )
                .wrap(),
            );
        });
}

/// The height a menu card of `rows` rows and `seps` separators paints at, rows
/// `gap` apart and padded `pad`, its border included. Exact, since rows and
/// separators are fixed heights; [`menu_place`] needs it before the card is shown.
pub fn menu_height_with(
    rows: usize,
    seps: usize,
    row_h: f32,
    gap: f32,
    sep_h: f32,
    pad: f32,
) -> f32 {
    let gaps = (rows + seps).saturating_sub(1);
    // The frame's stroke grows its outer rect, so the border counts.
    MENU_BORDER * 2.0 + pad * 2.0 + rows as f32 * row_h + seps as f32 * sep_h + gaps as f32 * gap
}

/// [`menu_height_with`] for a [`menu_frame`] of [`MENU_ROW_H`] rows 1pt apart,
/// padded [`MENU_PAD`].
pub fn menu_height(rows: usize, seps: usize) -> f32 {
    menu_height_with(rows, seps, MENU_ROW_H, 1.0, MENU_SEP_H, MENU_PAD)
}

/// The size the `Area` `id` painted at last frame, for a menu whose rows vary too
/// much to sum: `None` before it has painted once.
pub fn painted_size(ctx: &egui::Context, id: egui::Id) -> Option<egui::Vec2> {
    egui::AreaState::load(ctx, id).and_then(|s| s.size)
}

/// Where a menu of `size` opened at `at` actually goes, so that all of it stays
/// on screen (`docs/context-menus.md` §8). `egui::Area::fixed_pos` neither flips
/// nor clamps: flip, then round, then clamp, since rounding a clamped value can
/// push it off screen (`a_menu_is_clamped_after_it_is_rounded_and_not_before`).
pub fn menu_place(ctx: &egui::Context, at: egui::Pos2, size: egui::Vec2) -> egui::Pos2 {
    // `content_rect`, not `viewport_rect`: it excludes the strip an OS status bar
    // or notch may cover.
    let screen = ctx.content_rect();
    let mut pos = at;
    if pos.x + size.x > screen.right() {
        pos.x = at.x - size.x;
    }
    if pos.y + size.y > screen.bottom() {
        pos.y = at.y - size.y;
    }
    let ppp = ctx.pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    pos.x = snap(pos.x).clamp(screen.left(), (screen.right() - size.x).max(screen.left()));
    pos.y = snap(pos.y).clamp(screen.top(), (screen.bottom() - size.y).max(screen.top()));
    pos
}

/// Where a list `size` big opens under the field `under`, `gap` below it: left
/// edges together, or **right edges together** when that would run off the
/// screen, and above the field when below has no room — then clamped by
/// [`menu_place`] (which alone would flip it a list-width left of the field).
pub fn dropdown_place(
    ctx: &egui::Context,
    under: egui::Rect,
    size: egui::Vec2,
    gap: f32,
) -> egui::Pos2 {
    let screen = ctx.content_rect();
    let x = match under.left() + size.x > screen.right() {
        true => under.right() - size.x,
        false => under.left(),
    };
    let y = match under.bottom() + gap + size.y > screen.bottom() {
        true => under.top() - gap - size.y,
        false => under.bottom() + gap,
    };
    menu_place(ctx, egui::pos2(x, y), size)
}

/// Between the canvas's top edge and a [`canvas_popup_area`]'s card.
pub const CANVAS_POPUP_TOP: f32 = 56.0;

/// The most a [`canvas_popup_area`]'s scrolling list stands before it scrolls.
pub const CANVAS_POPUP_LIST_H: f32 = 372.0;

/// The place every popup over the canvas hangs from — the command palette, batch
/// rename, tokens import, Colour review: centred on the canvas's width (not the
/// window's), [`CANVAS_POPUP_TOP`] under its top edge. A modal passes it to
/// `Modal::area`; a card that leaves the canvas live shows it directly.
pub fn canvas_popup_area(ctx: &egui::Context, id: egui::Id, canvas: egui::Rect) -> egui::Area {
    let screen = ctx.content_rect();
    egui::Modal::default_area(id).anchor(
        egui::Align2::CENTER_TOP,
        egui::vec2(
            canvas.center().x - screen.center().x,
            canvas.top() - screen.top() + CANVAS_POPUP_TOP,
        ),
    )
}

/// The most height a [`canvas_popup_area`]'s card may take: down to
/// [`CANVAS_POPUP_TOP`] short of the canvas's bottom edge.
pub fn canvas_popup_room(canvas: egui::Rect) -> f32 {
    (canvas.height() - 2.0 * CANVAS_POPUP_TOP).max(160.0)
}

/// Where a submenu `size` big, padded `pad`, opens beside the menu row `row`: its
/// first row level with the row, to the right — or the left when the right has
/// no room — then clamped by [`menu_place`]. Short of room below, it shifts up
/// only as far as it must, so it stays beside its row rather than flipping above.
pub fn flyout_place(
    ctx: &egui::Context,
    row: egui::Rect,
    size: egui::Vec2,
    pad: f32,
) -> egui::Pos2 {
    let screen = ctx.content_rect();
    // The card's padding and border, so its first row lines up with `row`.
    let lift = pad + MENU_BORDER;
    let x = match row.right() + 4.0 + size.x > screen.right() {
        true => row.left() - 4.0 - size.x,
        false => row.right() + 4.0,
    };
    let y = (row.top() - lift).min(screen.bottom() - size.y);
    menu_place(ctx, egui::pos2(x, y), size)
}

/// The `▸` at the right of a menu row that opens a list beside it.
fn paint_submenu_caret(ui: &egui::Ui, row: egui::Rect) {
    ui.painter().text(
        egui::pos2(row.right() - 10.0, row.center().y),
        egui::Align2::CENTER_CENTER,
        icon::CARET_RIGHT,
        theme::icon_font(11.0),
        theme::text::dim(),
    );
}

/// A popup menu's row that opens a [`flyout`] beside it, `label ▸`: lit while
/// `open`, so it stays lit across the gap to its list and inside it. The caller
/// opens the list on the row's hover or click, and closes it when another row
/// is hovered. A `row` already given a keyboard highlight keeps it instead.
pub fn submenu_row(ui: &mut egui::Ui, row: MenuRow<'_>, open: bool) -> egui::Response {
    // The row about to be drawn: a highlighted row leaves its hover to the caller.
    let next = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), MENU_ROW_H),
    );
    let lit = row.enabled
        && row
            .highlight
            .unwrap_or_else(|| open || ui.rect_contains_pointer(next));
    let resp = menu_row(ui, row.highlight(lit), MENU_ROW_H);
    paint_submenu_caret(ui, resp.rect);
    resp
}

/// [`submenu_row`] in a menu of [`menu_check`] rows, so its label stays in their
/// column: never ticked, lit while `open`.
pub fn submenu_check_row(
    ui: &mut egui::Ui,
    label: &str,
    open: bool,
    height: f32,
) -> egui::Response {
    let resp = menu_check_held(ui, label, false, height, open);
    paint_submenu_caret(ui, resp.rect);
    resp
}

/// The air inside a [`flyout`]'s card by default: a menu's own, so a list beside
/// a menu's row matches it.
pub const FLYOUT_PAD: f32 = MENU_PAD;

/// The list a [`submenu_row`] at `row` opens beside it: a [`menu_frame`] card `w`
/// wide, placed by [`flyout_place`]. [`Flyout::pad`] and [`Flyout::order`] match it
/// to the menu it hangs from; [`Flyout::show`] draws it.
pub fn flyout(id: egui::Id, row: egui::Rect, w: f32) -> Flyout {
    Flyout {
        id,
        row,
        w,
        pad: FLYOUT_PAD,
        order: egui::Order::Tooltip,
    }
}

/// A [`flyout`] about to be shown.
#[must_use]
pub struct Flyout {
    id: egui::Id,
    row: egui::Rect,
    w: f32,
    pad: f32,
    order: egui::Order,
}

impl Flyout {
    /// The card's inner margin: the padding of the menu it hangs from, so their
    /// rows line up. [`FLYOUT_PAD`] by default.
    pub fn pad(mut self, pad: f32) -> Self {
        self.pad = pad;
        self
    }

    /// The layer the card shows on, above the menu it hangs from: `Tooltip` by
    /// default, over a popup; a menu in a `Foreground` area of its own may keep its
    /// list beside it there.
    pub fn order(mut self, order: egui::Order) -> Self {
        self.order = order;
        self
    }

    /// Show the card holding `rows` menu rows and `seps` separators, 1pt apart,
    /// that `add` draws. Answers the card's response, for the caller's
    /// click-outside and pointer-inside tests.
    pub fn show(
        self,
        ctx: &egui::Context,
        (rows, seps): (usize, usize),
        add: impl FnOnce(&mut egui::Ui),
    ) -> egui::Response {
        let items = (rows + seps) as f32;
        let h = 2.0 * (MENU_BORDER + self.pad)
            + rows as f32 * MENU_ROW_H
            + seps as f32 * MENU_SEP_H
            + (items - 1.0).max(0.0);
        let pos = flyout_place(ctx, self.row, egui::vec2(self.w, h), self.pad);
        egui::Area::new(self.id)
            .order(self.order)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                menu_frame(self.pad).show(ui, |ui| {
                    ui.set_width(menu_inner_w(self.w, self.pad));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    add(ui);
                });
            })
            .response
    }
}

/// Height of a top-bar dropdown's head, and so of its hit target.
pub(crate) const HEAD_H: f32 = 26.0;

/// Empty space a head keeps after its caret, `(HEAD_H - 16) / 2`, the inset an
/// [`icon_button`] gives its glyph, so the two space evenly in one cluster.
pub(crate) const HEAD_PAD: f32 = 5.0;

/// The space before a head's content: [`HEAD_PAD`] plus 2, since the caret's ink
/// is narrower than its glyph box and leaves the right side looking wider.
pub(crate) const HEAD_LEAD: f32 = HEAD_PAD + 2.0;

/// The head of a top-bar dropdown — a mark, `gap` points, then a caret — sensed as
/// one rect, lit like an [`icon_button`] and held lit while `open`. Hand-painted:
/// a `ui.label` wins the hit test over the rect that contains it.
pub fn menu_head(ui: &mut egui::Ui, lead: egui::RichText, gap: f32, open: bool) -> egui::Response {
    menu_head_min(ui, lead, 0.0, gap, open)
}

/// [`menu_head`] whose mark takes at least `min_lead` points, set against the caret,
/// so a readout that changes length moves nothing beside it.
pub fn menu_head_min(
    ui: &mut egui::Ui,
    lead: egui::RichText,
    min_lead: f32,
    gap: f32,
    open: bool,
) -> egui::Response {
    let lay = |ui: &egui::Ui, text: egui::RichText| {
        egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    };
    let lead = lay(ui, lead);
    let lead_w = lead.size().x.max(min_lead);
    let caret = lay(
        ui,
        theme::icon_text(icon::CARET_DOWN, 12.0, theme::text::dim()),
    );

    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(HEAD_LEAD + lead_w + gap + caret.size().x + HEAD_PAD, HEAD_H),
        egui::Sense::click(),
    );
    // Centred rather than baseline-aligned: different sizes and fonts.
    let p = ui.painter();
    let at = |g: &std::sync::Arc<egui::Galley>, x: f32| {
        egui::pos2(x, rect.center().y - g.size().y / 2.0)
    };
    // A disabled head still hovers, for its tooltip, but never lights.
    let lit = open || (resp.hovered() && ui.is_enabled());
    if lit {
        p.rect_filled(rect, egui::CornerRadius::same(5), theme::color::hover());
    }
    let ink_of = |g: &std::sync::Arc<egui::Galley>, pos: egui::Pos2| {
        if lit {
            p.galley_with_override_text_color(pos, g.clone(), color::text());
        } else {
            p.galley(pos, g.clone(), egui::Color32::PLACEHOLDER);
        }
    };
    let ink = rect.left() + HEAD_LEAD;
    ink_of(&lead, at(&lead, ink + lead_w - lead.size().x));
    ink_of(&caret, at(&caret, ink + lead_w + gap));
    resp
}

/// A dropdown row that carries a checkmark, as in the View and Snap menus: a
/// [`menu_row`] with no glyph, checkable, so its tick takes the glyph column and
/// labels stay in one line whether or not it shows.
pub fn menu_check(ui: &mut egui::Ui, label: &str, on: bool, height: f32) -> bool {
    menu_check_response(ui, label, on, height).clicked()
}

/// [`menu_check`], answering the row's whole response, for a caller that watches
/// its hover.
pub fn menu_check_response(
    ui: &mut egui::Ui,
    label: &str,
    on: bool,
    height: f32,
) -> egui::Response {
    menu_check_held(ui, label, on, height, false)
}

/// [`menu_check_response`] for [`submenu_check_row`]: `held` lights it whatever the
/// pointer does, so it stays lit across the gap to its open submenu and inside it.
fn menu_check_held(
    ui: &mut egui::Ui,
    label: &str,
    on: bool,
    height: f32,
    held: bool,
) -> egui::Response {
    let row = MenuRow::new("", label).checked(on);
    let row = if held { row.highlight(true) } else { row };
    menu_row(ui, row, height)
}

/// The painted width a dropdown of [`menu_check`] rows needs so its widest
/// label fits — `floor` or more, the row's inset on the right as on the left.
pub fn menu_check_width(ctx: &egui::Context, labels: &[&str], pad: f32, floor: f32) -> f32 {
    let rows: Vec<(&str, Option<&str>)> = labels.iter().map(|l| (*l, None)).collect();
    menu_check_accel_width(ctx, &rows, pad, floor)
}

/// [`menu_check_width`] for rows that may carry an accelerator, `(label, accel)`,
/// measured as [`menu_row`] lays the pair: the label, the gap, the spelled chord.
pub fn menu_check_accel_width(
    ctx: &egui::Context,
    rows: &[(&str, Option<&str>)],
    pad: f32,
    floor: f32,
) -> f32 {
    let measure = |text: String, pt: f32| {
        ctx.fonts_mut(|f| {
            f.layout_no_wrap(text, egui::FontId::proportional(pt), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    let widest = rows
        .iter()
        .map(|(label, accel)| {
            let chord = accel.map_or(0.0, |a| {
                measure(crate::input::keys(a).into_owned(), CAPTION_PT) + MENU_ACCEL_GAP
            });
            measure((*label).to_string(), SMALL_PT) + chord
        })
        .fold(0.0, f32::max);
    // `menu_row`'s label column, then its 8pt right inset.
    let row = MENU_LABEL_X + widest + 8.0;
    (row + 2.0 * pad + 2.0 * MENU_BORDER).ceil().max(floor)
}

/// A small-caps section eyebrow (`LAYERS`, `TRANSFORM`, …).
pub fn eyebrow(label: &str) -> egui::RichText {
    egui::RichText::new(label.to_uppercase())
        .size(EYEBROW_PT)
        .color(theme::text::faint())
}

/// An [`eyebrow`]'s size, named for the popover sections that draw a marked one
/// by hand (`typography::section_marked`).
pub const EYEBROW_PT: f32 = 9.5;

/// A [`fold_head`]'s row height: the icon buttons beside it.
pub const FOLD_HEAD_H: f32 = 20.0;

/// How far a [`fold_head`]'s caret sits above the capitals' centre line, in
/// points. An optical correction: a chevron's weight is in its arms, so one
/// centred by its ink reads low beside the capitals.
pub const FOLD_CARET_LIFT: f32 = 2.0;

/// A pane section's fold header (Pages, Layers): [`section_head`]'s caret and
/// eyebrow, but one press target over just the two, which both brighten while it
/// is hovered. Answers the press and the eyebrow's baseline, for text set beside it.
///
/// Placed by ink, not by layout box: the eyebrow's capitals sit on the row's
/// centre line, and the caret [`FOLD_CARET_LIFT`] above it.
pub fn fold_head(ui: &mut egui::Ui, label: &str, open: bool) -> (egui::Response, f32) {
    let caret = if open {
        icon::CARET_DOWN
    } else {
        icon::CARET_RIGHT
    };
    // `PLACEHOLDER`, so the painter's colour is the one drawn.
    let lay = |text: String, font| {
        ui.painter()
            .layout_no_wrap(text, font, egui::Color32::PLACEHOLDER)
    };
    let glyph = lay(caret.into(), theme::icon_font(12.0));
    let word = lay(label.to_uppercase(), egui::FontId::proportional(EYEBROW_PT));
    let word_x = glyph.size().x + 6.0;
    let size = egui::vec2(word_x + word.size().x, FOLD_HEAD_H);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let ink = if resp.hovered() {
        theme::text::muted()
    } else {
        theme::text::faint()
    };
    let ppp = ui.ctx().pixels_per_point();
    // The galley's top that puts its ink's centre on the row's, on a pixel.
    let top = |g: &egui::Galley| {
        let ink = g.rows.first().map_or(g.rect, |r| {
            r.row.visuals.mesh_bounds.translate(r.pos.to_vec2())
        });
        ((rect.center().y - ink.center().y) * ppp).round() / ppp
    };
    let word_top = top(&word);
    let baseline = word_top + baseline_in(&word);
    let p = ui.painter();
    p.galley(
        egui::pos2(rect.left(), top(&glyph) - FOLD_CARET_LIFT),
        glyph,
        ink,
    );
    p.galley(egui::pos2(rect.left() + word_x, word_top), word, ink);
    (resp, baseline)
}

/// How far below a galley's top its first row's baseline sits.
pub fn baseline_in(galley: &egui::Galley) -> f32 {
    galley
        .rows
        .first()
        .and_then(|r| r.row.glyphs.first().map(|g| r.pos.y + g.pos.y))
        .unwrap_or(galley.size().y)
}

/// An inspector card — a surface-filled rounded box with padding.
///
/// Cards float over the canvas with no backdrop pane, so the ground is opaque and
/// carries a hairline and a drop shadow to read against any artwork.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card_at(ui, add).0
}

/// The horizontal inner margin of a [`card`], and the width of the action icon a
/// [`section_head`] can carry.
pub const CARD_MARGIN_X: f32 = 14.0;
pub const HEAD_ACTION_W: f32 = 20.0;

/// Empty space around a header's action icon that presses to the icon rather
/// than to the header behind it.
///
/// A hit target, not a margin: only the sensed rect grows, and the header gives
/// up the same band ([`section_head`]) since overlapping targets would both press.
pub const HEAD_ACTION_PAD: f32 = 6.0;

/// The pitch between rows inside a card, which multi-row entries (image fill,
/// stroke pairs, sides rows) measure themselves against.
///
/// Read it, never copy it into a test fixture, or the fixture keeps agreeing with
/// itself while the row moves.
pub const CARD_ROW_GAP: f32 = 9.0;

/// How tall a control is, everywhere: panel fields, popover fields, the colour
/// picker's rows, the buttons beside them.
///
/// A [`segmented`] track paints [`SEGMENT_CELL_H`] plus two points at each end, so
/// hand it that number rather than this one.
pub const CONTROL_H: f32 = 28.0;

/// What to hand [`segmented`] so its track paints [`CONTROL_H`] tall.
///
/// The track adds two points of padding above and below; its hairline comes out
/// of the cells ([`SEGMENT_BORDER`]).
pub const SEGMENT_CELL_H: f32 = CONTROL_H - 4.0;

// --- the type scale ----------------------------------------------------------
//
// Every text size in the chrome is one of these roles. A source scan
// (`tests/text_scale.rs`) refuses a literal size outside this file and
// `theme.rs`; a role with a reason of its own names a constant beside its
// painter from one of these. `EYEBROW_PT` is the scale's smallest step.

/// Captions, counts, badges and menu accelerators.
pub const CAPTION_PT: f32 = 10.5;

/// Notes, menu rows and secondary list text.
pub const SMALL_PT: f32 = 11.5;

/// Type size of the text in a control: a button's word, a field's value, a
/// dropdown's choice and its list rows (egui's `TextStyle::Button`).
pub const CONTROL_PT: f32 = 12.0;

/// egui's `TextStyle::Body`: a tooltip's text and a label given no size.
pub const BODY_PT: f32 = CONTROL_PT;

/// A surface's lead line: the inspector's layer name, the tab's document path,
/// the palette's query.
pub const HEAD_PT: f32 = 13.0;

/// A card's or a dialog's title.
pub const TITLE_PT: f32 = 14.0;

/// A screen's heading: the dashboard's section and the import steps' numbers.
pub const HEADING_PT: f32 = 17.0;

/// Size of a glyph in a [`CONTROL_H`] control: a field button's, a prefix's, a
/// segment's, a dropdown's leading glyph. The dropdown chevron is
/// [`COMBO_CHEVRON_PT`].
pub const CONTROL_GLYPH_PT: f32 = 15.0;

/// [`CONTROL_GLYPH_PT`]'s twin in a [`TOOLBAR_H`] control.
pub const TOOLBAR_GLYPH_PT: f32 = 14.0;

/// How tall a control is in a bar laid over the canvas (Compare, Contrast) or a
/// top-bar pill: [`Scale::Toolbar`].
pub const TOOLBAR_H: f32 = 24.0;

/// [`SEGMENT_CELL_H`]'s twin for a [`TOOLBAR_H`] track.
pub const TOOLBAR_CELL_H: f32 = TOOLBAR_H - 4.0;

/// The two control scales. Everything is [`Scale::Card`] but the bars over the
/// canvas; a primitive reads its glyph and type sizes off the height it is given,
/// so a caller cannot pick a size of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scale {
    /// [`CONTROL_H`]: cards, popovers, dialogs, the dashboard.
    Card,
    /// [`TOOLBAR_H`]: the bars over the canvas and the top bar's pills.
    Toolbar,
}

impl Scale {
    /// The scale a control `h` tall belongs to: anything shorter than
    /// [`CONTROL_H`] is drawn at the toolbar's sizes.
    pub fn of(h: f32) -> Self {
        if h < CONTROL_H - 0.5 {
            Self::Toolbar
        } else {
            Self::Card
        }
    }

    /// A glyph's size at this scale.
    pub fn glyph_pt(self) -> f32 {
        match self {
            Self::Card => CONTROL_GLYPH_PT,
            Self::Toolbar => TOOLBAR_GLYPH_PT,
        }
    }

    /// A control's text size at this scale.
    pub fn text_pt(self) -> f32 {
        match self {
            Self::Card => CONTROL_PT,
            Self::Toolbar => SMALL_PT,
        }
    }
}

/// The gap between two controls side by side in one row — [`CARD_ROW_GAP`]'s
/// horizontal twin, set by the gap between the Transform card's X and Y.
///
/// A row that splits the card writes it twice, as `item_spacing.x` and in its own
/// width arithmetic; changing only one wraps the last control onto its own line.
pub const CARD_COL_GAP: f32 = 7.0;

/// [`card`], plus the card's outer rect — what a caller needs to make the whole
/// card interactive (see [`crate::app::OndinApp::panel`]).
pub fn card_at<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> (R, egui::Rect) {
    let out = egui::Frame::NONE
        .fill(color::card())
        .stroke(egui::Stroke::new(1.0, theme::color::text_a(15)))
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: egui::Color32::from_black_alpha(70),
        })
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(CARD_MARGIN_X as i8, 12))
        .show(ui, |ui| {
            // A `Frame` shrink-wraps, so every card is told to span the column.
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = CARD_ROW_GAP;
            add(ui)
        });
    (out.inner, out.response.rect)
}

/// What the user clicked on a [`section_head`] row.
#[derive(Default, Clone, Copy)]
pub struct HeadClicks {
    /// The caret or label — collapse or expand the panel.
    pub toggled: bool,
    /// The optional right-hand action icon (`+` on Fill and Stroke).
    pub acted: bool,
    /// The card-level reset a header with overrides offers on hover.
    pub reset: bool,
    /// The box the action icon paints, not its padded hit target, for a `+` that
    /// opens a menu.
    pub action_rect: Option<egui::Rect>,
    /// The `⋯` of a style slot's card.
    pub more: bool,
    /// The box the `⋯` paints, for its menu to hang from.
    pub more_rect: Option<egui::Rect>,
    /// The header's [`HeadDrop`] was pressed.
    pub drop: bool,
    /// The box the [`HeadDrop`] paints, for its menu to hang from.
    pub drop_rect: Option<egui::Rect>,
}

/// A small dropdown on a header's right, its value and a caret: the Appearance
/// card's blend mode. Dim at the default and full strength otherwise, so a
/// changed value shows with the card closed.
#[derive(Clone, Copy)]
pub struct HeadDrop<'a> {
    pub text: &'a str,
    pub strong: bool,
    pub tooltip: &'a str,
}

/// A card's overrides, for its header: a dot and `count` at rest, and on hover a
/// card-level reset named `reset` (*Reset effects*).
#[derive(Clone, Copy)]
pub struct HeadOverrides<'a> {
    pub count: usize,
    pub reset: &'a str,
}

/// The control on the right of a [`section_head`], if it has one.
///
/// `active` (a toggle such as Transform's crosshair stays tinted on) and `enabled`
/// (pressable; a control with nothing to act on stays in place) are independent,
/// so a one-shot can be inert ([`HeadAction::inert`]) and so can a toggle.
#[derive(Clone, Copy)]
pub struct HeadAction {
    pub glyph: &'static str,
    /// Accent-tinted while on. `false` for a one-shot action.
    pub active: bool,
    /// `false` dims the glyph and makes the click a no-op — for a control that
    /// has nothing to act on, which must say so rather than take the press.
    pub enabled: bool,
    pub tooltip: &'static str,
}

impl HeadAction {
    /// A one-shot action — the `+` that adds a paint.
    pub fn new(glyph: &'static str, tooltip: &'static str) -> Self {
        Self {
            glyph,
            active: false,
            enabled: true,
            tooltip,
        }
    }

    /// A one-shot action with nothing to act on: dimmed, inert, and still there so
    /// the panel does not jump. The tooltip carries the reason.
    pub fn inert(glyph: &'static str, tooltip: &'static str) -> Self {
        Self {
            glyph,
            active: false,
            enabled: false,
            tooltip,
        }
    }

    /// A switch that stays on.
    pub fn toggle(glyph: &'static str, active: bool, enabled: bool, tooltip: &'static str) -> Self {
        Self {
            glyph,
            active,
            enabled,
            tooltip,
        }
    }
}

/// A collapsible section header: caret + eyebrow, and an optional right-hand
/// action icon. The whole row toggles, except the action icon's corner. `sense`
/// is false when the caller senses something larger around this row (a collapsed
/// panel's whole card); overlapping targets would toggle twice.
pub fn section_head(
    ui: &mut egui::Ui,
    label: &str,
    open: bool,
    action: Option<HeadAction>,
    sense: bool,
) -> HeadClicks {
    section_head_badged(ui, label, open, action, None, sense)
}

/// [`section_head`] with an optional badge at the right-hand end, left of the
/// action icon: a small accent chip naming a state the card is in. Inert: the row
/// still toggles through it.
pub fn section_head_badged(
    ui: &mut egui::Ui,
    label: &str,
    open: bool,
    action: Option<HeadAction>,
    badge: Option<&str>,
    sense: bool,
) -> HeadClicks {
    section_head_full(ui, label, open, action, badge, None, false, None, sense)
}

/// The width a header's `⋯` takes from the toggle target beside it: its box and
/// the gap to the `+`.
pub const HEAD_MORE_W: f32 = HEAD_ACTION_W + HEAD_MORE_GAP;
/// The laid-out gap between a header's `+` and `⋯` boxes.
const HEAD_MORE_GAP: f32 = 5.0;

/// The chip a header's reset takes on hover: its height, and its padding either
/// side of the glyph-and-word.
const HEAD_RESET_H: f32 = 18.0;
const HEAD_RESET_PAD: f32 = 6.0;

/// [`section_head_badged`] with the card's overrides: after the label, a 4-pt
/// neutral dot and the count; with the pointer over the header, a chip
/// (*↺ Reset effects*) whose click is [`HeadClicks::reset`]. The chip's slot is
/// reserved at rest, and registered after the toggle target so it takes the press.
/// `more` puts a `⋯` last at the right-hand end ([`HeadClicks::more`]); `drop`
/// puts a [`HeadDrop`] left of everything else there ([`HeadClicks::drop`]).
#[allow(clippy::too_many_arguments)]
pub fn section_head_full(
    ui: &mut egui::Ui,
    label: &str,
    open: bool,
    action: Option<HeadAction>,
    badge: Option<&str>,
    overrides: Option<HeadOverrides<'_>>,
    more: bool,
    drop: Option<HeadDrop<'_>>,
    sense: bool,
) -> HeadClicks {
    let mut clicks = HeadClicks::default();
    let overrides = overrides.filter(|o| o.count > 0);
    let mut chip: Option<egui::Rect> = None;
    let row = ui
        .horizontal(|ui| {
            // Scoped so the 6px pitch stops before the action icon.
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(theme::icon_text(
                    if open {
                        icon::CARET_DOWN
                    } else {
                        icon::CARET_RIGHT
                    },
                    12.0,
                    theme::text::faint(),
                ));
                ui.label(eyebrow(label));
                if let Some(o) = overrides {
                    let w = ui
                        .painter()
                        .layout_no_wrap(
                            o.reset.to_owned(),
                            egui::FontId::proportional(SMALL_PT),
                            theme::text::muted(),
                        )
                        .size()
                        .x
                        + 12.0
                        + 4.0
                        + HEAD_RESET_PAD * 2.0;
                    chip = Some(
                        ui.allocate_exact_size(egui::vec2(w, HEAD_RESET_H), egui::Sense::hover())
                            .0,
                    );
                }
            });

            if action.is_some() || badge.is_some() || more || drop.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if more {
                        let pad = egui::Margin {
                            left: (HEAD_MORE_GAP / 2.0) as i8,
                            right: CARD_MARGIN_X as i8,
                            top: HEAD_ACTION_PAD as i8,
                            bottom: HEAD_ACTION_PAD as i8,
                        };
                        let resp = icon_button_padded(
                            ui,
                            icon::DOTS_THREE,
                            egui::Vec2::splat(HEAD_ACTION_W),
                            15.0,
                            false,
                            true,
                            pad,
                        )
                        .on_hover_text("Styles");
                        clicks.more = resp.clicked();
                        clicks.more_rect = Some(resp.rect - pad);
                        // The pads allocate nothing, so the gap is laid out here.
                        ui.add_space(HEAD_MORE_GAP - ui.spacing().item_spacing.x);
                    }
                    if let Some(a) = action {
                        let pad = egui::Margin {
                            left: HEAD_ACTION_PAD as i8,
                            // The rest of the gap the `⋯`'s half left, so an odd
                            // gap is not rounded down a point.
                            right: if more {
                                (HEAD_MORE_GAP - (HEAD_MORE_GAP / 2.0).floor()) as i8
                            } else {
                                CARD_MARGIN_X as i8
                            },
                            top: HEAD_ACTION_PAD as i8,
                            bottom: HEAD_ACTION_PAD as i8,
                        };
                        let resp = icon_button_padded(
                            ui,
                            a.glyph,
                            egui::Vec2::splat(HEAD_ACTION_W),
                            15.0,
                            a.active,
                            a.enabled,
                            pad,
                        )
                        .on_hover_text(a.tooltip);
                        clicks.acted = resp.clicked() && a.enabled;
                        // The painted box, so a menu hung from it sits where the ⋯'s does.
                        clicks.action_rect = Some(resp.rect - pad);
                    }
                    if let Some(word) = badge {
                        head_badge(ui, word);
                    }
                    if let Some(d) = drop {
                        let resp = head_drop(ui, d);
                        clicks.drop = resp.clicked();
                        clicks.drop_rect = Some(resp.rect);
                    }
                });
            }
        })
        .response;

    if sense {
        // `horizontal` shrink-wraps; stretch the target to the card's width.
        let mut hit = row.rect;
        hit.max.x = hit.max.x.max(ui.max_rect().right());
        if action.is_some() {
            // The action's left pad comes off so the targets meet, not overlap.
            hit.max.x -= HEAD_ACTION_W + HEAD_ACTION_PAD;
        }
        if more {
            hit.max.x -= if action.is_some() {
                HEAD_MORE_W
            } else {
                HEAD_ACTION_W + HEAD_ACTION_PAD
            };
        }
        // The drop is the leftmost of the right-hand controls, so the target
        // stops at it.
        if let Some(d) = clicks.drop_rect {
            hit.max.x = hit.max.x.min(d.left());
        }
        let head = ui.interact(
            hit,
            ui.id().with(("section-head", label)),
            egui::Sense::click(),
        );
        clicks.toggled = head.clicked();
    }
    if let (Some(o), Some(slot)) = (overrides, chip) {
        let mut over = row.rect;
        over.max.x = over.max.x.max(ui.max_rect().right());
        // Not when `sense` is false: the whole-card target would take the press.
        let hovered = sense && ui.rect_contains_pointer(over);
        // Registered whenever the head senses, so `Tab` reaches it; shown while the
        // pointer is over the head or the keyboard is on it.
        let target = sense.then(|| {
            ui.interact(
                slot,
                ui.id().with(("section-reset", label)),
                egui::Sense::click(),
            )
        });
        let p = ui.painter();
        if let Some(target) = target.filter(|t| hovered || t.has_focus()) {
            let ground = if target.hovered() {
                color::hover()
            } else {
                color::field()
            };
            p.rect_filled(slot, 4.0, ground);
            let mid = slot.center().y;
            p.text(
                egui::pos2(slot.left() + HEAD_RESET_PAD + 6.0, mid),
                egui::Align2::CENTER_CENTER,
                icon::ARROW_COUNTER_CLOCKWISE,
                theme::icon_font(12.0),
                theme::text::muted(),
            );
            p.text(
                egui::pos2(slot.left() + HEAD_RESET_PAD + 16.0, mid),
                egui::Align2::LEFT_CENTER,
                o.reset,
                egui::FontId::proportional(SMALL_PT),
                theme::text::strong(),
            );
            if target.clicked() {
                clicks.reset = true;
                // The header's toggle sits under the chip; the reset is the press.
                clicks.toggled = false;
                // The chip goes with the overrides; the keyboard lands on the head.
                if target.has_focus() {
                    let head = ui.id().with(("section-head", label));
                    ui.memory_mut(|m| m.request_focus(head));
                }
            }
        } else {
            override_dot(p, egui::pos2(slot.left() + 2.0, slot.center().y));
            p.text(
                egui::pos2(slot.left() + 8.0, slot.center().y),
                egui::Align2::LEFT_CENTER,
                o.count.to_string(),
                egui::FontId::proportional(CAPTION_PT),
                theme::text::muted(),
            );
        }
    }
    clicks
}

/// The override mark: a 4-pt neutral dot centred at `at`, on a card header, beside
/// an overridden field's label, and in a list item's trailing slot. Neutral
/// because the accent means selected.
pub fn override_dot(p: &egui::Painter, at: egui::Pos2) {
    p.circle_filled(at, 2.0, theme::text::muted());
}

/// The centre of every override dot a frame painted (2-pt muted circle, also the
/// layers panel's row dot).
#[cfg(test)]
pub(crate) fn painted_override_dots(out: &egui::FullOutput) -> Vec<egui::Pos2> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Pos2>) {
        match shape {
            egui::Shape::Circle(c) if c.radius == 2.0 && c.fill == theme::text::muted() => {
                out.push(c.center);
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut v = Vec::new();
    for s in &out.shapes {
        walk(&s.shape, &mut v);
    }
    v
}

/// A [`section_head_badged`] badge: an 18-pt accent chip, accent 18% under 10-pt
/// accent-100 text, 6 pt either side, a 4-pt corner.
fn head_badge(ui: &mut egui::Ui, word: &str) {
    let galley = ui.painter().layout_no_wrap(
        word.to_owned(),
        egui::FontId::proportional(CAPTION_PT),
        color::accent_100(),
    );
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 12.0, 18.0),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, 4.0, color::accent().gamma_multiply(0.18));
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        color::accent_100(),
    );
}

/// A [`HeadDrop`]: [`SMALL_PT`] text and a 10-pt caret 4 pt after it, muted at
/// the default and strong otherwise or under the pointer.
fn head_drop(ui: &mut egui::Ui, d: HeadDrop<'_>) -> egui::Response {
    let font = egui::FontId::proportional(SMALL_PT);
    let text_w = ui
        .painter()
        .layout_no_wrap(d.text.to_owned(), font.clone(), theme::text::strong())
        .size()
        .x;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(text_w + 14.0, 18.0), egui::Sense::click());
    let ink = if d.strong || resp.hovered() {
        theme::text::strong()
    } else {
        theme::text::muted()
    };
    let p = ui.painter();
    let mid = rect.center().y;
    p.text(
        egui::pos2(rect.left(), mid),
        egui::Align2::LEFT_CENTER,
        d.text,
        font,
        ink,
    );
    p.text(
        egui::pos2(rect.right() - 5.0, mid),
        egui::Align2::CENTER_CENTER,
        icon::CARET_DOWN,
        theme::icon_font(10.0),
        ink,
    );
    resp.on_hover_text(d.tooltip)
}

/// Horizontal inset between a [`field_frame`]'s hairline and its contents; [`grip`]
/// claims that band as part of its hit target.
pub const FIELD_PAD_X: f32 = 9.0;

/// Largest stroke weight, per-side width, and dash length or spacing the fields
/// accept, in world units.
///
/// A cap on what can be typed or scrubbed, not on the model, which allows any
/// positive width.
pub const MAX_STROKE_WIDTH: f64 = 10_000.0;

/// The recessed ground a value field or list row sits on.
pub fn field_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(color::field())
        .stroke(egui::Stroke::new(1.0, color::field_border()))
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(FIELD_PAD_X as i8, 0))
}

/// Vertical space [`field_frame`]'s hairline adds outside its content.
///
/// egui counts a `Frame`'s stroke width as margin, so a frame told to hold 28px
/// of content paints 30. Anything that must line up with a field row subtracts
/// this, or a 28px [`field_button`] reads shorter than the field beside it.
pub const FIELD_BORDER_H: f32 = 2.0;

/// A field row of exactly `size`: the recessed ground, laid out left to right
/// and vertically centred (`Frame::show` would inherit a panel column's layout).
/// `size.y` is the painted ground, hairline included; the content is shorter by
/// [`FIELD_BORDER_H`], and allocating the full `size.y` inside grows the frame.
pub fn field_row<R>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    framed_row(ui, size, field_frame(), add)
}

/// [`field_row`] with its own side insets, for a row that starts on a button
/// rather than text (the picker's value row).
pub fn field_row_inset<R>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    left: f32,
    right: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let margin = egui::Margin {
        left: left as i8,
        right: right as i8,
        top: 0,
        bottom: 0,
    };
    framed_row(ui, size, field_frame().inner_margin(margin), add)
}

/// [`field_row`]'s body, for the callers that need a different frame.
fn framed_row<R>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    frame: egui::Frame,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let row = ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            frame
                .show(ui, |ui| {
                    ui.set_min_height(size.y - FIELD_BORDER_H);
                    add(ui)
                })
                .inner
        },
    );
    if typing_inside(ui.ctx(), row.response.rect) {
        typing_edge(ui, row.response.rect);
    }
    row.inner
}

/// Whether the focused widget is a text entry lying inside `rect`: a field row's
/// own edit, a number being typed, so the row wears [`typing_edge`] whatever its
/// caller composed into it. A focused button in the row is keyboard focus, which
/// [`focus_ring`] shows, so only a widget holding a `TextEdit`'s state counts.
fn typing_inside(ctx: &egui::Context, rect: egui::Rect) -> bool {
    // Separate reads: the context lock is not re-entrant.
    let Some(id) = ctx.memory(|m| m.focused()) else {
        return false;
    };
    let Some(w) = widget_rect(ctx, id) else {
        return false;
    };
    rect.expand(0.5).contains_rect(w.rect) && egui::TextEdit::load_state(ctx, id).is_some()
}

/// Whether this frame's `Enter` is a card's own *Apply*: pressed with nothing
/// focused, or in a text entry. On a focused button or box `Enter` is that
/// control's click, so a focused *Cancel* cancels rather than applies.
pub fn card_enter(ctx: &egui::Context) -> bool {
    if !ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
        return false;
    }
    // Separate reads: the context lock is not re-entrant.
    match ctx.memory(|m| m.focused()) {
        None => true,
        Some(id) => egui::TextEdit::load_state(ctx, id).is_some(),
    }
}

/// [`field_row`] with an accent border, marking the paint the open picker is
/// pointing at.
pub fn field_row_active<R>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    active: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    if !active {
        return field_row(ui, size, add);
    }
    let frame = field_frame().stroke(egui::Stroke::new(1.0, color::accent_700()));
    framed_row(ui, size, frame, add)
}

/// A single-line **text** field on the same ground as [`value_field`]. A bare
/// `ui.add(TextEdit…)` differs in its frame, its inset and a focus ring nothing
/// else draws. A custom frame turns the ring off but also drops `TextEdit::margin`
/// (read only when no frame was passed), so padding goes in the frame
/// ([`edit_frame`]), which covers the whole row.
/// `size.y` is the painted ground, hairline included — see [`field_row`]. Its type
/// is the row's [`Scale`]'s, and it wears [`typing_edge`] while it holds focus.
pub fn text_field(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    text: &mut String,
    hint: &str,
) -> egui::Response {
    let pt = Scale::of(size.y).text_pt();
    let font = egui::FontId::proportional(pt);
    let frame = edit_frame(ui, &font, size.y - FIELD_BORDER_H, FIELD_PAD_X);
    // The row paints the typing edge (`framed_row`).
    field_row_inset(ui, size, 0.0, 0.0, |ui| {
        frame.add(
            ui,
            f32::INFINITY,
            egui::TextEdit::singleline(text)
                .font(font)
                .hint_text(egui::RichText::new(hint).size(pt)),
        )
    })
}

/// Typing focus: the `accent_700` hairline on a text entry's own edge while it
/// holds focus, however it got it. Every field a person types in wears it —
/// value, text, search, rename — painted after the field, over its own hairline;
/// a [`label_edit`], which has no edge, shows its caret instead.
pub fn typing_edge(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(BUTTON_R),
        egui::Stroke::new(1.0, color::accent_700()),
        egui::StrokeKind::Inside,
    );
}

/// A dropdown's closed face when its choice has no glyph: the word at
/// [`CONTROL_PT`] in the value ink, [`glyph_and_text`]'s glyphless twin.
pub fn combo_text(text: &str) -> egui::WidgetText {
    egui::RichText::new(text)
        .size(CONTROL_PT)
        .color(color::text())
        .into()
}

/// The side of a [`search_box`]'s clear button, and its inset from the field's
/// right edge.
const SEARCH_CLEAR: f32 = 20.0;
const SEARCH_CLEAR_INSET: f32 = 4.0;

/// The one search field, in a panel, a dialog or at the head of a dropdown's
/// list: [`text_field`]'s ground and [`typing_edge`], a faint magnifier before
/// the text and, once it holds something, a clear button after it. A press on
/// the magnifier focuses the field too; a press on the clear button empties it,
/// keeps the keyboard in it and reports the edit as changed.
pub fn search_box(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    text: &mut String,
    hint: &str,
    id: egui::Id,
) -> egui::Response {
    let scale = Scale::of(size.y);
    let pt = scale.text_pt();
    let font = egui::FontId::proportional(pt);
    let frame = edit_frame(ui, &font, size.y - FIELD_BORDER_H, FIELD_PAD_X).left(0.0);
    let clear_side = SEARCH_CLEAR.min(size.y - 8.0);
    // The row paints the typing edge (`framed_row`).
    field_row_inset(ui, size, FIELD_PAD_X, 0.0, |ui| {
        // Registered first, so the edit above it wins its own area and this
        // takes only the magnifier's. The bare `CLICK` flag: `Sense::click()`
        // adds `FOCUSABLE`, a tab stop in front of the edit.
        let row = ui.interact(ui.max_rect(), id.with("row"), egui::Sense::CLICK);
        ui.label(theme::icon_text(
            icon::MAGNIFYING_GLASS,
            scale.glyph_pt(),
            theme::text::faint(),
        ));
        // The edit stops short of the clear button rather than lying under it, so
        // the text never runs beneath the ✕ and the pointer there is an arrow.
        // Laid out from the right, the ✕ first, so the edit fills what is left
        // and the row is as wide with the ✕ as without it.
        let clearable = !text.is_empty();
        let (mut edit, cleared) = ui
            .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cleared = clearable && {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(SEARCH_CLEAR_INSET);
                    search_clear(ui, clear_side, scale.glyph_pt() - 2.0)
                };
                let edit = frame.add(
                    ui,
                    f32::INFINITY,
                    egui::TextEdit::singleline(text)
                        .id(id)
                        .font(font)
                        .hint_text(egui::RichText::new(hint).size(pt)),
                );
                (edit, cleared)
            })
            .inner;
        if row.clicked() {
            edit.request_focus();
        }
        if cleared {
            text.clear();
            edit.mark_changed();
            edit.request_focus();
            // The edit was drawn holding the old text.
            ui.ctx().request_repaint();
        }
        edit
    })
}

/// A [`search_box`]'s ✕: a hover ground and an arrow pointer, no tab stop.
/// Whether it was pressed.
fn search_clear(ui: &mut egui::Ui, side: f32, glyph: f32) -> bool {
    ui.spacing_mut().item_spacing.x = 0.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::CLICK);
    let hovered = resp.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(BUTTON_R), color::hover());
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon::X,
        theme::icon_font(glyph),
        match hovered {
            true => color::text(),
            false => theme::text::dim(),
        },
    );
    resp.on_hover_text("Clear").clicked()
}

/// The inset of an [`inline_edit`]'s text from its rect's left edge; a caller
/// places the rect this far left of where the label it replaces was drawn.
pub const INLINE_EDIT_PAD_X: f32 = 6.0;

/// A rename field laid over a list row or card (a document card, a version): the
/// field ground filling `rect`, the row's own height rather than [`CONTROL_H`],
/// text at [`CONTROL_PT`], and [`typing_edge`] while it holds focus.
pub fn inline_edit(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    text: &mut String,
    id: egui::Id,
) -> egui::Response {
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(BUTTON_R), color::field());
    let resp = label_edit(ui, rect, text, id);
    if resp.has_focus() {
        typing_edge(ui, rect);
    }
    resp
}

/// [`inline_edit`] with no ground and no edge: the label itself turns editable
/// where it was drawn, caret and selection only. For a name typed over in place
/// in a tree (the layers' and pages' rows), where a field would cover the row.
/// The label must be drawn at [`CONTROL_PT`] for the text not to move.
pub fn label_edit(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    text: &mut String,
    id: egui::Id,
) -> egui::Response {
    let font = egui::FontId::proportional(CONTROL_PT);
    let frame = edit_frame(ui, &font, rect.height(), INLINE_EDIT_PAD_X);
    ui.put(
        rect,
        frame
            .on(egui::TextEdit::singleline(text))
            .id(id)
            .desired_width(rect.width() - INLINE_EDIT_PAD_X * 2.0)
            .font(font),
    )
}

/// The side of a [`paint_tick_box`].
pub const TICK_BOX: f32 = 14.0;

/// How a list row whose box is unticked fades its ink: still legible, plainly
/// left out (tokens import's rows, Colour review's skipped members).
pub const UNTICKED_FADE: f32 = 0.7;

/// The box of a [`tick_box`], centred on `at`, for a list row that carries one
/// (tokens import's, Colour review's): the accent with a check when `on`, else a
/// ring that brightens under the pointer.
pub fn paint_tick_box(p: &egui::Painter, at: egui::Pos2, on: bool, hovered: bool) {
    let tick = egui::Rect::from_center_size(at, egui::vec2(TICK_BOX, TICK_BOX));
    if on {
        p.rect_filled(tick, 3.0, color::accent());
        p.text(
            tick.center(),
            egui::Align2::CENTER_CENTER,
            icon::CHECK,
            theme::icon_font(10.0),
            color::accent_ink(),
        );
    } else {
        let ring = match hovered {
            true => color::text_a(140),
            false => color::text_a(90),
        };
        p.rect_stroke(
            tick,
            3.0,
            egui::Stroke::new(1.0, ring),
            egui::StrokeKind::Inside,
        );
    }
}

/// A tick box, its label and an optional faint note after it, one
/// [`MENU_ROW_H`] row whose whole width is the target and a tab stop.
pub fn tick_box(ui: &mut egui::Ui, label: &str, note: &str, on: &mut bool) -> egui::Response {
    /// The box, and the gaps after it and after the label.
    const BOX: f32 = TICK_BOX;
    const GAP: f32 = 7.0;
    const NOTE_GAP: f32 = 5.0;
    let font = egui::FontId::proportional(CONTROL_PT);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), color::text()));
    let note_g = ui.fonts_mut(|f| f.layout_no_wrap(note.to_owned(), font, theme::text::faint()));
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(
            BOX + GAP + galley.size().x + NOTE_GAP + note_g.size().x,
            MENU_ROW_H,
        ),
        egui::Sense::click(),
    );
    let p = ui.painter();
    let tick = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - BOX / 2.0),
        egui::vec2(BOX, BOX),
    );
    paint_tick_box(p, tick.center(), *on, resp.hovered());
    let x = tick.right() + GAP;
    let w = galley.size().x;
    p.galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color::text(),
    );
    p.galley(
        egui::pos2(x + w + NOTE_GAP, rect.center().y - note_g.size().y / 2.0),
        note_g,
        theme::text::faint(),
    );
    if resp.clicked() {
        *on = !*on;
    }
    resp
}

/// The side of a [`field_toggle`]: a segment cell's height, so a toggle inside a
/// field lights as a [`segmented`] cell does.
pub const FIELD_TOGGLE: egui::Vec2 = egui::vec2(24.0, CONTROL_H - 6.0);

/// A switch that lives inside a text field (Find's and Batch rename's `Aa`, `.*`,
/// `Ab`): lit as a [`segmented`] cell is, with `text` at [`SMALL_PT`] (`mono` for the
/// regex dot-star). Flips `on` on a click; `tip` says what it matches.
pub fn field_toggle(
    ui: &mut egui::Ui,
    text: &str,
    tip: &str,
    on: &mut bool,
    mono: bool,
) -> egui::Response {
    let font = match mono {
        true => egui::FontId::monospace(SMALL_PT),
        false => egui::FontId::proportional(SMALL_PT),
    };
    let (rect, resp) = ui.allocate_exact_size(FIELD_TOGGLE, egui::Sense::click());
    segment_ground(ui.painter(), rect, *on, resp.hovered());
    let ink = match *on {
        true => color::text(),
        false => theme::text::dim(),
    };
    ui.painter()
        .text(rect.center(), egui::Align2::CENTER_CENTER, text, font, ink);
    let resp = resp.on_hover_text(tip);
    if resp.clicked() {
        *on = !*on;
    }
    resp
}

/// A selectable row's ground in a list that is not a dropdown (Find's results,
/// the compare list, a picker's faces): lit as a [`segmented`] cell is when it is
/// the current one, [`color::hover`] under the pointer, at a menu row's radius.
pub fn list_row_ground(painter: &egui::Painter, rect: egui::Rect, lit: bool, hovered: bool) {
    segment_ground(painter, rect, lit, hovered);
}

/// [`list_row_ground`]'s corner.
pub const LIST_ROW_RADIUS: u8 = 4;

/// [`list_row_ground`]'s fill, or none at rest: for a row whose ground runs on
/// into what it opens below it (Version history's opened row and its body), which
/// paints its own corners.
pub fn list_row_fill(lit: bool, hovered: bool) -> Option<egui::Color32> {
    match (lit, hovered) {
        (true, _) => Some(theme::color::text_a(33)),
        (false, true) => Some(color::hover()),
        (false, false) => None,
    }
}

/// The frame a frameless `TextEdit` in a field row takes, for a row whose content
/// is `h` tall, with `x` on either side for a row whose own inset is zero. The
/// edit fills the row's height and its line sits on the row's centre, where the
/// labels and glyphs beside it sit; the padding is the edit's, so a press
/// anywhere on that height focuses it and the pointer is an I-beam there. With
/// `Frame::NONE` the edit is one line tall and the bands above and below it are
/// dead.
pub fn edit_frame(ui: &egui::Ui, font: &egui::FontId, h: f32, x: f32) -> EditFrame {
    let row = ui.fonts_mut(|f| f.row_height(font));
    // Rounded down: a point over `h` would grow the row.
    let y = ((h - row).max(0.0) / 2.0).floor() as i8;
    EditFrame {
        frame: egui::Frame::NONE.inner_margin(egui::Margin {
            left: x as i8,
            right: x as i8,
            top: y,
            bottom: y,
        }),
        h,
    }
}

/// An [`edit_frame`]: whole-point padding, the same above and below the line, and
/// the row height it was made for.
///
/// A line is not whole points, so the padding alone leaves the edit short of the
/// row, and padding uneven by a point puts the line half a point off the row's
/// centre. So the edit is always stretched over a box of the row's height, with
/// its line centred in it: [`Self::show`] lays that box in a row, and
/// [`Self::on`] is for an edit `put` or `place`d over a box already.
#[derive(Clone, Copy, Debug)]
pub struct EditFrame {
    frame: egui::Frame,
    h: f32,
}

impl EditFrame {
    /// The inset before the text, for a row whose lead (a glyph, a prefix) is
    /// part of the edit.
    pub fn left(mut self, x: f32) -> Self {
        self.frame.inner_margin.left = x as i8;
        self
    }

    /// The inset after the text.
    pub fn right(mut self, x: f32) -> Self {
        self.frame.inner_margin.right = x as i8;
        self
    }

    /// `edit` in this frame, its line centred in the box it is stretched over.
    pub fn on<'t>(self, edit: egui::TextEdit<'t>) -> egui::TextEdit<'t> {
        edit.frame(self.frame).vertical_align(egui::Align::Center)
    }

    /// `edit` in this frame, laid in `ui` over a box `w` wide (at most what is
    /// left) and the row's height.
    pub fn show(
        self,
        ui: &mut egui::Ui,
        w: f32,
        edit: egui::TextEdit<'_>,
    ) -> egui::text_edit::TextEditOutput {
        let size = egui::vec2(w.min(ui.available_width()), self.h);
        let stretch = egui::Layout::centered_and_justified(egui::Direction::TopDown);
        ui.allocate_ui_with_layout(size, stretch, |ui| {
            self.on(edit).desired_width(size.x).show(ui)
        })
        .inner
    }

    /// [`Self::show`]'s response.
    pub fn add(self, ui: &mut egui::Ui, w: f32, edit: egui::TextEdit<'_>) -> egui::Response {
        self.show(ui, w, edit).response.response
    }
}

/// How far a value field's number travels per point of pointer movement,
/// as a multiple of the rate the field asks for.
///
/// The one number to turn when scrubbing feels wrong.
pub const SCRUB_SCALE: f64 = 0.5;

/// A field's drag speed: the rate it asks for, at the current [`SCRUB_SCALE`].
pub fn scrub(base: f64) -> f64 {
    base * SCRUB_SCALE
}

/// How a [`value_field`] behaves under the hand.
///
/// The speed and the rounding rule travel together: [`value_field`] needs the
/// speed for a drag on the prefix and the rule to round the result.
#[derive(Clone, Debug)]
pub struct Scrub {
    speed: f64,
    /// Decimal places a drag settles to. Typing is never rounded. Stated rather
    /// than inferred so a `DragValue`'s own drag and the prefix strip's agree.
    decimals: u32,
    range: std::ops::RangeInclusive<f64>,
}

impl Scrub {
    /// Whole units under the hand, any number from the keyboard.
    ///
    /// The default for anything in the document's own units. Rounding is keyed on
    /// the drag, never on focus, which would round a typed 1.5 away on blur.
    pub fn whole(base: f64) -> Self {
        Self {
            speed: scrub(base),
            decimals: 0,
            range: f64::NEG_INFINITY..=f64::INFINITY,
        }
    }

    /// `decimals` places under the hand, for numbers where a fraction is the point
    /// (a line-height multiplier). Typing is still unrestricted.
    pub fn fine(base: f64, decimals: u32) -> Self {
        Self {
            decimals,
            ..Self::whole(base)
        }
    }

    /// Clamp the value to `range`, both while dragging and when typed.
    ///
    /// An inverted range is swapped, as `DragValue` does, because `f64::clamp`
    /// panics on `min > max`. A NaN bound drops the range instead, since
    /// `f64::min` would pin the field to one number.
    pub fn range(mut self, range: std::ops::RangeInclusive<f64>) -> Self {
        let (a, b) = (*range.start(), *range.end());
        self.range = if a.is_nan() || b.is_nan() {
            f64::NEG_INFINITY..=f64::INFINITY
        } else {
            a.min(b)..=a.max(b)
        };
        self
    }

    /// `v` as this rule would leave it after a drag.
    fn settle(&self, v: f64) -> f64 {
        let scale = 10_f64.powi(self.decimals as i32);
        ((v * scale).round() / scale).clamp(*self.range.start(), *self.range.end())
    }
}

/// Format a field's number at most `decimals` places, trailing zeros trimmed, so
/// `160` stays `160` while `411.672736…` reads `411.67`. For
/// `DragValue::custom_formatter`: it would otherwise derive its precision from the
/// drag speed, fifteen places on a cursor wrap's zero-speed pass.
pub fn number(decimals: usize) -> impl Fn(f64, std::ops::RangeInclusive<usize>) -> String {
    move |v, _| {
        let s = format!("{v:.decimals$}");
        let Some(trimmed) = s.contains('.').then(|| {
            let t = s.trim_end_matches('0');
            t.strip_suffix('.').unwrap_or(t)
        }) else {
            return s;
        };
        // A value rounded to zero from below formats as `-0`.
        match trimmed {
            "-0" => "0".to_owned(),
            t => t.to_owned(),
        }
    }
}

// --- infinite scrub (cursor wrapping) --------------------------------------

/// How close to the window's edge the pointer gets before a scrub wraps it to the
/// other side: wide enough that a fast hand cannot clear it between two frames.
const WRAP_EDGE: f32 = 24.0;

/// How many passes a requested teleport is waited for before giving up.
///
/// A platform that refuses `set_cursor_position` reports nothing back, so without
/// a deadline one unanswered request would block every later wrap.
const WRAP_GIVE_UP: u64 = 20;

/// Where the state for [`wrap_scrub`] lives. One slot, because only one field can be
/// dragged at a time.
fn wrap_id() -> egui::Id {
    egui::Id::new("scrub-wrap")
}

/// A teleport that has been asked for and not yet seen to land.
#[derive(Clone, Copy)]
struct Wrap {
    /// The pass it was requested on, for [`WRAP_GIVE_UP`].
    asked_on: u64,
    /// How far the pointer was asked to jump, in points along x.
    distance: f32,
}

/// The teleport in flight, if one has been asked for.
fn wrap_pending(ctx: &egui::Context) -> Option<Wrap> {
    ctx.data(|d| d.get_temp(wrap_id()))
}

/// Whether this pass's pointer delta is a teleport landing rather than the hand moving.
///
/// A teleport is one enormous flick of `drag_delta`, spent on a zero speed
/// ([`scrub_speed_for`]). Recognised by size, not timing: the OS posts the move
/// several passes late.
fn wrap_landed(ctx: &egui::Context, pending: Wrap) -> bool {
    let dx = ctx.input(|i| i.pointer.delta().x);
    dx.abs() >= pending.distance * 0.5
}

/// [`scrub_speed_for`] as a bare number, for the prefix strip. A pure read:
/// clearing the pending wrap is left to [`wrap_scrub`].
fn scrub_speed_for_value(ctx: &egui::Context, speed: f64) -> f64 {
    match wrap_pending(ctx) {
        Some(p) if wrap_landed(ctx, p) => 0.0,
        _ => speed,
    }
}

/// The `DragValue` with its speed zeroed on the pass a teleport landed on.
fn scrub_speed_for<'a>(ctx: &egui::Context, value: egui::DragValue<'a>) -> egui::DragValue<'a> {
    match wrap_pending(ctx) {
        Some(p) if wrap_landed(ctx, p) => {
            // Zero rather than skipping the widget, so the gesture survives.
            value.speed(0.0)
        }
        _ => value,
    }
}

/// Apply a prefix strip's drag to `value`, as `DragValue` does its own: the
/// unrounded value carries across the gesture and only what is shown is settled.
///
/// Settling each pass's delta on its own would drop a slow field's slivers: at
/// 0.05 a point, a hand moving under ten points a frame never reaches a whole unit.
fn scrub_strip(ctx: &egui::Context, strip: &egui::Response, scrub: &Scrub, value: &mut f64) {
    let id = strip.id.with("precise");
    // Only while it still settles to the shown value, so a value changed from
    // elsewhere mid-gesture restarts the count from what is there.
    let carried = match strip.drag_started() {
        true => None,
        false => ctx
            .data(|d| d.get_temp::<f64>(id))
            .filter(|p| scrub.settle(*p) == *value),
    };
    let speed = scrub_speed_for_value(ctx, scrub.speed);
    // Clamped, so travel past a bound is not owed back before the value moves.
    let precise = (carried.unwrap_or(*value) + f64::from(strip.drag_delta().x) * speed)
        .clamp(*scrub.range.start(), *scrub.range.end());
    ctx.data_mut(|d| d.insert_temp(id, precise));
    *value = scrub.settle(precise);
}

/// Teleport the pointer to the far side of the window if a live scrub has run it into
/// an edge, so the drag can carry on indefinitely.
///
/// The x axis only, though `DragValue` also scrubs vertically: a field near the
/// window's top would start every drag inside the trigger and teleport at once.
fn wrap_scrub(ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    // A request in flight blocks new ones, or the wrap would bounce between edges.
    if let Some(p) = wrap_pending(ctx) {
        if !wrap_landed(ctx, p) && pass <= p.asked_on + WRAP_GIVE_UP {
            return; // still waiting
        }
        ctx.data_mut(|d| d.remove::<Wrap>(wrap_id()));
    }
    let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else {
        return;
    };
    // The viewport, not `content_rect`: a cursor position is window-relative.
    let screen = ctx.viewport_rect();
    // Land one band inside the opposite trigger band so it does not wrap back.
    let inset = WRAP_EDGE * 2.0;
    // A window too narrow for two bands and a gap has nowhere to wrap to.
    if screen.width() < inset * 3.0 {
        return;
    }
    let x = if p.x <= screen.left() + WRAP_EDGE {
        screen.right() - inset
    } else if p.x >= screen.right() - WRAP_EDGE {
        screen.left() + inset
    } else {
        return;
    };
    let to = egui::pos2(x, p.y);
    ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(to));
    // Read before the `data_mut`, never inside it: nested context accessors deadlock.
    let pending = Wrap {
        asked_on: pass,
        distance: (to.x - p.x).abs(),
    };
    ctx.data_mut(|d| d.insert_temp(wrap_id(), pending));
    // The teleport arrives as an event, which wakes egui only if a repaint is asked for.
    ctx.request_repaint();
}

/// What a [`value_field`] puts in front of its number.
///
/// A word (`X`, `LH`) or an icon glyph, both in the same 38%-of-text grey. One
/// enum so a call site cannot pick the wrong size or grey.
#[derive(Clone, Copy)]
pub enum Prefix {
    /// A short word — `X`, `W`, `LH`.
    Text(&'static str),
    /// An icon glyph.
    Icon(&'static str),
    /// A CSS property's name — `flex-grow`, `min-width` — at the unit's size
    /// rather than [`PREFIX_PT`], so a whole word does not take the number's width.
    Label(&'static str),
}

/// Type size of a [`Prefix::Text`].
///
/// 13: at 10.5 a capital inks 8pt tall against an icon
/// glyph's 11; 13 inks 10pt, and matching fully would out-shout the value.
const PREFIX_PT: f32 = 13.0;
/// Type size of a [`Prefix::Icon`].
const PREFIX_ICON_PT: f32 = 15.0;
/// Space between a prefix and the number it labels.
const PREFIX_GAP: f32 = 7.0;
/// How far past its own text a clickable [`Suffix`]'s hit box reaches on each side.
const SUFFIX_HIT_PAD: f32 = 4.0;
/// Type size of the unit itself, smaller than [`PREFIX_PT`] on purpose: it sits
/// alone at the far end of a field and must not compete with the number.
const SUFFIX_PT: f32 = SMALL_PT;
/// How far the chip hangs past the field's content box into its own padding, so
/// the chip finishes inside the hairline and belongs to the field.
const SUFFIX_BLEED: f32 = 4.0;
/// The room a unit leaves its number before it is cut short with an ellipsis:
/// four digits at the field's type size.
const UNIT_DIGITS_MIN: f32 = 28.0;

/// The width a field reserves for its prefix's run `lead` and the gap after it;
/// none for a bare field (`Prefix::Text("")`), whose number then sits as far in
/// as it does from the right. Rounded so the reserved space and the paint point
/// are one number.
fn prefix_inset(lead: &egui::Galley) -> f32 {
    match lead.size().x > 0.0 {
        true => (lead.size().x + PREFIX_GAP).round(),
        false => 0.0,
    }
}

/// [`value_field`]'s prefix strip for a field drawn in another face (a bound
/// field's chip, its retype buffer, the *Mixed* capsule), reserved as
/// [`value_field`] reserves it, with the row's item spacing zeroed after it.
/// Senses nothing; answers the strip and the prefix's run for the caller to paint,
/// through [`paint_prefix`] at rest.
pub fn prefix_strip(
    ui: &mut egui::Ui,
    prefix: Prefix,
) -> (egui::Rect, std::sync::Arc<egui::Galley>) {
    let lead = prefix.galley(ui);
    let inset = prefix_inset(&lead);
    let (strip, _) = ui.allocate_exact_size(
        egui::vec2(inset, CONTROL_H - FIELD_BORDER_H),
        egui::Sense::hover(),
    );
    ui.spacing_mut().item_spacing.x = 0.0;
    (strip, lead)
}

/// A [`prefix_strip`]'s prefix in `ink`, where [`value_field`] paints its own:
/// at the strip's left, centred down it.
pub fn paint_prefix(
    painter: &egui::Painter,
    strip: egui::Rect,
    lead: std::sync::Arc<egui::Galley>,
    ink: egui::Color32,
) -> egui::Pos2 {
    let at = egui::pos2(strip.left(), prefix_top(&lead, strip.center().y));
    painter.galley(at, lead, ink);
    at
}

/// The top of a prefix's run, centred down a field whose middle is `centre_y`.
/// Every icon is drawn centred in its em box, so no glyph needs its own nudge.
fn prefix_top(lead: &egui::Galley, centre_y: f32) -> f32 {
    centre_y - lead.size().y / 2.0
}

impl Prefix {
    /// The prefix as styled text: the 38% grey, with size and family per variant.
    fn rich_text(self) -> egui::RichText {
        match self {
            Prefix::Text(t) => egui::RichText::new(t)
                .size(PREFIX_PT)
                .color(theme::text::faint()),
            Prefix::Icon(g) => theme::icon_text(g, PREFIX_ICON_PT, theme::text::faint()),
            Prefix::Label(t) => egui::RichText::new(t)
                .size(SUFFIX_PT)
                .color(theme::text::faint()),
        }
    }

    /// The prefix as a laid-out run, so [`value_field`] can reserve its width
    /// before the number and paint it back afterwards.
    fn galley(self, ui: &egui::Ui) -> std::sync::Arc<egui::Galley> {
        egui::WidgetText::from(self.rich_text()).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    }
}

/// How near a [`split_handle`]'s line a press must land to take it.
pub const SPLIT_REACH: f32 = 12.0;

/// A before-and-after split's divider: an accent line down `rect` at `x`, with a
/// narrow grip wearing three vertical dots at its middle. The compare's Swipe and
/// the trace card's preview draw it; only the line, within [`SPLIT_REACH`], drags.
pub fn split_handle(painter: &egui::Painter, rect: egui::Rect, x: f32) {
    let accent = color::accent();
    painter.line_segment(
        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
        egui::Stroke::new(2.0, accent),
    );
    let grip = egui::Rect::from_center_size(egui::pos2(x, rect.center().y), egui::vec2(12.0, 36.0));
    painter.rect_filled(grip, 6.0, accent);
    painter.text(
        grip.center(),
        egui::Align2::CENTER_CENTER,
        icon::DOTS_THREE_VERTICAL,
        theme::icon_font(14.0),
        egui::Color32::from_rgb(0x10, 0x13, 0x1b),
    );
}

/// How tall a scrolled list's top fade is ([`scroll_fade_top`]).
pub const SCROLL_FADE_H: f32 = 20.0;

/// Fade the top of a scrolled list's `viewport` into `ground`, the panel behind
/// it, so rows pass under a section header instead of being cut off. Comes in over
/// the first [`SCROLL_FADE_H`] of scroll, so an unscrolled first row is untouched.
pub fn scroll_fade_top(ui: &egui::Ui, viewport: egui::Rect, offset_y: f32, ground: egui::Color32) {
    let strength = (offset_y / SCROLL_FADE_H).clamp(0.0, 1.0);
    if strength <= 0.0 || viewport.height() <= 0.0 {
        return;
    }
    let band = egui::Rect::from_min_max(
        viewport.min,
        egui::pos2(
            viewport.max.x,
            viewport.top() + SCROLL_FADE_H.min(viewport.height()),
        ),
    );
    let solid = ground.gamma_multiply(strength);
    let clear = egui::Color32::TRANSPARENT;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(band.left_top(), solid);
    mesh.colored_vertex(band.right_top(), solid);
    mesh.colored_vertex(band.right_bottom(), clear);
    mesh.colored_vertex(band.left_bottom(), clear);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(egui::Shape::mesh(mesh));
}

/// Speed of an edge-triggered auto-scroll along one axis, in points per second:
/// `0.0` while `p` is clear of both bands, rising quadratically to ±`top` at the
/// edge of `min..max`. Shared by `canvas::autopan` and the layers tree; a
/// container narrower than two bands scrolls in neither direction.
pub fn edge_scroll_speed(p: f32, min: f32, max: f32, band: f32, top: f32) -> f32 {
    if max - min < 2.0 * band {
        return 0.0;
    }
    let past = if p > max - band {
        p - (max - band)
    } else if p < min + band {
        p - (min + band)
    } else {
        return 0.0;
    };
    let t = (past / band).clamp(-1.0, 1.0);
    t * t * t.signum() * top
}

/// Size of the chevron every dropdown closes with.
pub const COMBO_CHEVRON_PT: f32 = 13.0;
/// Its ink: `text::dim()`, the same as the menu heads' caret.
pub fn combo_chevron_ink() -> egui::Color32 {
    theme::text::dim()
}

/// Set up a dropdown's menu rows, inside a `ComboBox::show_ui` closure, and hold
/// the list [`DROPDOWN_GAP`] off its field.
///
/// `ComboBox` opens its list with `Popup::menu`'s zero gap and has no hook for
/// one, so the gap is a translation of the popup's layer, which moves its hit
/// tests with its ink: down when it opened below the field, up when above.
/// A menu that already sets its own `.gap` uses [`popup_rows`] instead.
pub fn menu_rows(ui: &mut egui::Ui) {
    popup_rows(ui);
    let layer = ui.layer_id();
    let above =
        egui::AreaState::load(ui.ctx(), layer.id).is_some_and(|s| s.pivot.y() == egui::Align::Max);
    let dy = if above { -DROPDOWN_GAP } else { DROPDOWN_GAP };
    ui.ctx().set_transform_layer(
        layer,
        egui::emath::TSTransform::from_translation(egui::vec2(0.0, dy)),
    );
}

/// The air between a dropdown's field and its open list.
pub const DROPDOWN_GAP: f32 = 2.0;

/// [`menu_rows`]' row setup alone, for a popup that sets its own `.gap`.
///
/// egui 0.35 sizes a `Button`'s margin as `button_padding − bg_stroke.width` and
/// puts the stroke back on a hovered row, so rows would shift on hover. Zeroing
/// `bg_stroke` on all four states makes both branches identical; the fill is the
/// hover cue. [`MENU_ROW_H`] is the height floor, and the padding is set here
/// because a `ComboBox` list is built from the global style, not from the scope
/// its face was drawn in.
pub fn popup_rows(ui: &mut egui::Ui) {
    ui.spacing_mut().interact_size.y = MENU_ROW_H;
    ui.spacing_mut().button_padding.y = 2.0;
    let w = &mut ui.visuals_mut().widgets;
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.bg_stroke = egui::Stroke::NONE;
        state.corner_radius = egui::CornerRadius::same(4);
    }
}

/// Height of every list row: a dropdown's (a floor over the natural height, set by
/// [`popup_rows`]) and a menu's that allocates its rows exactly (the context menus,
/// `docs/context-menus.md` §8, and the top bar's menus), glyph column or not.
pub const MENU_ROW_H: f32 = 24.0;

/// The air between a button and the menu it opens (the top bar's menus and the
/// inspector's popup menus), which `egui::Popup::menu` leaves at none.
pub const MENU_GAP: f32 = 6.0;

/// Like [`egui::Ui::add_enabled_ui`], but idempotent under a disabled parent:
/// `Ui::disable` multiplies opacity by 0.5 every call, so a nested gate would land
/// at 0.25 and look fainter than its neighbours.
pub fn disable_unless<R>(
    ui: &mut egui::Ui,
    enabled: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.add_enabled_ui(enabled || !ui.is_enabled(), add)
}

/// The arrow at the closed end of a `ComboBox` — a caret glyph at one fixed
/// colour, in place of egui's triangle, which goes white when open.
///
/// Every new `ComboBox` owes `.icon(combo_chevron)`: `Style` has no hook for it.
/// To audit, compare the counts of `ComboBox::from_id_salt` and that call over
/// `crates/ondin-app/src`; only the `"probe"` combos in `#[cfg(test)]` lack one.
pub fn combo_chevron(
    ui: &egui::Ui,
    rect: egui::Rect,
    _visuals: &egui::style::WidgetVisuals,
    _open: bool,
) {
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon::CARET_DOWN,
        theme::icon_font(COMBO_CHEVRON_PT),
        combo_chevron_ink(),
    );
}

/// One icon glyph as widget text, for a control whose closed state is an icon
/// and nothing else, in full-strength ink since no label sits beside it.
pub fn glyph_only(glyph: &'static str) -> egui::WidgetText {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        glyph,
        0.0,
        egui::TextFormat {
            font_id: theme::icon_font(16.0),
            color: theme::text::strong(),
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.into()
}

/// A dimmed icon glyph followed by a label, as one run of widget text.
///
/// For where a [`Prefix`] cannot reach: a `ComboBox`'s `selected_text` is a single
/// `WidgetText`.
pub fn glyph_and_text(glyph: &str, text: &str) -> egui::WidgetText {
    glyph_and_text_tinted(glyph, text, theme::text::faint())
}

/// [`glyph_and_text`] with the glyph in a colour of the caller's choosing.
///
/// The type family row wears `TEXT_AA` in [`theme::color::warn()`] when the family
/// cannot be had.
pub fn glyph_and_text_tinted(
    glyph: &str,
    text: &str,
    glyph_col: egui::Color32,
) -> egui::WidgetText {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        glyph,
        0.0,
        egui::TextFormat {
            font_id: theme::icon_font(CONTROL_GLYPH_PT),
            color: glyph_col,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.append(
        text,
        7.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(CONTROL_PT),
            color: color::text(),
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.into()
}

/// A numeric field with a dimmed prefix: a 28pt recessed row holding
/// a small label or glyph and the value beside it. The prefix is painted by hand
/// because `DragValue::prefix` renders it in the number's colour and font.
///
/// The whole row scrubs, the prefix included: the strip is outside the
/// `DragValue`'s box, so a second claimant senses the drag there, which is why
/// this function owns the number (`&mut`, [`Scrub`]). Focus is drawn on the row,
/// since a focused `TextEdit` frames itself, nesting a box in a box.
pub fn value_field<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    value: &mut N,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    value_field_suffixed(ui, size, prefix, None, value, scrub, build).0
}

/// A unit written at the right-hand end of a value field, in the prefix's grey.
///
/// Its own widget, not `DragValue::suffix`, which sits in the number's text run
/// and can be neither dimmed nor clicked.
#[derive(Clone, Copy)]
pub struct Suffix<'a> {
    pub text: &'a str,
    /// Whether clicking it means something. A clickable suffix gets the pointing
    /// hand and a hover tooltip; an inert one is a label.
    pub clickable: bool,
    pub tooltip: &'a str,
}

/// [`value_field`] with a unit at the right-hand end.
///
/// The second return value is whether the suffix was clicked. That converts the
/// value rather than editing it, so a caller commits it directly, not through the
/// drag valve.
pub fn value_field_suffixed<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    suffix: Option<Suffix<'_>>,
    value: &mut N,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> (egui::Response, bool) {
    let (resp, unit) = value_field_unit(ui, size, prefix, suffix, value, scrub, build);
    (resp, unit.is_some_and(|u| u.clicked()))
}

/// [`value_field_suffixed`], handing back the clickable unit's own response rather
/// than whether it was clicked — `None` when there is no unit or it is inert.
///
/// For a caller that opens something from the unit: a popup anchored on the
/// field's response would open at the far end of the field.
pub fn value_field_unit<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    suffix: Option<Suffix<'_>>,
    value: &mut N,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> (egui::Response, Option<egui::Response>) {
    let (resp, unit, _) =
        value_field_unit_marked(ui, size, prefix, suffix, None, value, scrub, build);
    (resp, unit)
}

/// [`value_field_unit`] with an override mark ([`FieldMark`]), or without one when
/// `mark` is `None`; the third return value is whether the mark's ↺ was clicked.
#[allow(clippy::too_many_arguments)]
pub fn value_field_unit_marked<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    suffix: Option<Suffix<'_>>,
    mark: Option<FieldMark<'_>>,
    value: &mut N,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> (egui::Response, Option<egui::Response>, bool) {
    let mut v = value.to_f64();
    let out = value_field_f64_marked(ui, size, prefix, suffix, None, mark, &mut v, scrub, build);
    // Only on a real change, so an `f32` field keeps no round-trip error.
    if v != value.to_f64() {
        *value = N::from_f64(v);
    }
    out
}

/// The alpha of the accent a [`value_field_metered`] bar is painted in: 16%, a
/// wash because the bar sits under the field's label and value.
const METER_ALPHA: u8 = 41;

/// Width of the tick marking a metered field's zero, in points: a hairline so a
/// neutral value, which draws no bar, does not read as an empty box.
const METER_TICK_W: f32 = 1.0;

/// The accent wash a metered field's bar and its zero tick share.
fn meter_wash() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        color::accent_400().r(),
        color::accent_400().g(),
        color::accent_400().b(),
        METER_ALPHA,
    )
}

/// A [`value_field`] with a bar painted across its ground, growing from the
/// centre, for the image adjustments. `signed` is clamped to `-1..=1`: negative
/// fills left, positive right. The bar sits between the ground and the content (a
/// bar drawn after the text would tint it); the value is right-aligned so it does
/// not read as a bar label.
pub fn value_field_metered(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    signed: f32,
    value: &mut f64,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    value_field_f64(
        ui,
        size,
        prefix,
        None,
        Some(signed),
        None,
        value,
        scrub,
        build,
    )
    .0
}

/// An overridden field's mark: the label at full brightness with a 4-pt dot beside
/// it, and with the pointer on the field the label's slot turns into ↺, whose
/// click is the reset. `tip` is the hover text naming the main's value.
#[derive(Clone, Copy)]
pub struct FieldMark<'a> {
    pub tip: &'a str,
}

/// [`value_field`] with an override mark, or without one when `mark` is `None`.
/// The second return value is whether the ↺ was clicked.
///
/// The ↺ is the prefix strip's own press: a click with no drag on the label slot.
pub fn value_field_marked(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    mark: Option<FieldMark<'_>>,
    value: &mut f64,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> (egui::Response, bool) {
    let (resp, _, reset) =
        value_field_f64_marked(ui, size, prefix, None, None, mark, value, scrub, build);
    (resp, reset)
}

/// Height of a [`badge_field`], the size badge's own pill height.
pub const BADGE_FIELD_H: f32 = 20.0;
/// Type size inside a badge pill, and the gap between its label and its value.
pub const BADGE_PT: f32 = CAPTION_PT;
const BADGE_FIELD_GAP: f32 = 5.0;
/// Padding inside a badge pill, matching `canvas::draw_size_badge`'s.
const BADGE_FIELD_PAD_X: f32 = 7.0;
/// How much of the badge's ink the label carries, a multiplier rather than a
/// second colour.
const BADGE_LABEL_ALPHA: f32 = 0.7;

/// Everything in a [`badge_field`] that is not the number: the padding either side,
/// the label, and the gap between label and value.
pub fn badge_field_chrome_w(ctx: &egui::Context, label: &str) -> f32 {
    let label_w = ctx.fonts_mut(|f| {
        f.layout_no_wrap(
            label.to_string(),
            egui::FontId::proportional(BADGE_PT),
            egui::Color32::WHITE,
        )
        .size()
        .x
    });
    BADGE_FIELD_PAD_X * 2.0 + label_w + BADGE_FIELD_GAP
}

/// A numeric field drawn as a canvas badge: the size badge's pill, with a dimmed
/// one-letter label and a value that scrubs, types and takes arithmetic. Upright
/// where the badge follows the shape's angle (an egui widget cannot be rotated).
/// `width` is the whole pill's and the caller's, not shrink-wrapped, so a pill
/// does not slide out from under the pointer mid-drag.
pub fn badge_field(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
    value: &mut f64,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    let lead = egui::WidgetText::from(
        egui::RichText::new(label)
            .size(BADGE_PT)
            .color(theme::color::BADGE_INK.gamma_multiply(BADGE_LABEL_ALPHA)),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    let size = egui::vec2(width, BADGE_FIELD_H);
    // Sensed as nothing: the two claimants below own every press (see `icon_button_padded`).
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::empty());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(4), theme::color::BADGE);

    let chrome = badge_field_chrome_w(ui.ctx(), label);
    let strip = egui::Rect::from_min_size(
        rect.min + egui::vec2(BADGE_FIELD_PAD_X, 0.0),
        egui::vec2(
            chrome - BADGE_FIELD_PAD_X * 2.0 - BADGE_FIELD_GAP,
            rect.height(),
        ),
    );
    let lead_resp = ui
        .interact(
            strip,
            ui.auto_id_with("badge-label-scrub"),
            // Not `click_and_drag()`, which is `FOCUSABLE` and adds a dead tab stop.
            egui::Sense::CLICK | egui::Sense::DRAG,
        )
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    let before = *value;
    let mut nudged = false;
    if lead_resp.dragged() {
        scrub_strip(ui.ctx(), &lead_resp, &scrub, value);
        nudged = true;
        wrap_scrub(ui.ctx());
    }

    let number = egui::Rect::from_min_max(
        egui::pos2(strip.right() + BADGE_FIELD_GAP, rect.top()),
        egui::pos2(rect.right() - BADGE_FIELD_PAD_X, rect.bottom()),
    );
    let mut resp = ui
        .scope_builder(egui::UiBuilder::new().max_rect(number), |ui| {
            // The pill is the ground, so the `DragValue`'s own faces go.
            strip_drag_value_chrome(ui);
            // Dark ink on a light pill; the panel's text colour would be unreadable.
            ui.visuals_mut().override_text_color = Some(theme::color::BADGE_INK);
            ui.spacing_mut().button_padding = egui::Vec2::ZERO;
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.spacing_mut().interact_size = number.size();
            let dv = build(
                egui::DragValue::new(value)
                    .speed(scrub.speed)
                    .range(scrub.range.clone())
                    // As in `value_field_f64`: egui would otherwise rewrite a stored
                    // out-of-range value every frame.
                    .clamp_existing_to_range(false)
                    .custom_parser(crate::expr::eval),
            );
            ui.add_sized(number.size(), scrub_speed_for(ui.ctx(), dv))
        })
        .inner;
    if resp.dragged() {
        wrap_scrub(ui.ctx());
    }
    if resp.dragged() || resp.drag_stopped() {
        let settled = scrub.settle(*value);
        if settled != *value {
            *value = settled;
        }
    }
    // Painted after the value, so a selection block cannot land on the label.
    ui.painter().galley(
        egui::pos2(strip.left(), prefix_top(&lead, rect.center().y)),
        lead,
        theme::color::BADGE_INK,
    );
    resp = resp.union(lead_resp);
    if nudged || *value != before {
        resp.mark_changed();
    }
    resp
}

#[allow(clippy::too_many_arguments)]
fn value_field_f64(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    suffix: Option<Suffix<'_>>,
    meter: Option<f32>,
    mark: Option<FieldMark<'_>>,
    value: &mut f64,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> (egui::Response, Option<egui::Response>) {
    let (resp, unit, _) =
        value_field_f64_marked(ui, size, prefix, suffix, meter, mark, value, scrub, build);
    (resp, unit)
}

/// [`value_field_f64`], also answering whether a marked field's ↺ was clicked
/// ([`FieldMark`]).
#[allow(clippy::too_many_arguments)]
fn value_field_f64_marked(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    suffix: Option<Suffix<'_>>,
    meter: Option<f32>,
    mark: Option<FieldMark<'_>>,
    value: &mut f64,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> (egui::Response, Option<egui::Response>, bool) {
    let mut reset = false;
    let scope = ui.scope(|ui| {
        field_row(ui, size, |ui| {
            // Two flat slots rather than one `Shape::Vec`, which would hide the bar
            // from `meter_tests`. The tick is first so the bar paints over it.
            let meter_slots = meter.map(|_| {
                (
                    ui.painter().add(egui::Shape::Noop),
                    ui.painter().add(egui::Shape::Noop),
                )
            });
            let lead = prefix.galley(ui);
            let inset = prefix_inset(&lead);

            strip_drag_value_chrome(ui);
            // The height is the content box: the hairline is already counted.
            let full = ui.available_width();
            let inner_h = size.y - FIELD_BORDER_H;
            // The prefix's width is reserved by advancing the cursor, not padding:
            // `button_padding` applies to both sides, which left dead space on the
            // right and let the resting face grow the field and the inspector.
            //
            // Not a `DragValue::prefix` atom: atoms are laid out by the resting
            // `Button` only, so the glyph would vanish on focus
            // (`focusing_a_value_field_outlines_the_row_and_leaves_the_value_put`).
            // The unit's strip at the far end is reserved the same way.
            let trail = suffix.map(|s| {
                // `PLACEHOLDER` defers the colour to paint time; an unset colour
                // becomes the style's white. Cut to the room the digits leave: a
                // long unit made the number's box negative, and egui panics on that.
                let room = (full - inset - UNIT_DIGITS_MIN - PREFIX_GAP + SUFFIX_BLEED).max(0.0);
                let galley = egui::WidgetText::from(
                    egui::RichText::new(s.text)
                        .size(SUFFIX_PT)
                        .color(egui::Color32::PLACEHOLDER),
                )
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    room,
                    egui::TextStyle::Body,
                );
                (s, galley)
            });
            let trail_w = trail
                .as_ref()
                .map(|(_, g)| (g.size().x - SUFFIX_BLEED + PREFIX_GAP).round())
                .unwrap_or(0.0);

            ui.spacing_mut().button_padding.x = 0.0;
            // Every gap here is measured by hand, so egui adds none.
            ui.spacing_mut().item_spacing.x = 0.0;
            // Floored: a field narrower than its own prefix would ask for a negative box.
            let digits_w = (full - inset - trail_w).max(0.0);
            ui.spacing_mut().interact_size = egui::vec2(digits_w, inner_h);

            // Sensed before the number is laid out, so its drag is already in the
            // value and the digits do not trail by a frame. Its own id, not the
            // `DragValue`'s (an id clash); `auto_id_with`, not `id().with`, because
            // `Ui::id` is shared by two fields in one `horizontal`
            // (`a_row_of_value_fields_has_no_id_clash`).
            let strip = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(inset, inner_h));
            if let (Some((tick_slot, slot)), Some(frac)) = (meter_slots, meter) {
                // The ink box: the content width plus both paddings, inside the border.
                let ink = egui::Rect::from_min_size(
                    ui.cursor().min - egui::vec2(FIELD_PAD_X, 0.0),
                    egui::vec2(full + FIELD_PAD_X * 2.0, inner_h),
                );
                let frac = frac.clamp(-1.0, 1.0);
                let mid = ink.center().x;
                let reach = mid + ink.width() / 2.0 * frac;
                let bar = egui::Rect::from_min_max(
                    egui::pos2(mid.min(reach), ink.top()),
                    egui::pos2(mid.max(reach), ink.bottom()),
                );
                // A pair of corners rounds only when the bar reaches that end.
                let r = 5;
                let end = |touching: bool| match touching {
                    true => r,
                    false => 0,
                };
                let corner = egui::CornerRadius {
                    nw: end(frac <= -1.0),
                    sw: end(frac <= -1.0),
                    ne: end(frac >= 1.0),
                    se: end(frac >= 1.0),
                };
                // The zero tick, under the bar so it does not cut a line through it.
                ui.painter().set(
                    tick_slot,
                    egui::epaint::RectShape::filled(
                        egui::Rect::from_min_size(
                            egui::pos2(mid - METER_TICK_W / 2.0, ink.top()),
                            egui::vec2(METER_TICK_W, ink.height()),
                        ),
                        egui::CornerRadius::ZERO,
                        meter_wash(),
                    ),
                );
                ui.painter().set(
                    slot,
                    egui::epaint::RectShape::filled(bar, corner, meter_wash()),
                );
            }
            let strip_id = ui.auto_id_with("prefix-scrub");
            // A scrub strip is no tab stop (the number is); a marked field's ↺ is
            // one, ahead of the number, so the keyboard can reset it too. It stays
            // one while focused, or the reset would drop the focus with the mark.
            let keyboard = ui.memory(|m| m.has_focus(strip_id));
            let lead_resp = ui.interact(
                strip,
                strip_id,
                match mark.is_some() || keyboard {
                    true => egui::Sense::CLICK | egui::Sense::DRAG | egui::Sense::FOCUSABLE,
                    false => egui::Sense::CLICK | egui::Sense::DRAG,
                },
            );
            // The plain arrow while the slot is a marked field's ↺, unless dragging.
            let cursor = match mark {
                Some(_) if !lead_resp.dragged() => egui::CursorIcon::Default,
                _ => egui::CursorIcon::ResizeHorizontal,
            };
            let lead_resp = lead_resp.on_hover_cursor(cursor);
            let lead_resp = match mark {
                Some(m) => {
                    if lead_resp.clicked() {
                        reset = true;
                    }
                    lead_resp.on_hover_text(m.tip)
                }
                None => lead_resp,
            };
            let mut nudged = false;
            if lead_resp.dragged() {
                scrub_strip(ui.ctx(), &lead_resp, &scrub, value);
                nudged = true;
                wrap_scrub(ui.ctx());
            }

            // `left_to_right` would centre the number; it sits beside the prefix, or
            // at the far end in a metered field.
            let main = match meter.is_some() {
                true => egui::Align::Max,
                false => egui::Align::Min,
            };
            let before = *value;
            let dv = build(
                egui::DragValue::new(value)
                    .speed(scrub.speed)
                    .range(scrub.range.clone())
                    // The range bounds what a gesture can reach, not what the document
                    // holds: egui's default rewrites a stored out-of-range value every
                    // frame and `mark_changed` below would report a user edit.
                    .clamp_existing_to_range(false)
                    // Set before `build`, so a caller's own parser wins.
                    .custom_parser(crate::expr::eval),
            );
            let dv = scrub_speed_for(ui.ctx(), dv);
            ui.add_space(inset);
            let mut resp = ui
                .allocate_ui_with_layout(
                    egui::vec2(digits_w, inner_h),
                    egui::Layout::left_to_right(egui::Align::Center).with_main_align(main),
                    |ui| ui.add(dv),
                )
                .inner;
            if resp.dragged() {
                wrap_scrub(ui.ctx());
            }
            // `settle` also re-clamps, since a rounded value can step outside a bound.
            if resp.dragged() || resp.drag_stopped() {
                let settled = scrub.settle(*value);
                if settled != *value {
                    *value = settled;
                }
            }
            // Read before the union, which answers for both.
            let reset_focused = lead_resp.has_focus();
            resp = resp.union(lead_resp);
            if nudged || *value != before {
                resp.mark_changed();
            }
            // Painted after the value, so a selection block cannot land on the label.
            let at = egui::pos2(strip.left(), prefix_top(&lead, resp.rect.center().y));
            match mark {
                None => {
                    ui.painter().galley(at, lead, theme::text::faint());
                }
                Some(_) if reset_focused || ui.rect_contains_pointer(strip.union(resp.rect)) => {
                    ui.painter().text(
                        strip.left_center() + egui::vec2(lead.size().x / 2.0, 0.0),
                        egui::Align2::CENTER_CENTER,
                        icon::ARROW_COUNTER_CLOCKWISE,
                        theme::icon_font(12.0),
                        theme::text::strong(),
                    );
                }
                Some(_) => {
                    let w = lead.size().x;
                    ui.painter().galley(at, lead, theme::text::strong());
                    override_dot(ui.painter(), at + egui::vec2(w + 1.5, 3.0));
                }
            }

            let mut unit = None;
            if let Some((s, galley)) = trail {
                // Allocated, not just painted into: `field_frame` shrink-wraps.
                ui.allocate_exact_size(egui::vec2(trail_w, inner_h), egui::Sense::empty());
                let band = egui::Rect::from_min_size(
                    egui::pos2(full_rect_right(ui) - trail_w, resp.rect.top()),
                    egui::vec2(trail_w, inner_h),
                );
                // Text that lights up, not a chip (`%` and `px` do not fill a box
                // alike); it hangs `SUFFIX_BLEED` into the field's padding.
                let text_right = band.right() + SUFFIX_BLEED;
                let hit_rect = egui::Rect::from_min_max(
                    egui::pos2(text_right - galley.size().x - SUFFIX_HIT_PAD, band.top()),
                    egui::pos2(text_right + SUFFIX_HIT_PAD, band.bottom()),
                );
                let mut ink = theme::text::faint();
                if s.clickable {
                    // Sensed before it is painted, so the ink is this frame's answer;
                    // wide, since two characters are small to aim at. A tab stop after
                    // the number: some units (a size's Fixed, Hug, Fill) have no other way in.
                    let hit = ui.interact(
                        hit_rect,
                        ui.auto_id_with("unit-suffix"),
                        egui::Sense::click(),
                    );
                    let hit = if s.tooltip.is_empty() {
                        hit
                    } else {
                        hit.on_hover_text(s.tooltip)
                    };
                    if hit.is_pointer_button_down_on() {
                        ink = color::text();
                    } else if hit.hovered() {
                        ink = theme::text::strong();
                    }
                    unit = Some(hit);
                }
                let at = egui::pos2(
                    text_right - galley.size().x,
                    band.center().y - galley.size().y / 2.0,
                );
                ui.painter().galley(at, galley, ink);
            }
            (resp, unit)
        })
    });

    // The row paints the typing edge (`framed_row`): the number's focus only, as a
    // focused ↺ is keyboard focus, which the ring shows.
    let (resp, unit) = scope.inner;
    (resp, unit, reset)
}

/// The right edge of the content box the unit sits against; `ui.cursor()` has
/// already advanced past the number.
fn full_rect_right(ui: &egui::Ui) -> f32 {
    ui.max_rect().right()
}

/// A flat icon button (tool-rail / top-bar chrome). Paints its own ground so it
/// can be transparent at rest, tinted on hover, and accent-filled when `active`.
/// `enabled: false` dims the glyph and suppresses hover feedback.
pub fn icon_button(
    ui: &mut egui::Ui,
    glyph: &str,
    box_size: f32,
    glyph_size: f32,
    active: bool,
    enabled: bool,
) -> egui::Response {
    icon_button_sized(
        ui,
        glyph,
        egui::vec2(box_size, box_size),
        glyph_size,
        active,
        enabled,
    )
}

/// [`icon_button`] on a box that need not be square (the Align row's are 24×22).
pub fn icon_button_sized(
    ui: &mut egui::Ui,
    glyph: &str,
    box_size: egui::Vec2,
    glyph_size: f32,
    active: bool,
    enabled: bool,
) -> egui::Response {
    icon_button_padded(
        ui,
        glyph,
        box_size,
        glyph_size,
        active,
        enabled,
        egui::Margin::ZERO,
    )
}

/// [`icon_button_sized`] whose hit target is `pad` larger than the box it paints,
/// on each side (see [`HEAD_ACTION_PAD`]).
///
/// One widget, not two: a clickable square inside a larger clickable one wins the
/// hit test wherever it lies, so the box senses nothing and the padded rect does.
pub fn icon_button_padded(
    ui: &mut egui::Ui,
    glyph: &str,
    box_size: egui::Vec2,
    glyph_size: f32,
    active: bool,
    enabled: bool,
    pad: egui::Margin,
) -> egui::Response {
    let sense = if pad == egui::Margin::ZERO {
        egui::Sense::click()
    } else {
        egui::Sense::empty()
    };
    let (rect, mut resp) = ui.allocate_exact_size(box_size, sense);
    if pad != egui::Margin::ZERO {
        resp = ui.interact(rect + pad, resp.id.with("padded"), egui::Sense::click());
    }
    let hovered = enabled && resp.hovered();
    let resting = resting_ink(ui, enabled);
    let r5 = egui::CornerRadius::same(5);
    let p = ui.painter();
    let fg = if active {
        color::accent_100()
    } else if hovered {
        color::text()
    } else {
        resting
    };
    if active {
        p.rect_filled(rect, r5, color::accent_800());
        p.rect_stroke(
            rect,
            r5,
            egui::Stroke::new(1.0, color::accent()),
            egui::StrokeKind::Inside,
        );
    } else if hovered {
        p.rect_filled(rect, r5, theme::color::hover());
    }
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(glyph_size),
        fg,
    );
    resp
}

/// What a [`field_button`] is saying about itself.
///
/// Three visual states rather than two `bool`s: [`FieldButton::Set`] is off while
/// holding a value, which `(active, enabled)` cannot express.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FieldButton {
    /// Off: the field ground and a muted glyph.
    #[default]
    Off,
    /// On — the accent wash and an accent border, for a toggle whose extra fields
    /// are showing; lighter than [`icon_button`]'s pressed state.
    On,
    /// Off, but not at its default: the ordinary ground and glyph with the on
    /// border, so a control can collapse without discarding its value (the stroke
    /// panel's sides selector).
    Set,
    /// Nothing to do: a dimmed glyph and no hover feedback. Callers must still
    /// ignore the click; this only draws the state.
    Disabled,
    /// [`FieldButton::On`] in red, on [`theme::color`]'s danger ramp: the
    /// confirming button of a card that destroys something. Quieter than
    /// [`FieldButton::Primary`], so a destructive confirm is never the loudest.
    Danger,
    /// A surface's one main action (a dialog's confirm, *New file*, *Publish*):
    /// the solid accent and [`theme::color::accent_ink`]. An action, where
    /// [`FieldButton::On`] is a state, so the two look different.
    Primary,
}

impl FieldButton {
    /// [`FieldButton::On`] or [`FieldButton::Off`] — the plain toggle case.
    pub fn on_if(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }

    /// [`FieldButton::Off`] or [`FieldButton::Disabled`] — an action that is
    /// sometimes unavailable.
    pub fn enabled_if(enabled: bool) -> Self {
        if enabled { Self::Off } else { Self::Disabled }
    }

    /// Whether a click on it means anything.
    pub fn is_enabled(self) -> bool {
        self != Self::Disabled
    }
}

/// The corner every [`button_face`] is rounded to, and every overlay painted on
/// one has to match.
pub const BUTTON_R: u8 = 5;

/// How far a [`focus_ring`] sits outside the widget it marks; the ring's corner is
/// [`BUTTON_R`] plus this gap, to stay concentric with the button.
const FOCUS_GAP: f32 = 2.0;

/// Where widget `id` was registered, this pass or the last, read without side
/// effects. Not `Context::read_response`: building a `Response` surrenders the
/// widget's focus on a frame whose click landed elsewhere, so reading the focused
/// widget that way drops a field focused by the click that opened it.
pub fn widget_rect(ctx: &egui::Context, id: egui::Id) -> Option<egui::WidgetRect> {
    ctx.viewport(|v| {
        v.this_pass
            .widgets
            .get(id)
            .or_else(|| v.prev_pass.widgets.get(id))
            .copied()
    })
}

/// Keyboard focus for one frame, like CSS `:focus-visible`. Made at the top of the
/// frame body, before any widget runs; dropped after the last, on every return
/// path. On drop it moves a `Tab` an open surface held ([`hold_tab`]) and paints
/// [`focus_ring`]. Every focusable control gets both without doing anything.
pub struct KeyboardFocus {
    ctx: egui::Context,
    /// This frame's `Tab`, `Some(true)` with `Shift`, unless the canvas keeps it.
    tab: Option<bool>,
    /// Whether `begin` moved it along an open surface.
    held: bool,
    /// Whether anything held the focus as the frame began.
    focused: bool,
}

impl KeyboardFocus {
    /// Updates the visibility latch (`Tab` turns it on, a pointer press or `Escape`
    /// off) and, when a surface held `Tab` last frame, moves this frame's `Tab`
    /// along it before any widget runs, taking it from egui. Moved here rather than
    /// after the widgets, so the control gaining the focus sees `gained_focus` and a
    /// field selects its text. `canvas_tab`: the canvas has a use for `Tab` (a text
    /// session, a path's points), which it keeps while no control holds the focus.
    pub fn begin(ctx: &egui::Context, canvas_tab: bool) -> Self {
        // Reads and writes one at a time: the context lock is not re-entrant.
        let pressed = ctx.input(|i| tab_pressed(&i.events));
        let keyed = ctx
            .data_mut(|d| d.remove_temp::<bool>(keyed_press_id()))
            .is_some();
        let pointed = !keyed && ctx.input(|i| i.pointer.any_pressed());
        let escaped = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let focused = ctx.memory(|m| m.focused());
        let tab = pressed.filter(|_| focused.is_some() || !canvas_tab);
        let open = ctx.data(|d| d.get_temp::<Option<egui::LayerId>>(open_id()).flatten());
        let held = match (tab, open) {
            (Some(_), Some(top)) => holds(ctx, top, focused),
            _ => false,
        };
        ctx.data_mut(|d| {
            let was = d.get_temp::<bool>(visible_id()).unwrap_or(false);
            d.insert_temp(visible_id(), (was || tab.is_some()) && !pointed && !escaped);
            d.remove::<Vec<egui::LayerId>>(holders_id());
        });
        if let (true, Some(top), Some(back)) = (held, open, tab) {
            ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            cycle_focus(ctx, top, back, Pass::Last);
        }
        Self {
            ctx: ctx.clone(),
            tab,
            held,
            focused: focused.is_some(),
        }
    }
}

/// Whether the open surface on `top` takes a `Tab`: while nothing is focused, the
/// focus is on it, or it is a modal. A card the person tabs past (Find, Colour
/// review) leaves a focus in the panels to walk them.
fn holds(ctx: &egui::Context, top: egui::LayerId, focused: Option<egui::Id>) -> bool {
    let modal = ctx.memory(|m| m.top_modal_layer()).is_some();
    let on = focused
        .and_then(|id| widget_rect(ctx, id))
        .map(|w| w.layer_id);
    modal || focused.is_none() || on == Some(top)
}

impl Drop for KeyboardFocus {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let ctx = &self.ctx;
        let holders = ctx
            .data(|d| d.get_temp::<Vec<egui::LayerId>>(holders_id()))
            .unwrap_or_default();
        // The higher `Order` wins; within one, the area on top of it, else the later.
        let top = holders
            .into_iter()
            .reduce(|a, b| match a.order.cmp(&b.order) {
                std::cmp::Ordering::Greater => a,
                std::cmp::Ordering::Less => b,
                std::cmp::Ordering::Equal => {
                    match ctx.memory(|m| m.areas().top_layer_id(a.order)) == Some(a) {
                        true => a,
                        false => b,
                    }
                }
            });
        let was_open = ctx.data(|d| d.get_temp::<Option<egui::LayerId>>(open_id()).flatten());
        ctx.data_mut(|d| d.insert_temp(open_id(), top));
        // A surface that opened this frame was not there for `begin` to move the
        // key along: egui kept it, and a move it made into the surface stands. Only
        // from no focus, as `holds` asks.
        if let (Some(top), Some(back), false, None, false) =
            (top, self.tab, self.held, was_open, self.focused)
        {
            cycle_focus(ctx, top, back, Pass::ThisUnlessIn);
        }
        focus_ring(ctx);
    }
}

fn visible_id() -> egui::Id {
    egui::Id::new("focus-visible")
}

/// The layers that called [`hold_tab`] this frame.
fn holders_id() -> egui::Id {
    egui::Id::new("focus-holders")
}

/// The topmost surface that held `Tab` at the end of last frame, if any.
fn open_id() -> egui::Id {
    egui::Id::new("focus-surface-open")
}

/// Keeps `Tab` inside the surface this `ui` draws: a popover, a dialog, a card
/// over the canvas. Call it anywhere in the surface's body, every frame it shows.
/// The first `Tab` after it opens focuses its first control and `Shift+Tab` its
/// last; from either end the focus wraps round, never leaving for the panels
/// behind it. When several are open, the topmost holds.
pub fn hold_tab(ui: &egui::Ui) {
    let layer = ui.layer_id();
    ui.ctx().data_mut(|d| {
        let holders = d.get_temp_mut_or_default::<Vec<egui::LayerId>>(holders_id());
        if !holders.contains(&layer) {
            holders.push(layer);
        }
    });
}

/// Which frame's controls [`cycle_focus`] walks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Last frame's, before this one's widgets run.
    Last,
    /// This frame's, after its widgets; a focus already on the layer stands.
    ThisUnlessIn,
}

/// Moves the focus one tab stop along `layer`'s controls, in the order they were
/// laid out, wrapping at either end. A focus outside the layer enters at the end
/// the key points to.
fn cycle_focus(ctx: &egui::Context, layer: egui::LayerId, back: bool, pass: Pass) {
    let mut stops: Vec<egui::Id> = Vec::new();
    ctx.viewport(|v| {
        let widgets = match pass {
            Pass::Last => &v.prev_pass.widgets,
            Pass::ThisUnlessIn => &v.this_pass.widgets,
        };
        for w in widgets.get_layer(layer) {
            if w.enabled && w.sense.is_focusable() && !stops.contains(&w.id) {
                stops.push(w.id);
            }
        }
    });
    if stops.is_empty() {
        return;
    }
    let focused = ctx.memory(|m| m.focused());
    let at = focused.and_then(|f| stops.iter().position(|&s| s == f));
    if pass == Pass::ThisUnlessIn && at.is_some() {
        return;
    }
    let n = stops.len();
    let next = match (at, back) {
        (Some(i), false) => (i + 1) % n,
        (Some(i), true) => (i + n - 1) % n,
        (None, false) => 0,
        (None, true) => n - 1,
    };
    ctx.memory_mut(|m| m.request_focus(stops[next]));
    // The ring comes a frame after the focus, and nothing else asks for that frame.
    ctx.request_repaint();
}

/// Whether the keyboard moved the focus last: on after `Tab`, off at any pointer
/// press. The pointer never shows [`focus_ring`].
pub fn focus_visible(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(visible_id()))
        .unwrap_or(false)
}

/// The accent hairline outside whatever the keyboard has focused, painted last on
/// the focused widget's own layer so it sits under any surface above it, and cut
/// where the widget is. Never on a text entry, whose [`typing_edge`] already says
/// where keys go, and not on the frame focus arrives: a number field becomes its
/// text entry only on its next frame.
fn focus_ring(ctx: &egui::Context) {
    if !focus_visible(ctx) {
        return;
    }
    let Some(id) = ctx.memory(|m| m.focused()) else {
        return;
    };
    if !ctx.memory(|m| m.had_focus_last_frame(id)) {
        ctx.request_repaint();
        return;
    }
    if egui::TextEdit::load_state(ctx, id).is_some() {
        return;
    }
    let Some(w) = ctx.viewport(|v| v.this_pass.widgets.get(id).copied()) else {
        return;
    };
    let ring = w.rect.expand(FOCUS_GAP);
    if !w.interact_rect.is_positive() {
        // Scrolled wholly out of view.
        return;
    }
    let clip = ring_clip(w.rect, w.interact_rect, ring.expand(1.0));
    ctx.layer_painter(w.layer_id)
        .with_clip_rect(clip)
        .rect_stroke(
            ring,
            egui::CornerRadius::same(BUTTON_R + FOCUS_GAP as u8),
            egui::Stroke::new(1.0, color::accent()),
            egui::StrokeKind::Outside,
        );
}

/// Where a focus ring may paint: all of `room` (the ring and its stroke), except on
/// a side where `shown`, the widget `rect` cut to its clip, was cut; there, at the
/// cut. Cutting to `shown` itself would hide a ring that lies outside it.
fn ring_clip(rect: egui::Rect, shown: egui::Rect, room: egui::Rect) -> egui::Rect {
    let side = |cut: bool, at: f32, open: f32| if cut { at } else { open };
    egui::Rect::from_min_max(
        egui::pos2(
            side(shown.min.x > rect.min.x, shown.min.x, room.min.x),
            side(shown.min.y > rect.min.y, shown.min.y, room.min.y),
        ),
        egui::pos2(
            side(shown.max.x < rect.max.x, shown.max.x, room.max.x),
            side(shown.max.y < rect.max.y, shown.max.y, room.max.y),
        ),
    )
}

/// Scrolls the scroll area whose content `ui` is so the control the keyboard just
/// focused is in view. Call it last inside the content of a scroll area that holds
/// tab stops; a focus outside the content, or one the pointer gave, is left alone.
pub fn keep_focus_in_view(ui: &egui::Ui) {
    let ctx = ui.ctx();
    if !focus_visible(ctx) {
        return;
    }
    let Some(id) = ctx.memory(|m| m.focused()) else {
        return;
    };
    let Some(w) = ctx.viewport(|v| v.this_pass.widgets.get(id).copied()) else {
        return;
    };
    // Once per focus: a person scrolling the focused control away is not undone.
    let key = ui.id().with("focus-scrolled");
    if ctx.data(|d| d.get_temp::<egui::Id>(key)) == Some(id) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(key, id));
    let ring = w.rect.expand(FOCUS_GAP + 1.0);
    if w.layer_id == ui.layer_id()
        && ui.min_rect().intersects(w.rect)
        && !ui.clip_rect().contains_rect(ring)
    {
        ui.scroll_to_rect(ring, None);
    }
}

/// The arrow keys a focused control reads for itself this frame: `-1`, `0` or `1`
/// on each axis (`y` down the screen), and whether `Shift` asks the coarse step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arrows {
    pub x: i8,
    pub y: i8,
    pub coarse: bool,
}

impl Arrows {
    /// Whether any arrow this control reads was pressed.
    pub fn any(self) -> bool {
        self.x != 0 || self.y != 0
    }
}

/// The arrows on the axes `x` and `y` name, for a control that moves a value with
/// them (a slider, a plane, a tree row's fold) while `resp` holds the focus. They
/// are taken from egui, which otherwise moves the focus to the neighbour that way;
/// the other axis still does. Bare or with `Shift`; any other modifier is not
/// read. Nothing without the focus.
pub fn arrow_keys(ui: &egui::Ui, resp: &egui::Response, x: bool, y: bool) -> Arrows {
    if !resp.has_focus() {
        return Arrows::default();
    }
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            resp.id,
            egui::EventFilter {
                horizontal_arrows: x,
                vertical_arrows: y,
                ..Default::default()
            },
        );
    });
    let keys = ui.input(|i| {
        let m = i.modifiers;
        if !(m.is_none() || m.shift_only()) {
            return Arrows::default();
        }
        let step = |less: egui::Key, more: egui::Key| {
            i8::from(i.key_pressed(more)) - i8::from(i.key_pressed(less))
        };
        Arrows {
            x: if x {
                step(egui::Key::ArrowLeft, egui::Key::ArrowRight)
            } else {
                0
            },
            y: if y {
                step(egui::Key::ArrowUp, egui::Key::ArrowDown)
            } else {
                0
            },
            coarse: m.shift,
        }
    });
    // An arrow read here is not also egui's: a focus that arrived this frame
    // carries no filter yet, and egui would move it on at the frame's end.
    if keys.any() {
        ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    }
    keys
}

/// Where the primary button is pressing, clicking or dragging `resp`, for a control
/// whose press is a value (a slider, a colour plane). A right-click, including the
/// one `Shift+F10` makes at the control's middle, is its menu and moves nothing.
pub fn primary_press_pos(ui: &egui::Ui, resp: &egui::Response) -> Option<egui::Pos2> {
    let primary = resp.clicked()
        || resp.dragged_by(egui::PointerButton::Primary)
        || resp.drag_stopped_by(egui::PointerButton::Primary)
        || resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
    primary.then(|| resp.interact_pointer_pos()).flatten()
}

/// Whether the focused control `resp` was asked to rename what it stands for, or
/// to type into the cell it is: `F2` or `Ctrl+R`, the keys rename has everywhere.
pub fn rename_key(ui: &egui::Ui, resp: &egui::Response) -> bool {
    resp.has_focus()
        && ui.input(|i| {
            i.modifiers.is_none() && i.key_pressed(egui::Key::F2)
                || i.modifiers.command_only() && i.key_pressed(egui::Key::R)
        })
}

/// Whether the focused control `resp` was asked for the list behind it (a split
/// button's choices, a tool group's members): `Alt+↓`, as a dropdown opens.
pub fn list_key(ui: &egui::Ui, resp: &egui::Response) -> bool {
    resp.has_focus()
        && ui.input(|i| {
            i.modifiers.alt && !i.modifiers.command && i.key_pressed(egui::Key::ArrowDown)
        })
}

/// `Shift+F10` on a focused control is a right-click on it, as on every desktop:
/// rewritten in the raw input (`App::raw_input_hook`) as a secondary press and
/// release at the middle of the control's visible part, so every right-click menu
/// opens from the keys. The pointer stays there until the mouse moves; the ring
/// stays up. A control scrolled out of view, or under another surface, is left.
pub fn menu_key_as_right_click(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let Some(at) = raw.events.iter().position(|e| {
        matches!(e, egui::Event::Key {
            key: egui::Key::F10,
            pressed: true,
            modifiers,
            ..
        } if modifiers.shift_only())
    }) else {
        return;
    };
    let Some(id) = ctx.memory(|m| m.focused()) else {
        return;
    };
    // A text entry keeps its keys; it has no menu of its own here.
    if egui::TextEdit::load_state(ctx, id).is_some() {
        return;
    }
    let Some(w) = widget_rect(ctx, id) else {
        return;
    };
    let pos = w.interact_rect.center();
    // The press goes to whatever is on top there, which must be the control.
    let on_top = match ctx.layer_id_at(pos) {
        Some(top) => top == w.layer_id,
        // No area there: the panels' own ground.
        None => w.layer_id.order == egui::Order::Background,
    };
    if !w.interact_rect.is_positive() || !on_top {
        return;
    }
    let press = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    raw.events.splice(
        at..=at,
        [egui::Event::PointerMoved(pos), press(true), press(false)],
    );
    ctx.data_mut(|d| d.insert_temp(keyed_press_id(), true));
}

/// Set for the frame [`menu_key_as_right_click`] made a press, which
/// [`KeyboardFocus::begin`] does not count as the pointer's.
fn keyed_press_id() -> egui::Id {
    egui::Id::new("focus-keyed-press")
}

/// This frame's `Tab` going down: `Some(true)` with `Shift`, `Some(false)` alone,
/// `None` without one or with another modifier.
fn tab_pressed(events: &[egui::Event]) -> Option<bool> {
    events.iter().find_map(|e| match e {
        egui::Event::Key {
            key: egui::Key::Tab,
            pressed: true,
            modifiers,
            ..
        } if !modifiers.any() => Some(false),
        egui::Event::Key {
            key: egui::Key::Tab,
            pressed: true,
            modifiers,
            ..
        } if modifiers.shift_only() => Some(true),
        _ => None,
    })
}

/// A square button that sits *in* a field row rather than floating on the card:
/// the recessed ground and hairline of [`field_frame`], sized to match the value
/// fields beside it (the flip and per-corner buttons, 28×28). The glyph's size is
/// the box's [`Scale`]'s, never the caller's.
pub fn field_button(
    ui: &mut egui::Ui,
    glyph: &str,
    box_size: f32,
    state: FieldButton,
) -> egui::Response {
    field_button_sized(ui, glyph, egui::vec2(box_size, box_size), state)
}

/// [`field_button`] on a box that need not be square — the Transform panel's row
/// of four buttons splits the card evenly between them.
pub fn field_button_sized(
    ui: &mut egui::Ui,
    glyph: &str,
    box_size: egui::Vec2,
    state: FieldButton,
) -> egui::Response {
    let (rect, resp, fg) = button_face(ui, box_size, state);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(Scale::of(box_size.y).glyph_pt()),
        fg,
    );
    resp
}

/// [`field_button_sized`] with an override mark ([`FieldMark`]); the second return
/// value is whether the mark's reset was clicked, in which case the button's own
/// click is not the caller's to act on. [`segmented_marked`]'s lit cell for a
/// toggle: a click on a marked toggle is the reset; a second click toggles.
pub fn field_button_marked(
    ui: &mut egui::Ui,
    glyph: &str,
    box_size: egui::Vec2,
    state: FieldButton,
    mark: Option<FieldMark<'_>>,
) -> (egui::Response, bool) {
    let glyph_size = Scale::of(box_size.y).glyph_pt();
    let (rect, resp, fg) = button_face(ui, box_size, state);
    let Some(m) = mark else {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            theme::icon_font(glyph_size),
            fg,
        );
        return (resp, false);
    };
    if resp.hovered() {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon::ARROW_COUNTER_CLOCKWISE,
            theme::icon_font(glyph_size),
            fg,
        );
    } else {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            theme::icon_font(glyph_size),
            fg,
        );
        override_dot(ui.painter(), rect.right_top() + egui::vec2(-4.0, 4.0));
    }
    let resp = resp.on_hover_text(m.tip);
    let reset = resp.clicked();
    (resp, reset)
}

/// [`field_button_sized`] with a **word** in it instead of a glyph.
///
/// For controls that must not be mistaken for an icon button elsewhere: the image
/// popover's mirror buttons flip the picture where the Transform panel's flip the
/// layer, and only words tell them apart.
pub fn label_button(
    ui: &mut egui::Ui,
    text: &str,
    box_size: egui::Vec2,
    state: FieldButton,
) -> egui::Response {
    let (rect, resp, fg) = button_face(ui, box_size, state);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(Scale::of(box_size.y).text_pt()),
        fg,
    );
    resp
}

/// [`label_button`] with a `note` after its word, in [`SMALL_PT`] mono and
/// [`theme::text::faint()`] — never the accent — sized by
/// [`label_button_noted_w`]: a set's value chip naming the state it is tied to.
pub fn label_button_noted(
    ui: &mut egui::Ui,
    text: &str,
    note: &str,
    state: FieldButton,
) -> egui::Response {
    let w = label_button_noted_w(ui.ctx(), text, note);
    let (rect, resp, fg) = button_face(ui, egui::vec2(w, CONTROL_H), state);
    let p = ui.painter();
    let word = p.text(
        egui::pos2(rect.left() + FIELD_PAD_X, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(CONTROL_PT),
        fg,
    );
    p.text(
        egui::pos2(word.right() + NOTE_GAP, rect.center().y),
        egui::Align2::LEFT_CENTER,
        note,
        egui::FontId::monospace(SMALL_PT),
        theme::text::faint(),
    );
    resp
}

/// The air between a [`label_button_noted`]'s word and its note.
const NOTE_GAP: f32 = 5.0;

/// The width a [`label_button_noted`] takes: its word, the gap, its note, and
/// [`FIELD_PAD_X`] each side; a plain [`label_button`]'s with no note.
pub fn label_button_noted_w(ctx: &egui::Context, text: &str, note: &str) -> f32 {
    let w = |s: &str, font: egui::FontId| {
        ctx.fonts_mut(|f| {
            f.layout_no_wrap(s.to_owned(), font, egui::Color32::PLACEHOLDER)
                .size()
                .x
        })
    };
    let note_w = match note.is_empty() {
        true => 0.0,
        false => NOTE_GAP + w(note, egui::FontId::monospace(SMALL_PT)),
    };
    (w(text, egui::FontId::proportional(CONTROL_PT)) + note_w + FIELD_PAD_X * 2.0).ceil()
}

/// The ink a resting control's content takes: [`theme::text::muted()`] while it can
/// be used, [`theme::text::disabled()`] when it cannot. `enabled` is ignored under an
/// already-disabled `Ui` ([`disable_unless`]): faded `muted()` is `text::disabled()`.
fn resting_ink(ui: &egui::Ui, enabled: bool) -> egui::Color32 {
    match enabled || !ui.is_enabled() {
        true => theme::text::muted(),
        false => theme::text::disabled(),
    }
}

/// The ground, hairline and hover treatment every field-row button shares, and
/// the colour its content should be drawn in. Its corner is [`BUTTON_R`], which a
/// caller painting over the face must match. `pub` for callers whose content is
/// neither a glyph nor a word.
pub fn button_face(
    ui: &mut egui::Ui,
    box_size: egui::Vec2,
    state: FieldButton,
) -> (egui::Rect, egui::Response, egui::Color32) {
    let (rect, resp) = ui.allocate_exact_size(box_size, egui::Sense::click());
    let hovered = state.is_enabled() && resp.hovered();
    let dead = resting_ink(ui, false);
    let r5 = egui::CornerRadius::same(BUTTON_R);
    let p = ui.painter();
    // `Set` takes an ordinary button's hover; the filled states lighten one ramp
    // step, ink fixed (the danger ramp has no twin of `accent_100()`).
    let (bg, fg) = match (state, hovered) {
        (FieldButton::On, false) => (color::accent_900(), color::accent_200()),
        (FieldButton::On, true) => (color::accent_800(), color::accent_200()),
        (FieldButton::Danger, false) => (color::danger_900(), color::danger_200()),
        (FieldButton::Danger, true) => (color::danger_800(), color::danger_200()),
        (FieldButton::Primary, false) => (color::accent(), color::accent_ink()),
        (FieldButton::Primary, true) => (color::accent_400(), color::accent_ink()),
        (_, true) => (color::hover(), color::text()),
        (FieldButton::Disabled, false) => (color::field(), dead),
        (_, false) => (color::field(), theme::text::muted()),
    };
    p.rect_filled(rect, r5, bg);
    p.rect_stroke(
        rect,
        r5,
        egui::Stroke::new(
            1.0,
            match state {
                FieldButton::On | FieldButton::Set => color::accent_700(),
                FieldButton::Danger => color::danger_700(),
                FieldButton::Primary => match hovered {
                    true => color::accent_400(),
                    false => color::accent(),
                },
                FieldButton::Off | FieldButton::Disabled => color::field_border(),
            },
        ),
        egui::StrokeKind::Inside,
    );
    (rect, resp, fg)
}

/// What a word on a [`label_button`] sized to it keeps clear of the button's
/// corners, per side: `settings::text_button`'s, and [`split_button`]'s word.
pub const LABEL_BUTTON_PAD_X: f32 = 14.0;

/// The caret end of a [`split_button`]: its width.
pub const SPLIT_CARET_W: f32 = 24.0;

/// What a [`split_button`] was asked.
pub struct SplitButton {
    /// The whole button, for its tooltip.
    pub resp: egui::Response,
    /// The word was clicked; meaningless while the action is unavailable.
    pub action: bool,
    /// The caret was clicked: open the other choices.
    pub caret: bool,
    /// Where the caret is, to hang its menu from.
    pub caret_rect: egui::Rect,
}

/// An action with a caret at its end that opens the other ways to do it —
/// Colour review's *Merge*. One [`button_face`] of `h`, [`FieldButton::Primary`]
/// while `enabled`, with a hairline before the caret; which end a click landed
/// on decides what it asks. Unavailable, the face is an ordinary button with the
/// word dimmed, and the caret still opens the choices. One tab stop: `Enter`
/// asks the action, `Alt+↓` the caret, as a dropdown opens.
pub fn split_button(ui: &mut egui::Ui, text: &str, h: f32, enabled: bool) -> SplitButton {
    let font = egui::FontId::proportional(Scale::of(h).text_pt());
    // The placeholder, so the word takes the face's ink when it is painted.
    let galley = ui
        .painter()
        .layout_no_wrap(text.into(), font, egui::Color32::PLACEHOLDER);
    let main_w = galley.size().x + LABEL_BUTTON_PAD_X * 2.0;
    let state = match enabled {
        true => FieldButton::Primary,
        false => FieldButton::Off,
    };
    let (rect, resp, ink) = button_face(ui, egui::vec2(main_w + SPLIT_CARET_W, h), state);
    let caret_rect = egui::Rect::from_min_max(
        egui::pos2(rect.right() - SPLIT_CARET_W, rect.top()),
        rect.max,
    );
    let on_caret = resp
        .interact_pointer_pos()
        .is_some_and(|p| caret_rect.contains(p));
    let word_ink = if enabled { ink } else { resting_ink(ui, false) };
    let p = ui.painter();
    p.vline(
        caret_rect.left(),
        rect.y_range().shrink(6.0),
        egui::Stroke::new(1.0, ink.gamma_multiply(0.35)),
    );
    let word = egui::Rect::from_min_max(rect.min, egui::pos2(caret_rect.left(), rect.bottom()));
    p.galley(word.center() - galley.size() / 2.0, galley, word_ink);
    p.text(
        caret_rect.center(),
        egui::Align2::CENTER_CENTER,
        icon::CARET_DOWN,
        theme::icon_font(12.0),
        ink,
    );
    let alt_down = list_key(ui, &resp);
    SplitButton {
        action: resp.clicked() && !on_caret,
        caret: resp.clicked() && on_caret || alt_down,
        resp,
        caret_rect,
    }
}

/// Between one labelled section and the next inside a popover: [`CARD_ROW_GAP`],
/// the pitch the inspector stacks every row at. `popup_rule`'s 5 above plus 4
/// below plus its hairline comes to the same.
pub const POPOVER_SECTION_GAP: f32 = CARD_ROW_GAP;

/// Between a small-caps eyebrow and the control it names: tighter than
/// [`CARD_ROW_GAP`], so the label does not float between what it names and the
/// section above.
pub const SECTION_LABEL_GAP: f32 = 5.0;

/// One labelled block: the eyebrow, then its controls at the card's own row pitch.
///
/// The label gap and the control gap are two numbers, so they cannot be one
/// `item_spacing.y`.
pub fn labelled<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        // Zero, so the eyebrow's gap is the `add_space` below alone.
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(eyebrow(label));
        ui.add_space(SECTION_LABEL_GAP);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = CARD_ROW_GAP;
            add(ui)
        })
        .inner
    })
    .inner
}

/// Between an [`action_button`]'s glyph and its word, the field gap the value
/// fields use at both ends of their number.
const ACTION_GAP: f32 = 7.0;
/// An [`action_button`]'s leading glyph.
const ACTION_GLYPH_PT: f32 = CONTROL_GLYPH_PT;
/// What an [`action_button`]'s pair keeps clear of its own corners, so an elided
/// name ends in a `…` with air after it.
const ACTION_PAD: f32 = 10.0;
/// An [`action_button`]'s word.
const ACTION_LABEL_PT: f32 = CONTROL_PT;

/// The width an [`action_button`] needs to show `label` without eliding it,
/// measured off the constants the button lays itself out with.
pub fn action_button_w(ctx: &egui::Context, label: &str) -> f32 {
    let w = ctx
        .fonts_mut(|f| {
            f.layout_no_wrap(
                label.to_owned(),
                egui::FontId::proportional(ACTION_LABEL_PT),
                theme::text::muted(),
            )
        })
        .size()
        .x;
    ACTION_PAD * 2.0 + ACTION_GLYPH_PT + ACTION_GAP + w
}

/// A wide button with a glyph and a word, centred as a pair (where [`toggle_row`]
/// is left-aligned, being a row in a list): the Export panel's actions and the
/// dashboard's sort control. The word is elided to fit, since it may be a layer's
/// name and `p.text` clips against nothing; a caller that would rather grow the
/// box asks [`action_button_w`].
pub fn action_button(
    ui: &mut egui::Ui,
    glyph: &str,
    label: &str,
    state: FieldButton,
    size: egui::Vec2,
) -> egui::Response {
    const GAP: f32 = ACTION_GAP;
    const GLYPH_PT: f32 = ACTION_GLYPH_PT;
    const PAD: f32 = ACTION_PAD;
    let (rect, resp, fg) = button_face(ui, size, state);
    let font = egui::FontId::proportional(ACTION_LABEL_PT);
    // Measured after eliding, so the pair is centred as one thing.
    let galley = ui.ctx().fonts_mut(|f| {
        let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), font, fg);
        job.wrap = egui::text::TextWrapping::truncate_at_width(
            (rect.width() - PAD * 2.0 - GLYPH_PT - GAP).max(0.0),
        );
        f.layout_job(job)
    });
    let total = GLYPH_PT + GAP + galley.size().x;
    let left = rect.center().x - total / 2.0;
    let p = ui.painter();
    p.text(
        egui::pos2(left + GLYPH_PT / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(GLYPH_PT),
        fg,
    );
    let top = rect.center().y - galley.size().y / 2.0;
    p.galley(egui::pos2(left + GLYPH_PT + GAP, top), galley, fg);
    resp
}

/// A full-width switch inside a popover: leading glyph, label, and a tick or a
/// dash pinned to the right (the Export settings card). [`button_face`]'s four
/// states rather than a checkbox, so the whole row is the target; off draws a dash,
/// since an empty right-hand column reads as unfinished loading.
pub fn toggle_row(
    ui: &mut egui::Ui,
    glyph: &str,
    label: &str,
    state: FieldButton,
    size: egui::Vec2,
) -> egui::Response {
    /// The inset, and the gap after the leading glyph.
    const PAD_X: f32 = 8.0;
    const GAP: f32 = 8.0;
    let scale = Scale::of(size.y);
    let glyph_pt = scale.glyph_pt();
    let (rect, resp, fg) = button_face(ui, size, state);
    let p = ui.painter();
    p.text(
        egui::pos2(rect.left() + PAD_X + glyph_pt / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(glyph_pt),
        fg,
    );
    p.text(
        egui::pos2(rect.left() + PAD_X + glyph_pt + GAP, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(scale.text_pt()),
        fg,
    );
    p.text(
        egui::pos2(rect.right() - PAD_X, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        match state {
            FieldButton::On => icon::CHECK,
            _ => icon::MINUS,
        },
        theme::icon_font(glyph_pt),
        fg,
    );
    resp
}

/// The drag grip on a reorderable row: a six-dot glyph, sensing drag rather than
/// click, pulled back into the field's left margin by 3px. `enabled: false` (a
/// list of one) draws the identical glyph and does not drag, so the row does not
/// jump as paints are added.
///
/// The hit target is the whole left end of the field, `row_h` tall and reaching
/// back through [`FIELD_PAD_X`] to the hairline. It stops at the glyph's right
/// edge and leaves the 9pt gap to the colour chip, which is used on every paint.
pub fn grip(ui: &mut egui::Ui, enabled: bool, row_h: f32) -> egui::Response {
    // Always allocated so the chip lands in the same place; it senses nothing,
    // the press is read from the wider rect below (see [`icon_button_padded`]).
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(11.0, 20.0), egui::Sense::empty());
    let resp = if enabled {
        let hit = egui::Rect::from_min_max(
            egui::pos2(
                rect.left() - FIELD_PAD_X - FIELD_BORDER_H / 2.0,
                rect.center().y - row_h / 2.0,
            ),
            egui::pos2(rect.right(), rect.center().y + row_h / 2.0),
        );
        // The bare flag: `Sense::drag()` adds `FOCUSABLE`, a tab stop no key can use.
        ui.interact(hit, resp.id.with("grab"), egui::Sense::DRAG)
    } else {
        resp
    };
    let live = enabled && (resp.hovered() || resp.dragged());
    let fg = if live {
        theme::text::muted()
    } else {
        theme::text::faint()
    };
    ui.painter().text(
        rect.center() - egui::vec2(3.0, 0.0),
        egui::Align2::CENTER_CENTER,
        icon::DOTS_SIX_VERTICAL,
        theme::icon_font(14.0),
        fg,
    );
    if live {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if enabled {
        return resp.on_hover_text("Drag to reorder");
    }
    resp
}

/// Whether a hairline `width` points wide wants its centre on a pixel centre
/// rather than a pixel edge, at this display scale.
///
/// An odd device width covers whole pixels only when centred on one, an even one
/// only on the boundary between two; the parity is `floor(width × ppp)`.
pub fn hairline_on_pixel_centre(ppp: f32, width: f32) -> bool {
    (width * ppp).floor() as i32 % 2 == 1
}

/// A hairline's coordinate snapped to the nearest grid position of the right
/// parity, so it covers whole physical pixels and stays inside its column
/// (within half a device pixel). Used by [`rule`], `inspector::popup_rule` and
/// `settings::section`.
///
/// The two arms are not interchangeable: `floor + 0.5` is the nearest pixel centre
/// and `round` the nearest edge, each up to a whole pixel out for the other's
/// parity. `rulers::snap_across_axis` rounds for both: guides have no column.
pub fn hairline_in_column(centre: f32, ppp: f32, width: f32) -> f32 {
    let device = centre * ppp;
    match hairline_on_pixel_centre(ppp, width) {
        true => (device.floor() + 0.5) / ppp,
        false => device.round() / ppp,
    }
}

/// The 1px rule the Align row uses to part its three groups, allocated at the
/// row's full height and painted 16px tall (a `Separator` would claim the whole
/// remaining width), snapped by [`hairline_in_column`].
pub fn rule(ui: &mut egui::Ui, row_h: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, row_h), egui::Sense::empty());
    let half = 8.0_f32.min(row_h * 0.5);
    let ppp = ui.ctx().pixels_per_point();
    let x = hairline_in_column(rect.center().x, ppp, 1.0);
    ui.painter().line_segment(
        [
            egui::pos2(x, rect.center().y - half),
            egui::pos2(x, rect.center().y + half),
        ],
        egui::Stroke::new(1.0, theme::color::text_a(26)),
    );
}

/// Paint a locked layer's filled lock ([`theme::icon::LOCK_SIMPLE_FILL`]) centred
/// on `center` and turned by `rot`, so a mark can follow a rotated frame. `size` is
/// the em size the glyph is set at.
pub fn paint_lock_filled(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    color: egui::Color32,
    rot: f32,
) {
    paint_glyph_centred(
        painter,
        theme::icon::LOCK_SIMPLE_FILL,
        center,
        size,
        color,
        rot,
    );
}

/// Whether `resp`'s primary click this pass is the second of a double-click:
/// within egui's `max_double_click_delay` and `max_click_dist` of the click
/// before it, and not itself after a double.
///
/// Not `Response::double_clicked`: egui 0.35 measures distance from the last click
/// only, so a click on one layer then a fast double-click on another counts 3.
/// Here the chain restarts wherever the pointer moved too far, and a third click on
/// one spot is single. The record is one per context; a click no reader asks of
/// does not enter the chain.
pub fn double_clicked(ui: &egui::Ui, resp: &egui::Response) -> bool {
    #[derive(Clone, Copy)]
    struct Last {
        time: f64,
        pos: egui::Pos2,
        double: bool,
        pass: u64,
    }
    if !resp.clicked_by(egui::PointerButton::Primary) {
        return false;
    }
    let Some(pos) = resp.interact_pointer_pos() else {
        return false;
    };
    let ctx = ui.ctx();
    // One accessor at a time — two nested take the context's lock twice.
    let pass = ctx.cumulative_pass_nr();
    let now = ctx.input(|i| i.time);
    let (delay, dist) = ctx.options(|o| {
        (
            o.input_options.max_double_click_delay,
            o.input_options.max_click_dist,
        )
    });
    let id = egui::Id::new("ondin-double-click");
    let last: Option<Last> = ctx.data(|d| d.get_temp(id));
    if let Some(l) = last
        && l.pass == pass
        && l.pos == pos
    {
        return l.double;
    }
    let double =
        last.is_some_and(|l| !l.double && now - l.time < delay && l.pos.distance(pos) < dist);
    ctx.data_mut(|d| {
        d.insert_temp(
            id,
            Last {
                time: now,
                pos,
                double,
                pass,
            },
        )
    });
    double
}

/// Paint a main component's filled hexagon ([`theme::icon::HEXAGON_FILL`]) centred
/// on `center`, where [`theme::icon::HEXAGON`] would sit at the same `size`.
pub fn paint_hexagon_filled(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    color: egui::Color32,
) {
    paint_glyph_centred(painter, theme::icon::HEXAGON_FILL, center, size, color, 0.0);
}

/// Set one icon glyph with the middle of its em box on `center`, turned by `rot`
/// about that point. Every icon is drawn centred in its em box, so its ink is
/// centred there too.
pub(crate) fn paint_glyph_centred(
    painter: &egui::Painter,
    glyph: &str,
    center: egui::Pos2,
    size: f32,
    color: egui::Color32,
    rot: f32,
) {
    let galley = painter.layout_no_wrap(glyph.to_owned(), theme::icon_font(size), color);
    let pos = center - egui::emath::Rot2::from_angle(rot) * (galley.size() / 2.0);
    painter.add(egui::epaint::TextShape::new(pos, galley, color).with_angle(rot));
}

/// The hairline round a [`segmented`] track, and the width the cells give up to it
/// on every side; subtracted in one place and painted in another.
const SEGMENT_BORDER: f32 = 1.0;

/// A segmented control: `n` equal cells in a recessed track, the selected one
/// raised. `paint` draws cell `i` into its rect (`selected` picks the contrast
/// tier), so cells can hold pictures. Returns the clicked index.
pub fn segmented(
    ui: &mut egui::Ui,
    width: f32,
    cell_h: f32,
    n: usize,
    selected: usize,
    paint: impl Fn(&egui::Painter, usize, egui::Rect, bool),
) -> Option<usize> {
    segmented_enabled(ui, width, cell_h, n, selected, |_| true, paint)
}

/// [`segmented`] with cells that can be **unavailable** rather than absent.
///
/// A disabled cell keeps its place and its label, takes no hover ground, no click
/// and no focus, so the row keeps its size (the picker's kind tabs). This is the
/// untipped form of [`segmented_tipped`].
pub fn segmented_enabled(
    ui: &mut egui::Ui,
    width: f32,
    cell_h: f32,
    n: usize,
    selected: usize,
    enabled: impl Fn(usize) -> bool,
    paint: impl Fn(&egui::Painter, usize, egui::Rect, bool),
) -> Option<usize> {
    segmented_tipped(ui, width, cell_h, n, selected, enabled, |_| "", paint)
}

/// [`segmented_enabled`] with a tooltip per cell: `tip(i)`, where an empty string
/// is none.
///
/// A disabled cell carries its tooltip too, saying why it cannot be picked.
#[allow(clippy::too_many_arguments)]
pub fn segmented_tipped(
    ui: &mut egui::Ui,
    width: f32,
    cell_h: f32,
    n: usize,
    selected: usize,
    enabled: impl Fn(usize) -> bool,
    tip: impl Fn(usize) -> &'static str,
    paint: impl Fn(&egui::Painter, usize, egui::Rect, bool),
) -> Option<usize> {
    segmented_marked(ui, width, cell_h, n, selected, enabled, tip, None, paint).0
}

/// [`segmented_tipped`] with an override mark ([`FieldMark`]); the second return
/// value is whether the mark's reset was clicked. The mark sits on the lit cell
/// (dot at rest, ↺ and the tooltip under the pointer); a click there is the reset.
/// Over a selection that disagrees no cell is lit: the dot sits at the track's top
/// right and the reset is the card header's.
#[allow(clippy::too_many_arguments)]
pub fn segmented_marked(
    ui: &mut egui::Ui,
    width: f32,
    cell_h: f32,
    n: usize,
    selected: usize,
    enabled: impl Fn(usize) -> bool,
    tip: impl Fn(usize) -> &'static str,
    mark: Option<FieldMark<'_>>,
    paint: impl Fn(&egui::Painter, usize, egui::Rect, bool),
) -> (Option<usize>, bool) {
    let (track, cell) = segment_track(ui, width, cell_h, n);
    let mut clicked = None;
    let mut reset = false;
    if mark.is_some() && selected >= n {
        override_dot(ui.painter(), track.right_top() + egui::vec2(-5.0, 5.0));
    }
    for i in 0..n {
        let rect = cell(i);
        let live = enabled(i);
        // A disabled cell senses hover only: `Sense::click()` is `FOCUSABLE`, a tab
        // stop that would swallow Return and Space.
        let resp = ui.interact(
            rect,
            ui.id()
                .with(("seg", i, n, track.min.x as i32, track.min.y as i32)),
            match live {
                true => egui::Sense::click(),
                false => egui::Sense::hover(),
            },
        );
        let on = i == selected;
        segment_ground(ui.painter(), rect, on, live && resp.hovered());
        match mark {
            Some(m) if on => {
                if resp.hovered() {
                    segment_glyph(ui.painter(), rect, icon::ARROW_COUNTER_CLOCKWISE, true);
                } else {
                    paint(ui.painter(), i, rect, on);
                    override_dot(ui.painter(), rect.right_top() + egui::vec2(-4.0, 4.0));
                }
                if resp.on_hover_text(m.tip).clicked() {
                    reset = true;
                }
                continue;
            }
            _ => paint(ui.painter(), i, rect, on),
        }
        let words = tip(i);
        let resp = if words.is_empty() {
            resp
        } else {
            resp.on_hover_text(words)
        };
        if live && resp.clicked() {
            clicked = Some(i);
        }
    }
    (clicked, reset)
}

/// A segmented track `width` wide around cells `cell_h` tall: allocates and paints
/// it, and returns it with the rect of each of its `n` cells.
fn segment_track(
    ui: &mut egui::Ui,
    width: f32,
    cell_h: f32,
    n: usize,
) -> (egui::Rect, impl Fn(usize) -> egui::Rect + use<>) {
    const PAD: f32 = 2.0;
    const GAP: f32 = 2.0;
    let (track, _) =
        ui.allocate_exact_size(egui::vec2(width, cell_h + PAD * 2.0), egui::Sense::empty());
    let r5 = egui::CornerRadius::same(5);
    let p = ui.painter();
    // The ground every control has: a track beside a field reads as one family.
    p.rect_filled(track, r5, color::field());
    p.rect_stroke(
        track,
        r5,
        egui::Stroke::new(SEGMENT_BORDER, color::field_border()),
        egui::StrokeKind::Inside,
    );
    // The hairline comes out of the cells: a 28pt control is `1 + 2 + 22 + 2 + 1`.
    let inner = track.shrink(PAD + SEGMENT_BORDER);
    let cell_w = (inner.width() - GAP * (n.saturating_sub(1)) as f32) / n.max(1) as f32;
    let cell = move |i: usize| {
        egui::Rect::from_min_size(
            egui::pos2(inner.min.x + i as f32 * (cell_w + GAP), inner.min.y),
            egui::vec2(cell_w, inner.height()),
        )
    };
    (track, cell)
}

/// A segmented cell's ground: raised when `on`, tinted under the pointer.
fn segment_ground(painter: &egui::Painter, rect: egui::Rect, on: bool, hovered: bool) {
    if let Some(fill) = list_row_fill(on, hovered) {
        painter.rect_filled(rect, egui::CornerRadius::same(LIST_ROW_RADIUS), fill);
    }
}

/// A centred icon glyph for a [`segmented`] cell — the icon twin of
/// [`segment_label`], for the controls whose choices are pictures (alignment).
pub fn segment_glyph(painter: &egui::Painter, rect: egui::Rect, glyph: &str, on: bool) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(15.0),
        if on {
            color::text()
        } else {
            theme::text::dim()
        },
    );
}

/// A [`segmented`] cell showing nothing but a dash: the mixed state, with no cell
/// raised, since blank would read as a bug and a raised first cell as a lie.
pub fn segment_mixed(painter: &egui::Painter, rect: egui::Rect) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "–",
        egui::FontId::proportional(SMALL_PT),
        theme::text::faint(),
    );
}

// --- the slider ------------------------------------------------------------

/// The row a [`slider`] allocates: the knob's diameter.
///
/// The knob's shadow is a `half`-radius disc one point lower, so a slider paints
/// `SLIDER_H + 1` of ink into the row it reserves, bleeding into the caller's row
/// gap (`a_sliders_shadow_leaves_the_row_by_exactly_one_point`). Unlike
/// `panels::picker`'s track, travel is clamped so the knob stays on the rail.
pub const SLIDER_H: f32 = 11.0;
/// The rail's thickness: a hairline track with a bead riding on it.
pub(crate) const SLIDER_RAIL_H: f32 = 3.0;

/// A value between two bounds: a hairline rail, filled in accent to the knob.
/// Hand-painted: this rail and knob are out of reach of `Spacing` and
/// `Visuals` without restyling every `egui::Slider`. The knob's centre travels
/// between `half..width - half` so a knob touching the end never reads as short.
/// Focused, `←`/`→` move it a hundredth of the range, a tenth with `Shift`.
pub fn slider(
    ui: &mut egui::Ui,
    width: f32,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
) -> egui::Response {
    let (rect, mut resp) =
        ui.allocate_exact_size(egui::vec2(width, SLIDER_H), egui::Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    let half = SLIDER_H / 2.0;
    let travel = (rect.width() - SLIDER_H).max(1.0);

    // Absolute: jumps to where it is pressed, then follows the pointer.
    if let Some(pos) = primary_press_pos(ui, &resp)
        && hi > lo
    {
        let t = f64::from(((pos.x - rect.left() - half) / travel).clamp(0.0, 1.0));
        let next = lo + t * (hi - lo);
        if next != *value {
            *value = next;
            resp.mark_changed();
        }
    }
    // Focused, ←/→ step a hundredth of the range, a tenth with `Shift`.
    let keys = arrow_keys(ui, &resp, true, false);
    if keys.any() && hi > lo {
        let step = (hi - lo) / if keys.coarse { 10.0 } else { 100.0 };
        let next = (*value + f64::from(keys.x) * step).clamp(lo, hi);
        if next != *value {
            *value = next;
            resp.mark_changed();
        }
    }

    let t = if hi > lo {
        (((*value - lo) / (hi - lo)).clamp(0.0, 1.0)) as f32
    } else {
        0.0
    };
    let knob_x = rect.left() + half + travel * t;
    let rail = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.center().y - SLIDER_RAIL_H / 2.0),
        egui::pos2(rect.right(), rect.center().y + SLIDER_RAIL_H / 2.0),
    );
    let r = egui::CornerRadius::same(SLIDER_RAIL_H as u8);
    let p = ui.painter();
    p.rect_filled(rail, r, theme::color::text_a(33));
    if knob_x > rail.left() {
        p.rect_filled(
            egui::Rect::from_min_max(rail.min, egui::pos2(knob_x, rail.max.y)),
            r,
            color::accent_600(),
        );
    }
    // One soft disc under the knob, as `box-shadow: 0 1px 3px rgba(0,0,0,.4)`.
    p.circle_filled(
        egui::pos2(knob_x, rect.center().y + 1.0),
        half,
        egui::Color32::from_black_alpha(60),
    );
    p.circle_filled(
        egui::pos2(knob_x, rect.center().y),
        half,
        // Brighter under the pointer in the dark theme; the light one's knob is
        // already white, so it tints instead.
        if resp.is_pointer_button_down_on() || resp.hovered() {
            color::by_theme(color::text(), color::accent_800())
        } else {
            color::on_accent()
        },
    );
    resp
}

// --- the switch ------------------------------------------------------------

/// The track a [`paint_switch`] paints, and so the space one claims in a row.
pub const SWITCH_SIZE: egui::Vec2 = egui::vec2(26.0, 16.0);
/// Empty ground the track keeps around its knob, on every side.
const SWITCH_PAD: f32 = 2.0;
/// How far a [`switch_row`]'s hover ground reaches past the row on each side.
const SWITCH_ROW_BLEED: f32 = 4.0;

/// A two-state toggle: a track with a knob at one end.
///
/// A rounded-rectangle knob in a rounded-rectangle track, not a pill: radii 5
/// outside, 3 inside. Painting, not a widget: the only caller wants it at the end
/// of a row whose whole width is the target (see [`switch_row`] and
/// [`icon_button_padded`]).
pub fn paint_switch(painter: &egui::Painter, rect: egui::Rect, on: bool, hot: bool) {
    let (track, knob) = match (on, hot) {
        (true, false) => (color::accent_600(), color::on_accent()),
        (true, true) => (color::accent(), color::on_accent()),
        (false, false) => (theme::color::text_a(33), theme::text::dim()),
        (false, true) => (theme::color::text_a(46), theme::text::muted()),
    };
    painter.rect_filled(rect, egui::CornerRadius::same(5), track);
    let d = rect.height() - SWITCH_PAD * 2.0;
    let x = if on {
        rect.right() - SWITCH_PAD - d
    } else {
        rect.left() + SWITCH_PAD
    };
    painter.rect_filled(
        egui::Rect::from_min_size(egui::pos2(x, rect.top() + SWITCH_PAD), egui::vec2(d, d)),
        egui::CornerRadius::same(3),
        knob,
    );
}

/// Type size of a [`switch_row`]'s label, shared with the typography panel's
/// n-way `cvXX` row, which sits in the same column.
pub const SWITCH_ROW_LABEL_PT: f32 = SMALL_PT;

/// The ink of a toggle-list row's label: [`theme::text::strong()`] when the row is
/// on, [`theme::text::muted()`] when it is off. A row that is not a switch still
/// owes the two tiers (see [`switch_row`]).
pub fn switch_row_ink(on: bool) -> egui::Color32 {
    if on {
        theme::text::strong()
    } else {
        theme::text::muted()
    }
}

/// A row of a toggle list: a label, and a [`paint_switch`] against the right margin.
///
/// Returns the row's response, so `clicked()` is the toggle and a hover tooltip
/// attaches to the row itself. The row is the target, not the switch (see
/// [`icon_button_padded`]). The label dims when the feature is off, as
/// [`menu_check`]'s does.
pub fn switch_row(ui: &mut egui::Ui, label: &str, on: bool, height: f32) -> egui::Response {
    switch_row_inner(ui, label, Some(on), height, false)
}

/// A [`switch_row`] whose value an instance overrides: the dot after the label.
/// No ↺, since the reset is the toggle itself.
pub fn switch_row_marked(
    ui: &mut egui::Ui,
    label: &str,
    on: bool,
    height: f32,
    marked: bool,
) -> egui::Response {
    switch_row_inner(ui, label, Some(on), height, marked)
}

/// A [`switch_row`] over a selection whose runs disagree: the word "Mixed" where
/// the switch would be.
///
/// The word rather than a third knob state: a knob painted at the first run's
/// value would read as a claim the next click makes true. The row stays live:
/// clicking is how a mixed range is resolved.
pub fn switch_row_mixed(ui: &mut egui::Ui, label: &str, height: f32) -> egui::Response {
    switch_row_inner(ui, label, None, height, false)
}

/// The one implementation behind [`switch_row`] and [`switch_row_mixed`].
///
/// `on: None` is the runs disagree. One function so the two rows of one column
/// cannot come apart.
fn switch_row_inner(
    ui: &mut egui::Ui,
    label: &str,
    on: Option<bool>,
    height: f32,
    marked: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if resp.hovered() {
        // Wider than the row: the label is flush with the content edge, so the
        // ground borrows the card's padding.
        ui.painter().rect_filled(
            rect.expand2(egui::vec2(SWITCH_ROW_BLEED, 0.0)),
            egui::CornerRadius::same(5),
            theme::color::text_a(12),
        );
    }
    let inner = rect;
    let words = ui.painter().text(
        egui::pos2(inner.left(), inner.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(SWITCH_ROW_LABEL_PT),
        // A mixed row takes the off tier: the feature is not on for the range.
        switch_row_ink(on.unwrap_or(false)),
    );
    if marked {
        override_dot(ui.painter(), words.right_top() + egui::vec2(4.0, 3.0));
    }
    match on {
        Some(on) => paint_switch(
            ui.painter(),
            egui::Rect::from_min_size(
                egui::pos2(
                    inner.right() - SWITCH_SIZE.x,
                    inner.center().y - SWITCH_SIZE.y / 2.0,
                ),
                SWITCH_SIZE,
            ),
            on,
            resp.hovered(),
        ),
        None => {
            ui.painter().text(
                egui::pos2(inner.right(), inner.center().y),
                egui::Align2::RIGHT_CENTER,
                MIXED_WORD,
                egui::FontId::proportional(SEGMENT_LABEL_PT),
                theme::text::faint(),
            );
        }
    }
    resp
}

/// What a control says when the selection disagrees with itself, one spelling so
/// no panel says "mixed" or "—" instead.
pub const MIXED_WORD: &str = "Mixed";

/// Type size of a [`segment_label`]. Named because a cell clips nothing, so
/// whether a label fits is a question a test can ask only with this number.
pub const SEGMENT_LABEL_PT: f32 = CAPTION_PT;

/// Centred text for a [`segmented`] cell.
pub fn segment_label(painter: &egui::Painter, rect: egui::Rect, text: &str, on: bool) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(SEGMENT_LABEL_PT),
        if on {
            color::text()
        } else {
            theme::text::dim()
        },
    );
}

/// A [`segment_label`] for a cell that is present but unavailable — see
/// [`segmented_enabled`]. [`theme::text::disabled()`], not the resting tier, or the
/// cell would look merely unselected.
pub fn segment_label_disabled(painter: &egui::Painter, rect: egui::Rect, text: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(SEGMENT_LABEL_PT),
        theme::text::disabled(),
    );
}

// --- colour painting -------------------------------------------------------

/// Paint the alpha checkerboard behind a partly transparent colour.
pub fn paint_checkerboard(painter: &egui::Painter, rect: egui::Rect, cell: f32) {
    painter.rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::WHITE);
    let dark = egui::Color32::from_gray(0xd8);
    let mut y = rect.min.y;
    let mut row = 0;
    while y < rect.max.y {
        let mut x = rect.min.x + if row % 2 == 0 { 0.0 } else { cell };
        while x < rect.max.x {
            let c = egui::Rect::from_min_max(
                egui::pos2(x, y),
                egui::pos2((x + cell).min(rect.max.x), (y + cell).min(rect.max.y)),
            );
            painter.rect_filled(c, egui::CornerRadius::ZERO, dark);
            x += cell * 2.0;
        }
        y += cell;
        row += 1;
    }
}

/// A colour chip painted straight onto `p`, in the Fill row's look ([`swatch`]):
/// a rounded square with [`swatch_hairline`] round it, the checkerboard under a
/// translucent colour only. For rows that draw their chip as paint; with no
/// ground to mask against (they sit on a hover wash) the checkerboard's own cells
/// are rounded at the corners instead.
pub fn paint_colour_chip(p: &egui::Painter, rect: egui::Rect, c: egui::Color32) {
    let r = (rect.width().min(rect.height()) * 0.25).round();
    if c.a() < 255 {
        let cell = rect.width() * 0.5;
        let r8 = r as u8;
        p.rect_filled(rect, r8, egui::Color32::WHITE);
        let dark = egui::Color32::from_gray(0xd8);
        // Two by two, the dark ones on the diagonal, rounded at their corner.
        let tl = egui::Rect::from_min_size(rect.min, egui::vec2(cell, cell));
        let br = egui::Rect::from_min_max(rect.min + egui::vec2(cell, cell), rect.max);
        p.rect_filled(
            tl,
            egui::CornerRadius {
                nw: r8,
                ..Default::default()
            },
            dark,
        );
        p.rect_filled(
            br,
            egui::CornerRadius {
                se: r8,
                ..Default::default()
            },
            dark,
        );
    }
    p.rect_filled(rect, r, c);
    p.rect_stroke(
        rect,
        r,
        egui::Stroke::new(1.0, swatch_hairline()),
        egui::StrokeKind::Inside,
    );
}

/// Paint a left-to-right ramp of `stops` across `rect` as a vertex-coloured
/// mesh. Square corners — see [`mask_corners`]. Offsets are made monotonic here: an
/// imported `<linearGradient>` with descending offsets arrives verbatim, and the
/// swatch must clamp as the canvas does (`ondin_core::image::monotonic_offset`).
pub fn paint_ramp(painter: &egui::Painter, rect: egui::Rect, stops: &[(f32, egui::Color32)]) {
    if stops.is_empty() {
        return;
    }
    if stops.len() == 1 {
        painter.rect_filled(rect, egui::CornerRadius::ZERO, stops[0].1);
        return;
    }
    // A column pair per stop, with the ends extended flat to the box edges so
    // stops that start past 0 (or end before 1) hold their colour to the edge.
    fn column(mesh: &mut egui::Mesh, rect: egui::Rect, x: f32, c: egui::Color32) -> u32 {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(x, rect.min.y), c);
        mesh.colored_vertex(egui::pos2(x, rect.max.y), c);
        i
    }
    let mut mesh = egui::Mesh::default();
    let mut prev = column(&mut mesh, rect, rect.min.x, stops[0].1);
    let mut hi = 0.0f32;
    for (off, c) in stops {
        let off = ondin_core::image::monotonic_offset(&mut hi, *off);
        let x = rect.min.x + rect.width() * off.clamp(0.0, 1.0);
        let cur = column(&mut mesh, rect, x, *c);
        mesh.add_triangle(prev, prev + 1, cur);
        mesh.add_triangle(prev + 1, cur, cur + 1);
        prev = cur;
    }
    let last = column(&mut mesh, rect, rect.max.x, stops[stops.len() - 1].1);
    mesh.add_triangle(prev, prev + 1, last);
    mesh.add_triangle(prev + 1, last, last + 1);
    painter.add(egui::Shape::mesh(mesh));
}

/// Paint the saturation/value plane for `hue` (0..1): white → hue across, and
/// down to black. Square corners — see [`mask_corners`].
pub fn paint_sv_plane(painter: &egui::Painter, rect: egui::Rect, hue: f32) {
    let pure = Hsv::new(hue, 1.0, 1.0, 1.0).to_egui();
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), egui::Color32::WHITE);
    mesh.colored_vertex(rect.right_top(), pure);
    mesh.colored_vertex(rect.left_bottom(), egui::Color32::BLACK);
    mesh.colored_vertex(rect.right_bottom(), egui::Color32::BLACK);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Trim the corners of `rect` back to `radius` by painting the leftover area in
/// `bg`: egui cannot round a vertex-coloured mesh, so the ramp and SV plane are
/// drawn square and masked. Emitted as one `Mesh`, not filled polygons: a corner
/// sliver is tangent to both its edges, and egui's anti-aliased fill mitres that
/// zero-degree vertex into spikes clear of the rect.
pub fn mask_corners(painter: &egui::Painter, rect: egui::Rect, radius: f32, bg: egui::Color32) {
    painter.add(egui::Shape::mesh(corner_mask(rect, radius, bg)));
}

/// The mask geometry [`mask_corners`] paints, split out so tests can check it.
fn corner_mask(rect: egui::Rect, radius: f32, bg: egui::Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5);
    if r <= 0.0 {
        return mesh;
    }
    // Corner point, and unit vectors along its two edges.
    let corners = [
        (rect.left_top(), egui::vec2(1.0, 0.0), egui::vec2(0.0, 1.0)),
        (
            rect.right_top(),
            egui::vec2(-1.0, 0.0),
            egui::vec2(0.0, 1.0),
        ),
        (
            rect.right_bottom(),
            egui::vec2(-1.0, 0.0),
            egui::vec2(0.0, -1.0),
        ),
        (
            rect.left_bottom(),
            egui::vec2(1.0, 0.0),
            egui::vec2(0.0, -1.0),
        ),
    ];
    const STEPS: usize = 8;
    for (corner, a, b) in corners {
        let centre = corner + a * r + b * r;
        // A fan from the square corner out to the arc.
        let apex = mesh.vertices.len() as u32;
        mesh.colored_vertex(corner, bg);
        for i in 0..=STEPS {
            let t = i as f32 / STEPS as f32 * std::f32::consts::FRAC_PI_2;
            // t = 0 lands on edge `a`, t = π/2 on edge `b`.
            mesh.colored_vertex(centre - b * (r * t.cos()) - a * (r * t.sin()), bg);
            if i > 0 {
                mesh.add_triangle(apex, apex + i as u32, apex + i as u32 + 1);
            }
        }
    }
    mesh
}

/// How a [`swatch`] should render.
pub enum Swatch<'a> {
    Solid(egui::Color32),
    /// A gradient's stops, previewed as a left-to-right ramp.
    Ramp(&'a [(f32, egui::Color32)]),
    /// The paints this chip stands for do not agree on a colour: the bare
    /// checkerboard with a dash on it, [`segment_mixed`]'s mark, since a 16px chip
    /// has no room for the word "Mixed". A plausible colour would read as a claim
    /// the next click makes true.
    Mixed,
    /// An image fill's thumbnail, cut to the chip's square by [`crate::thumbs`],
    /// over the checkerboard like the rest.
    ///
    /// `alpha` is the brush's own `ImageSampler::alpha` ([`panels::paint::image_alpha`]
    /// reads it). It rides on the variant because the texture is shared: two layers
    /// can carry one photograph at two opacities.
    ///
    /// [`panels::paint::image_alpha`]: crate::panels::paint::image_alpha
    Picture {
        id: egui::TextureId,
        alpha: f32,
    },
}

/// The hairline every colour chip wears, drawn over its rounded boundary after
/// the corners are masked. Always on, since a chip near the row's own ground is
/// otherwise invisible and a threshold would pop as a colour is scrubbed.
/// The far end from the theme's field: white at 76/255 in the dark theme, set by
/// the black chip (grey 76 against `color::field()`'s 47), and black at the same
/// alpha in the light one, set by the white chip.
pub fn swatch_hairline() -> egui::Color32 {
    theme::color::by_theme(
        egui::Color32::from_white_alpha(76),
        egui::Color32::from_black_alpha(76),
    )
}

/// A small colour chip: checkerboard, the paint, and [`swatch_hairline`] over the
/// rounded boundary, sensed for clicks. `bg` is the ground the chip sits on,
/// painted over the leftover corners ([`mask_corners`]); the wrong one shows
/// four notches.
pub fn swatch(
    ui: &mut egui::Ui,
    box_size: f32,
    bg: egui::Color32,
    paint: Swatch<'_>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(box_size, box_size), egui::Sense::click());
    let p = ui.painter();
    paint_checkerboard(p, rect, box_size * 0.5);
    match paint {
        Swatch::Solid(c) => {
            p.rect_filled(rect, egui::CornerRadius::ZERO, c);
        }
        Swatch::Ramp(stops) => paint_ramp(p, rect, stops),
        Swatch::Picture { id, alpha } => {
            let tint = egui::Color32::WHITE.gamma_multiply(alpha);
            p.image(id, rect, crate::thumbs::FULL_UV, tint);
        }
        // Dark ink, to read against the checkerboard's light squares.
        Swatch::Mixed => {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "–",
                egui::FontId::proportional(box_size * 0.8),
                egui::Color32::from_gray(0x50),
            );
        }
    }
    mask_corners(p, rect, 4.0, bg);
    // After the mask, or it bites pieces out of the stroke; `Inside` so the chip does not grow.
    p.rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, swatch_hairline()),
        egui::StrokeKind::Inside,
    );
    resp
}

/// Strip a `DragValue`'s own grounds and borders, so it floats on whatever the
/// caller has already drawn (see [`value_field_f64`], [`badge_field`],
/// [`bare_drag_value`]). Both its faces ground themselves: the resting `Button`
/// from `bg_fill`/`weak_bg_fill`/`bg_stroke`, the keyboard `TextEdit` from
/// `extreme_bg_color` and, focused, `selection.stroke`.
///
/// `selection.stroke` loses its width, not its colour: selected glyphs are
/// repainted in it, so `Stroke::NONE` would make them invisible. The selection
/// fill stays: it shows the field is being typed into.
fn strip_drag_value_chrome(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets;
    for ws in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        ws.bg_fill = egui::Color32::TRANSPARENT;
        ws.weak_bg_fill = egui::Color32::TRANSPARENT;
        ws.bg_stroke = egui::Stroke::NONE;
    }
    ui.visuals_mut().extreme_bg_color = egui::Color32::TRANSPARENT;
    let selected_ink = ui.visuals().selection.stroke.color;
    ui.visuals_mut().selection.stroke = egui::Stroke::new(0.0, selected_ink);
}

/// A `DragValue` with every ground and border of its own stripped off, so it
/// floats on whatever row it sits in.
///
/// The paint rows and the picker's hex row already are fields, and a second
/// bordered box for the opacity inside one reads as bolted on.
pub fn bare_drag_value(ui: &mut egui::Ui, value: egui::DragValue<'_>) -> egui::Response {
    bare_drag_value_parsed(ui, value, crate::expr::eval)
}

/// [`bare_drag_value`] reading typed text with `parser` rather than plain
/// arithmetic — the picker's channel fields, which also take a whole colour.
pub fn bare_drag_value_parsed<'a>(
    ui: &mut egui::Ui,
    value: egui::DragValue<'a>,
    parser: impl 'a + Fn(&str) -> Option<f64>,
) -> egui::Response {
    ui.scope(|ui| {
        strip_drag_value_chrome(ui);
        // No clamping of a stored value outside a caller's `.range(0.0..=100.0)`:
        // egui's default would rewrite it each frame and report a user edit. Both
        // go on last, the `DragValue` being finished.
        let value = scrub_speed_for(ui.ctx(), value)
            .custom_parser(parser)
            .clamp_existing_to_range(false);
        let resp = ui.add(value);
        if resp.dragged() {
            wrap_scrub(ui.ctx());
        }
        resp
    })
    .inner
}

// --- colour conversion -----------------------------------------------------

/// sRGB straight `peniko::Color` → egui `Color32`, for *painting only*.
///
/// One way on purpose. `Color32` is premultiplied 8-bit, so a colour that comes
/// back out of it has lost its RGB triple wherever alpha is low and the picker
/// would drift the hue. Colour *values* travel as [`Hsv`] instead.
pub fn color_to_egui(c: ondin_core::peniko::Color) -> egui::Color32 {
    let k = c.components;
    egui::Color32::from_rgba_unmultiplied(
        (k[0] * 255.0).round() as u8,
        (k[1] * 255.0).round() as u8,
        (k[2] * 255.0).round() as u8,
        (k[3] * 255.0).round() as u8,
    )
}

/// Straight-alpha HSV over sRGB values, the space the hex field agrees with.
///
/// Not egui's `Hsva`, which converts through linearised light and premultiplied
/// bytes: the round trip moves the hex and destroys the colour at low alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    /// Hue, 0..1.
    pub h: f32,
    /// Saturation, 0..1.
    pub s: f32,
    /// Value, 0..1.
    pub v: f32,
    /// Alpha, 0..1.
    pub a: f32,
}

impl Hsv {
    pub fn new(h: f32, s: f32, v: f32, a: f32) -> Self {
        Self { h, s, v, a }
    }

    pub fn from_color(c: ondin_core::peniko::Color) -> Self {
        let [r, g, b, a] = c.components;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d <= f32::EPSILON {
            0.0
        } else if max == r {
            ((g - b) / d).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        let s = if max <= f32::EPSILON { 0.0 } else { d / max };
        Self { h, s, v: max, a }
    }

    pub fn to_color(self) -> ondin_core::peniko::Color {
        let (h, s, v) = (
            self.h.rem_euclid(1.0) * 6.0,
            self.s.clamp(0.0, 1.0),
            self.v.clamp(0.0, 1.0),
        );
        let sector = h.floor();
        let f = h - sector;
        let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
        let [r, g, b] = match sector as u32 % 6 {
            0 => [v, t, p],
            1 => [q, v, p],
            2 => [p, v, t],
            3 => [p, q, v],
            4 => [t, p, v],
            _ => [v, p, q],
        };
        ondin_core::peniko::Color::new([r, g, b, self.a.clamp(0.0, 1.0)])
    }

    /// The colour as egui sees it, for painting a knob or a plane corner.
    pub fn to_egui(self) -> egui::Color32 {
        color_to_egui(self.to_color())
    }
}

/// `RRGGBB` for a colour swatch label.
pub fn hex_of(c: ondin_core::peniko::Color) -> String {
    let k = c.components;
    format!(
        "{:02X}{:02X}{:02X}",
        (k[0] * 255.0).round() as u8,
        (k[1] * 255.0).round() as u8,
        (k[2] * 255.0).round() as u8,
    )
}

/// `n` and its noun, with an `s` past one: *1 layer*, *3 layers*.
pub fn count(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}

/// `n` and its noun, for a noun whose plural is not an added `s`: *1 match*, *2 matches*.
pub fn count_as(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// How many characters a colour field accepts: room for any CSS colour anyone
/// pastes (`crate::colour_text`), since a field that refuses to hold one refuses
/// the paste outright.
pub const COLOUR_CHAR_LIMIT: usize = 64;

/// Whether a text field's edit has ended and is meant to be kept: the frame it
/// lost focus, unless `Escape` is what took the focus away.
///
/// egui does not revert a `TextEdit` on `Escape`; it hands back what was typed and
/// surrenders focus, so `resp.lost_focus()` alone would apply the edit. It reads
/// the key because `Escape` clears focus in `Memory::begin_pass`: only the frame
/// `lost_focus()` fires still has the press. Numeric fields use
/// `app::OndinApp::edit_valve` instead; this is a text field's share of that rule
/// (`inspector::paint_hex_field_tests::escape_abandons_a_typed_hex_instead_of_committing_it`).
pub fn defocus_commits(resp: &egui::Response) -> bool {
    resp.lost_focus() && !resp.ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

/// Select the whole of a hex field the moment it takes focus, so typing replaces
/// the colour (as `DragValue` does; a plain `TextEdit` drops a caret). Call with
/// the field's own response: stored after the field has run, it takes effect next
/// frame and overwrites the focusing click's caret.
pub fn select_all_on_focus(ui: &egui::Ui, resp: &egui::Response, text: &str) {
    if !resp.gained_focus() {
        return;
    }
    let mut state = egui::TextEdit::load_state(ui.ctx(), resp.id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::default(),
            egui::text::CCursor::new(text.chars().count()),
        )));
    state.store(ui.ctx(), resp.id);
}

/// The app's mark, `icons/icon-64.png`, compiled in for the top bars: 64 pixels
/// for a mark drawn at 26 points, so a 2× display still has artwork to sample.
static LOGO_PNG: &[u8] = include_bytes!("../../../icons/icon-64.png");

/// The app's mark, painted into `rect`, with a rounded hover fill behind it when
/// `lit`.
///
/// The texture is uploaded on the first call and kept in egui's memory, shared by
/// both screens.
pub(crate) fn logo(ui: &egui::Ui, rect: egui::Rect, lit: bool) {
    let id = egui::Id::new("ondin-logo");
    // Read into a local: two `Context` accessors nested deadlock (CLAUDE.md).
    let cached = ui.ctx().data(|d| d.get_temp::<egui::TextureHandle>(id));
    let texture = cached.unwrap_or_else(|| {
        let icon = eframe::icon_data::from_png_bytes(LOGO_PNG)
            .expect("the bundled logo is not a valid PNG");
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        );
        let options =
            egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
        let handle = ui.ctx().load_texture("ondin-logo", image, options);
        ui.ctx().data_mut(|d| d.insert_temp(id, handle.clone()));
        handle
    });
    if lit {
        ui.painter().rect_filled(
            rect.expand(3.0),
            egui::CornerRadius::same(7),
            color::hover(),
        );
    }
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter()
        .image(texture.id(), rect, uv, egui::Color32::WHITE);
}

#[cfg(test)]
mod pulse_tests {
    use super::{PULSE_HOLD, PULSE_RAMP, pulse_alpha};

    /// `pulse_alpha` runs whole, down to half, held, back to whole, held, repeating.
    #[test]
    fn the_pulse_dips_to_half_holds_and_comes_back() {
        let e = 0.01;
        assert_eq!(pulse_alpha(0.0), 1.0, "whole at the click");
        assert!(pulse_alpha(PULSE_RAMP * 0.5) < 1.0 && pulse_alpha(PULSE_RAMP * 0.5) > 0.5);
        assert_eq!(pulse_alpha(PULSE_RAMP + e), 0.5, "held at half");
        assert_eq!(pulse_alpha(PULSE_RAMP + PULSE_HOLD - e), 0.5);
        let back = 2.0 * PULSE_RAMP + PULSE_HOLD;
        assert!(pulse_alpha(back - PULSE_RAMP * 0.5) > 0.5, "coming back");
        assert_eq!(pulse_alpha(back + e), 1.0, "held whole");
        let period = 2.0 * (PULSE_RAMP + PULSE_HOLD);
        assert_eq!(pulse_alpha(period + PULSE_RAMP + e), 0.5, "and again");
    }
}

#[cfg(test)]
mod keyboard_focus_tests {
    use super::*;

    /// A panel of two buttons and, when `open`, a popover of three that holds
    /// `Tab`, drawn after the panel as the app draws its popovers.
    struct Fixture {
        ctx: egui::Context,
        panel: Vec<egui::Id>,
        popover: Vec<egui::Id>,
        /// The popover button that saw `gained_focus` on the last frame.
        gained: Option<egui::Id>,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl Fixture {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            Self {
                ctx,
                panel: Vec::new(),
                popover: Vec::new(),
                gained: None,
                shapes: Vec::new(),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>, open: bool, canvas_tab: bool) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let (mut panel, mut popover) = (Vec::new(), Vec::new());
            let mut gained = None;
            let out = self.ctx.run_ui(input, |ui| {
                let _focus = KeyboardFocus::begin(ui.ctx(), canvas_tab);
                for _ in 0..2 {
                    panel.push(field_button(ui, icon::X, 28.0, FieldButton::Off).id);
                }
                if open {
                    egui::Area::new(egui::Id::new("popover"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(egui::pos2(400.0, 100.0))
                        .show(ui.ctx(), |ui| {
                            hold_tab(ui);
                            for _ in 0..3 {
                                let resp = field_button(ui, icon::X, 28.0, FieldButton::Off);
                                if resp.gained_focus() {
                                    gained = Some(resp.id);
                                }
                                popover.push(resp.id);
                            }
                        });
                }
            });
            self.panel = panel;
            self.popover = popover;
            self.gained = gained;
            self.shapes = out.shapes;
        }

        fn idle(&mut self, open: bool) {
            for _ in 0..3 {
                self.frame(Vec::new(), open, false);
            }
        }

        fn focused(&self) -> Option<egui::Id> {
            self.ctx.memory(|m| m.focused())
        }

        fn rings(&self) -> usize {
            self.shapes
                .iter()
                .filter(|cs| {
                    matches!(&cs.shape, egui::Shape::Rect(r)
                        if r.stroke.color == color::accent()
                            && r.stroke_kind == egui::StrokeKind::Outside)
                })
                .count()
        }
    }

    fn tab(shift: bool) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: match shift {
                true => egui::Modifiers::SHIFT,
                false => egui::Modifiers::NONE,
            },
        }]
    }

    /// An open popover takes the first `Tab` to its first control, wraps from its
    /// last to its first and back, and never lets the focus reach the panel.
    /// flip: `KeyboardFocus::begin` no longer takes the `Tab` from egui → fails at
    /// "Tab walks it in order", egui and the hold each moving the focus.
    #[test]
    fn an_open_popover_holds_tab_among_its_own_controls() {
        let mut f = Fixture::new();
        f.idle(true);
        assert_eq!(f.focused(), None, "the fixture opens with nothing focused");

        f.frame(tab(false), true, false);
        f.idle(true);
        assert_eq!(
            f.focused(),
            Some(f.popover[0]),
            "the first Tab enters the popover"
        );

        for _ in 0..2 {
            f.frame(tab(false), true, false);
            f.idle(true);
        }
        assert_eq!(f.focused(), Some(f.popover[2]), "Tab walks it in order");
        f.frame(tab(false), true, false);
        f.idle(true);
        assert_eq!(
            f.focused(),
            Some(f.popover[0]),
            "the wrap: past the last, its first"
        );
        f.frame(tab(true), true, false);
        f.idle(true);
        assert_eq!(f.focused(), Some(f.popover[2]), "and Shift+Tab wraps back");
        assert_eq!(f.rings(), 1, "the keyboard's focus wears the ring");
    }

    /// A stop a held `Tab` lands on sees `gained_focus`, which a field selects its text on.
    /// flip: `begin` leaves the held move to the frame's end → fails, nothing gained it.
    #[test]
    fn a_held_tab_lands_with_gained_focus() {
        let mut f = Fixture::new();
        f.idle(true);
        f.frame(tab(false), true, false);
        f.idle(true);
        f.frame(tab(false), true, false);
        assert_eq!(
            f.focused(),
            Some(f.popover[1]),
            "the fixture: the second Tab moved it"
        );
        assert_eq!(f.gained, Some(f.popover[1]), "on the frame it arrived");
    }

    /// A focus in the panel keeps walking the panel past an open card (Find, Colour review).
    /// flip: `holds` answers yes for a focus off the surface → fails, the popover took it.
    #[test]
    fn a_focus_in_the_panel_walks_the_panel_past_an_open_card() {
        let mut f = Fixture::new();
        f.idle(true);
        f.ctx.memory_mut(|m| m.request_focus(f.panel[0]));
        f.idle(true);
        f.frame(tab(false), true, false);
        f.idle(true);
        assert_eq!(f.focused(), Some(f.panel[1]));
    }

    /// With no surface open, `Tab` is egui's own walk through the panel.
    #[test]
    fn without_a_surface_tab_walks_the_panel() {
        let mut f = Fixture::new();
        f.idle(false);
        f.frame(tab(false), false, false);
        f.idle(false);
        assert_eq!(f.focused(), Some(f.panel[0]));
        f.frame(tab(false), false, false);
        f.idle(false);
        assert_eq!(f.focused(), Some(f.panel[1]));
    }

    /// A text session's `Tab` under an open popover stays the canvas's while nothing is focused.
    /// flip: drop `canvas_tab` from `begin`'s filter → fails, the popover's first control focused.
    #[test]
    fn the_canvas_keeps_its_tab_while_nothing_is_focused() {
        let mut f = Fixture::new();
        f.idle(true);
        f.frame(tab(false), true, true);
        f.idle(true);
        let focused = f.focused();
        assert!(
            focused.is_none_or(|id| !f.popover.contains(&id)),
            "{focused:?}"
        );
    }

    /// A pointer press hides the ring, and a click never focuses a button.
    #[test]
    fn the_pointer_never_shows_the_ring() {
        let mut f = Fixture::new();
        f.idle(true);
        f.frame(tab(false), true, false);
        f.idle(true);
        assert_eq!(f.rings(), 1, "the fixture shows a ring after Tab");
        let at = widget_rect(&f.ctx, f.popover[1])
            .expect("drawn")
            .rect
            .center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        f.frame(vec![egui::Event::PointerMoved(at)], true, false);
        f.frame(vec![press(true)], true, false);
        f.frame(vec![press(false)], true, false);
        f.idle(true);
        assert_eq!(f.rings(), 0, "the press put the ring away");
        assert_ne!(
            f.focused(),
            Some(f.popover[1]),
            "and the click did not focus"
        );
    }

    /// A marked field's ↺ is the stop before its number, and `Enter` on it resets
    /// as a click does; an unmarked field's strip is no stop at all.
    #[test]
    fn the_keyboard_reaches_a_marked_fields_reset() {
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        // Whether the ↺ fired, and whether the digits hold the focus.
        let run = |marked: bool| {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let frame = |events: Vec<egui::Event>| {
                let mut out = (false, false);
                let _ = ctx.run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let mark = marked.then_some(FieldMark { tip: "Reset" });
                        let mut v = 4.0;
                        let size = egui::vec2(120.0, CONTROL_H);
                        let (resp, reset) = value_field_marked(
                            ui,
                            size,
                            Prefix::Text("X"),
                            mark,
                            &mut v,
                            Scrub::whole(0.5),
                            |d| d,
                        );
                        let id = ui.ctx().memory(|m| m.focused());
                        let digits =
                            id.is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
                        out = (reset, digits && resp.has_focus());
                    },
                );
                out
            };
            frame(Vec::new());
            frame(vec![key(egui::Key::Tab)]);
            let landed = frame(Vec::new());
            let pressed = frame(vec![key(egui::Key::Enter)]);
            (landed, pressed, ctx.memory(|m| m.focused()).is_some())
        };

        let ((_, digits), (reset, _), focused) = run(true);
        assert!(
            focused && !digits,
            "marked: Tab lands on the ↺, not in the digits"
        );
        assert!(reset, "Enter on the ↺ resets");

        let ((_, digits), _, _) = run(false);
        assert!(digits, "unmarked: Tab goes straight to the digits");
    }

    /// A value field being typed in wears one typing edge, the row's.
    /// flip: `value_field_f64_marked` paints its own `typing_edge` again → fails at "one".
    #[test]
    fn a_typed_value_field_wears_one_typing_edge() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let frame = |events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                events,
                ..Default::default()
            };
            ctx.run_ui(raw, |ui| {
                let mut v = 4.0;
                let size = egui::vec2(120.0, CONTROL_H);
                let _ = value_field(
                    ui,
                    size,
                    Prefix::Text("X"),
                    &mut v,
                    Scrub::whole(0.5),
                    |d| d,
                );
            })
        };
        frame(Vec::new());
        frame(vec![egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        let out = frame(Vec::new());
        let mut edges = 0;
        let mut count = |s: &egui::Shape| {
            if let egui::Shape::Rect(r) = s
                && r.stroke.color == color::accent_700()
            {
                edges += 1;
            }
        };
        for c in &out.shapes {
            match &c.shape {
                egui::Shape::Vec(v) => v.iter().for_each(&mut count),
                s => count(s),
            }
        }
        assert_eq!(edges, 1, "one");
    }

    /// A focused slider steps on `←`/`→` (a hundredth, a tenth with `Shift`) and keeps the focus.
    #[test]
    fn a_focused_slider_steps_on_the_arrows_and_keeps_the_focus() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut value = 500.0_f64;
        let frame = |value: &mut f64, events: Vec<egui::Event>, shift: bool| {
            let mut id = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    modifiers: match shift {
                        true => egui::Modifiers::SHIFT,
                        false => egui::Modifiers::NONE,
                    },
                    ..Default::default()
                },
                |ui| {
                    let resp = ui.horizontal(|ui| {
                        let resp = slider(ui, 200.0, value, 100.0..=900.0);
                        // A neighbour to the right, for egui's arrow walk to reach.
                        let _ = field_button(ui, icon::X, 28.0, FieldButton::Off);
                        resp
                    });
                    id = Some(resp.inner.id);
                },
            );
            id.expect("drawn")
        };
        let right = |shift: bool| egui::Event::Key {
            key: egui::Key::ArrowRight,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: match shift {
                true => egui::Modifiers::SHIFT,
                false => egui::Modifiers::NONE,
            },
        };
        let slider_id = frame(&mut value, Vec::new(), false);
        ctx.memory_mut(|m| m.request_focus(slider_id));
        frame(&mut value, Vec::new(), false);
        frame(&mut value, vec![right(false)], false);
        frame(&mut value, vec![right(true)], true);
        frame(&mut value, Vec::new(), false);
        assert_eq!(value, 588.0, "8 for →, 80 for Shift+→ over a range of 800");
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(slider_id),
            "the arrows stayed its"
        );
    }

    /// `Shift+F10` on a focused slider is a right-click, which moves no value.
    /// flip: `primary_press_pos` returns any button's position → fails at "unmoved".
    #[test]
    fn shift_f10_on_a_focused_slider_leaves_its_value() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut value = 100.0_f64;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let frame = |value: &mut f64, mut raw: egui::RawInput| {
            raw.screen_rect = Some(screen);
            let mut out = None;
            let _ = ctx.run_ui(raw, |ui| {
                let resp = slider(ui, 200.0, value, 100.0..=900.0);
                out = Some((resp.id, resp.secondary_clicked()));
            });
            out.expect("drawn")
        };
        let (id, _) = frame(&mut value, Default::default());
        ctx.memory_mut(|m| m.request_focus(id));
        frame(&mut value, Default::default());
        let mut raw = egui::RawInput {
            modifiers: egui::Modifiers::SHIFT,
            events: vec![egui::Event::Key {
                key: egui::Key::F10,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
            ..Default::default()
        };
        menu_key_as_right_click(&ctx, &mut raw);
        let (_, right) = frame(&mut value, raw);
        assert!(right, "the key reached the slider as a right-click");
        assert_eq!(value, 100.0, "unmoved");
    }

    /// `Alt+↓` on a focused split button opens its choices, as its caret does; `↓`
    /// alone does not.
    /// flip: `split_button` dropping `|| alt_down` → fails at "Alt+↓ opens".
    #[test]
    fn alt_down_on_a_focused_split_button_opens_its_choices() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let frame = |events: Vec<egui::Event>, alt: bool| {
            let mut out = None;
            let raw = egui::RawInput {
                events,
                modifiers: match alt {
                    true => egui::Modifiers::ALT,
                    false => egui::Modifiers::NONE,
                },
                ..Default::default()
            };
            let _ = ctx.run_ui(raw, |ui| {
                let b = split_button(ui, "Merge", CONTROL_H, true);
                out = Some((b.resp.id, b.caret));
            });
            out.expect("drawn")
        };
        let down = |alt: bool| egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: match alt {
                true => egui::Modifiers::ALT,
                false => egui::Modifiers::NONE,
            },
        };
        let (id, _) = frame(Vec::new(), false);
        ctx.memory_mut(|m| m.request_focus(id));
        frame(Vec::new(), false);
        assert!(!frame(vec![down(false)], false).1, "↓ alone does not");
        ctx.memory_mut(|m| m.request_focus(id));
        frame(Vec::new(), false);
        assert!(frame(vec![down(true)], true).1, "Alt+↓ opens");
    }

    /// A scroll area of tab stops brings the control `Tab` reaches into view, once:
    /// scrolled away again, the focused control is left where the person put it.
    /// flip: `keep_focus_in_view` without its once-per-focus check → fails at "left
    /// where it was scrolled".
    #[test]
    fn a_tabbed_to_control_is_scrolled_into_view_once() {
        let mut f = Fixture::new();
        let frame = |f: &mut Fixture, events: Vec<egui::Event>, offset: Option<f32>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut at = 0.0;
            let _ = f.ctx.run_ui(input, |ui| {
                let _focus = KeyboardFocus::begin(ui.ctx(), false);
                let mut area = egui::ScrollArea::vertical().max_height(100.0);
                if let Some(y) = offset {
                    area = area.vertical_scroll_offset(y);
                }
                at = area
                    .show(ui, |ui| {
                        for _ in 0..12 {
                            let _ = field_button(ui, icon::X, 28.0, FieldButton::Off);
                        }
                        keep_focus_in_view(ui);
                    })
                    .state
                    .offset
                    .y;
            });
            at
        };
        frame(&mut f, Vec::new(), None);
        for _ in 0..12 {
            frame(&mut f, tab(false), None);
            frame(&mut f, Vec::new(), None);
        }
        let moved = frame(&mut f, Vec::new(), None);
        assert!(f.focused().is_some(), "the fixture: Tab reached the list");
        assert!(moved > 200.0, "scrolled to the last: {moved}");
        frame(&mut f, Vec::new(), Some(0.0));
        frame(&mut f, Vec::new(), None);
        let back = frame(&mut f, Vec::new(), None);
        assert_eq!(back, 0.0, "left where it was scrolled");
    }

    /// `Shift+F10` on a focused button is a right-click on it, and the ring stays.
    /// flip: `menu_key_as_right_click` returns before the splice → fails at "a
    /// right-click".
    #[test]
    fn shift_f10_right_clicks_the_focused_control() {
        let mut f = Fixture::new();
        f.idle(true);
        f.frame(tab(false), true, false);
        f.idle(true);
        assert_eq!(
            f.focused(),
            Some(f.popover[0]),
            "the fixture focuses a button"
        );
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            modifiers: egui::Modifiers::SHIFT,
            events: vec![egui::Event::Key {
                key: egui::Key::F10,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
            ..Default::default()
        };
        menu_key_as_right_click(&f.ctx, &mut raw);
        let mut right = false;
        let out = f.ctx.run_ui(raw, |ui| {
            let _focus = KeyboardFocus::begin(ui.ctx(), false);
            egui::Area::new(egui::Id::new("popover"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(400.0, 100.0))
                .show(ui.ctx(), |ui| {
                    hold_tab(ui);
                    right = field_button(ui, icon::X, 28.0, FieldButton::Off).secondary_clicked();
                    for _ in 0..2 {
                        let _ = field_button(ui, icon::X, 28.0, FieldButton::Off);
                    }
                });
        });
        f.shapes = out.shapes;
        assert!(right, "a right-click on the focused button");
        assert!(
            focus_visible(&f.ctx),
            "and the keyed press left the ring up"
        );
    }
}

#[cfg(test)]
mod prefix_tests {
    use super::*;

    /// The rows a one-glyph run inks, top and bottom, in points within the run,
    /// read from the font atlas rather than the glyph's padded quad.
    fn ink_rows(ctx: &egui::Context, lead: &egui::Galley) -> (f32, f32) {
        let img = ctx.fonts(|f| f.image());
        let w = img.size[0];
        let row = &lead.rows[0];
        let vs = &row.visuals.mesh.vertices;
        let fold = |f: fn(&egui::epaint::Vertex) -> f32| {
            vs.iter()
                .map(f)
                .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)))
        };
        let (p0, p1) = fold(|v| v.pos.y);
        let (u0, u1) = fold(|v| v.uv.x);
        let (v0, v1) = fold(|v| v.uv.y);
        let (sx, sy) = if v1 <= 1.0 {
            (img.size[0] as f32, img.size[1] as f32)
        } else {
            (1.0, 1.0)
        };
        let (tx0, tx1) = ((u0 * sx) as usize, (u1 * sx) as usize);
        let (ty0, ty1) = ((v0 * sy) as usize, (v1 * sy) as usize);
        let inked: Vec<usize> = (ty0..ty1)
            .filter(|y| (tx0..tx1).any(|x| img.pixels[y * w + x].a() > 40))
            .collect();
        let k = (p1 - p0) / (ty1 - ty0) as f32;
        let top = row.pos.y + p0 + (inked[0] - ty0) as f32 * k;
        let bottom = row.pos.y + p0 + (inked[inked.len() - 1] + 1 - ty0) as f32 * k;
        (top, bottom)
    }

    /// At 100% a round prefix glyph inks centred on the same line as the square
    /// prefix glyphs, with no nudge of its own.
    ///
    /// flip: a one-pixel lift for `SQUIRCLE` in `prefix_top` → fails at the squircle's
    /// centre assertion.
    #[test]
    fn a_round_prefix_sits_level_with_the_others() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut centres = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            for g in [icon::CORNERS_OUT, icon::ROTATE_RIGHT, icon::SQUIRCLE] {
                let lead = Prefix::Icon(g).galley(ui);
                let top = prefix_top(&lead, 14.0).round();
                let (ink_top, ink_bottom) = ink_rows(ui.ctx(), &lead);
                centres.push(top + (ink_top + ink_bottom) / 2.0);
            }
        });
        assert!(
            (centres[0] - centres[1]).abs() <= 0.5,
            "fixture: the square glyphs agree: {centres:?}"
        );
        assert!(
            (centres[2] - centres[0]).abs() <= 0.5,
            "the squircle's ink centre: {centres:?}"
        );
    }
}

#[cfg(test)]
mod dropdown_place_tests {
    use super::dropdown_place;
    use eframe::egui;

    /// A list opens under its field, flips to right-aligned or above at the screen edges, and is clamped on screen.
    ///
    /// flip: `dropdown_place` answering `menu_place` → fails "under it, left edges together".
    #[test]
    fn a_list_opens_under_its_field_or_flips_at_the_edges() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |_| {},
        );
        let size = egui::vec2(200.0, 150.0);
        let field =
            |x: f32, y: f32| egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(100.0, 20.0));
        assert_eq!(
            dropdown_place(&ctx, field(50.0, 50.0), size, 4.0),
            egui::pos2(50.0, 74.0),
            "under it, left edges together"
        );
        assert_eq!(
            dropdown_place(&ctx, field(650.0, 50.0), size, 4.0),
            egui::pos2(550.0, 74.0),
            "right edges together"
        );
        assert_eq!(
            dropdown_place(&ctx, field(50.0, 500.0), size, 4.0),
            egui::pos2(50.0, 346.0),
            "above it at the bottom"
        );
        let tall = egui::vec2(200.0, 900.0);
        assert_eq!(
            dropdown_place(&ctx, field(50.0, 50.0), tall, 4.0).y,
            0.0,
            "a list taller than the screen is clamped to its top"
        );
    }
}

#[cfg(test)]
mod logo_tests {
    //! Plain backticks in this module's prose: `cargo doc` cannot see a `cfg(test)` module.
    use super::*;

    /// The mark is the 64px artwork with a transparent ground, and its texture is uploaded once.
    ///
    /// flip: dropping the `insert_temp` that keeps the handle → fails "the texture is kept".
    #[test]
    fn the_logo_is_the_64px_mark_and_uploads_once() {
        let icon = eframe::icon_data::from_png_bytes(LOGO_PNG).unwrap();
        assert_eq!((icon.width, icon.height), (64, 64));
        let alpha_at = |x: usize, y: usize| icon.rgba[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha_at(0, 0), 0, "a transparent corner");
        assert_eq!(alpha_at(32, 32), 0, "a transparent middle");
        assert_eq!(alpha_at(7, 37), 255, "solid ink on the coil");

        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(26.0, 26.0));
        let mut ids = Vec::new();
        for _ in 0..3 {
            let _ = ctx.run_ui(Default::default(), |ui| {
                logo(ui, rect, false);
                ids.push(
                    ui.ctx()
                        .data(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("ondin-logo")))
                        .map(|t| t.id()),
                );
            });
        }
        assert!(ids[0].is_some(), "the texture is kept");
        assert!(ids.iter().all(|i| *i == ids[0]), "and reused: {ids:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `edit_frame`d edit fills its row's height with its line on the row's
    /// centre, both shown in a field row beside a label and `put` over a box.
    ///
    /// flip: `edit_frame` padding one point more below the line than above, or
    /// `EditFrame::on` without `vertical_align` → fails at the row's line.
    #[test]
    fn an_edit_frames_line_sits_on_the_rows_centre() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let font = egui::FontId::proportional(CONTROL_PT);
        let boxed = egui::Rect::from_min_size(egui::pos2(10.0, 100.0), egui::vec2(200.0, 22.0));
        let (mut row, mut put) = (None, None);
        let out = ctx.run_ui(Default::default(), |ui| {
            let (mut a, mut b) = ("In a row".to_string(), "In a box".to_string());
            field_row(ui, egui::vec2(200.0, CONTROL_H), |ui| {
                ui.label(egui::RichText::new("Label").font(font.clone()));
                let pad = edit_frame(ui, &font, CONTROL_H - FIELD_BORDER_H, 0.0);
                row = Some(pad.add(
                    ui,
                    f32::INFINITY,
                    egui::TextEdit::singleline(&mut a).font(font.clone()),
                ));
            });
            let pad = edit_frame(ui, &font, boxed.height(), 0.0);
            put = Some(
                ui.put(
                    boxed,
                    pad.on(egui::TextEdit::singleline(&mut b))
                        .font(font.clone()),
                ),
            );
        });
        let mid = |want: &str| {
            out.shapes
                .iter()
                .find_map(|cs| match &cs.shape {
                    egui::Shape::Text(t) if t.galley.text() == want => {
                        Some(t.galley.rect.translate(t.pos.to_vec2()).center().y)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{want:?} is drawn"))
        };
        assert!(
            (mid("In a row") - mid("Label")).abs() < 0.01,
            "row: {} against the label's {}",
            mid("In a row"),
            mid("Label")
        );
        assert!(
            (mid("In a box") - boxed.center().y).abs() < 0.01,
            "box: {} against {}",
            mid("In a box"),
            boxed.center().y
        );
        assert_eq!(
            put.expect("the box drew").rect,
            boxed,
            "the edit takes the whole box"
        );
        assert_eq!(
            row.expect("the row drew").rect.height(),
            CONTROL_H - FIELD_BORDER_H,
            "the edit takes the row's whole height"
        );
    }

    /// A stored value outside a field's range is shown, not rewritten, and an event-free frame reports no change.
    ///
    /// flip: removing `.clamp_existing_to_range(false)` → fails at the first assertion (1000 where 1200 was).
    #[test]
    fn a_stored_value_outside_a_fields_range_is_shown_rather_than_rewritten() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let draw = |v: &mut f64| {
            let mut changed = false;
            let _ = ctx.run_ui(Default::default(), |ui| {
                let r = value_field(
                    ui,
                    egui::vec2(200.0, 28.0),
                    Prefix::Text("R"),
                    v,
                    Scrub::whole(1.0).range(0.0..=1000.0),
                    |d| d,
                );
                changed = r.changed();
            });
            changed
        };

        let mut out_of_range = 1200.0;
        let changed = draw(&mut out_of_range);
        assert_eq!(
            out_of_range, 1200.0,
            "the field draws what the document holds"
        );
        assert!(
            !changed,
            "and a frame nobody touched must not report a user edit"
        );

        // Control: an in-range value is equally quiet.
        let mut in_range = 500.0;
        let changed = draw(&mut in_range);
        assert_eq!(in_range, 500.0);
        assert!(!changed);
    }

    /// `hex_of` writes six digits and never a hash or the alpha.
    #[test]
    fn hex_of_writes_six_digits() {
        let c = ondin_core::peniko::Color::from_rgba8(0x6D, 0x8C, 0xD9, 0x80);
        assert_eq!(hex_of(c), "6D8CD9");
    }

    /// `Color -> Hsv -> Color` is the identity at 8-bit precision, low alpha included.
    #[test]
    fn hsv_round_trips_every_colour_exactly() {
        use ondin_core::peniko::Color;
        for c in [
            Color::from_rgba8(0x6D, 0x8C, 0xD9, 0xFF),
            Color::from_rgba8(0x6D, 0x8C, 0xD9, 0x40),
            Color::from_rgba8(0x6D, 0x8C, 0xD9, 0x01),
            Color::from_rgba8(0, 0, 0, 0xFF),
            Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xFF),
            Color::from_rgba8(0xFF, 0, 0, 0xFF),
            Color::from_rgba8(0, 0xFF, 0, 0x80),
            Color::from_rgba8(0, 0, 0xFF, 0x80),
            Color::from_rgba8(0x80, 0x80, 0x80, 0x80),
            Color::from_rgba8(0x12, 0x34, 0x56, 0x78),
        ] {
            let back = Hsv::from_color(c).to_color();
            let (a, b) = (c.to_rgba8().to_u8_array(), back.to_rgba8().to_u8_array());
            assert_eq!(a, b, "{a:?} -> {b:?}");
        }
    }

    /// The corner mask stays inside the box it rounds (a mitred tangent vertex once spiked out).
    #[test]
    fn the_corner_mask_never_paints_outside_its_rect() {
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(14.0, 14.0));
        let mut tess = egui::epaint::tessellator::Tessellator::new(
            1.0,
            egui::epaint::TessellationOptions::default(),
            [16, 16],
            vec![],
        );
        let mask = corner_mask(rect, 4.0, egui::Color32::RED);
        assert!(!mask.indices.is_empty(), "the mask covered nothing");

        let mut out = egui::epaint::Mesh::default();
        tess.tessellate_shape(egui::Shape::mesh(mask), &mut out);
        for v in &out.vertices {
            assert!(
                rect.distance_to_pos(v.pos) <= 0.01,
                "mask vertex {:?} escaped {rect:?}",
                v.pos
            );
        }
    }

    /// A grey keeps the hue it is given, so saturating it does not jump to red.
    #[test]
    fn a_grey_keeps_whatever_hue_it_is_given() {
        let c = Hsv::new(0.6, 0.0, 0.5, 1.0).to_color();
        assert_eq!(Hsv::from_color(c).s, 0.0);
        let saturated = Hsv::new(0.6, 1.0, 0.5, 1.0).to_color();
        assert!((Hsv::from_color(saturated).h - 0.6).abs() < 1e-5);
    }

    /// Paints one `value_field` in a card; returns the shapes and the field's id.
    fn paint_value_field(
        ctx: &egui::Context,
        prefix: Prefix,
        focus: Option<egui::Id>,
    ) -> (Vec<egui::Shape>, egui::Id) {
        let mut id = egui::Id::NULL;
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(284.0);
            card(ui, |ui| {
                let w = ui.available_width();
                ui.horizontal(|ui| {
                    let mut v = 101.548_f64;
                    if let Some(f) = focus {
                        ui.ctx().memory_mut(|m| m.request_focus(f));
                    }
                    id = value_field(
                        ui,
                        egui::vec2(w, 28.0),
                        prefix,
                        &mut v,
                        Scrub::whole(0.5),
                        |d| d,
                    )
                    .id;
                });
            });
        });
        (out.shapes.into_iter().map(|c| c.shape).collect(), id)
    }

    /// A field keeps the width it was given at the largest value it accepts; the resting `DragValue` button grows rather than clips.
    #[test]
    fn a_value_field_holds_its_width_up_to_the_largest_value_it_accepts() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        for width in [88.0_f32, 131.0] {
            for value in [1.0_f64, 100.0, 1000.0, 9999.0, MAX_STROKE_WIDTH] {
                let out = ctx.run_ui(Default::default(), |ui| {
                    ui.set_max_width(300.0);
                    let mut v = value;
                    value_field(
                        ui,
                        egui::vec2(width, 28.0),
                        Prefix::Icon(icon::LINE_SEGMENT),
                        &mut v,
                        Scrub::whole(0.5),
                        |d| d.custom_formatter(number(2)),
                    );
                });
                let painted = out
                    .shapes
                    .iter()
                    .filter_map(|c| match &c.shape {
                        egui::Shape::Rect(r) if (r.rect.height() - 28.0).abs() < 0.51 => {
                            Some(r.rect.width())
                        }
                        _ => None,
                    })
                    .fold(0.0_f32, f32::max);
                assert!(
                    painted <= width + 0.51,
                    "a field asked for {width} painted {painted} holding {value} — it grew, \
                     and the card and the panel grow with it"
                );
            }
        }
    }

    /// The prefix glyph is painted in both the resting and the keyboard face of a field.
    #[test]
    fn a_value_fields_prefix_survives_being_typed_into() {
        for prefix in [Prefix::Text("W"), Prefix::Icon(icon::ROTATE_RIGHT)] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            paint_value_field(&ctx, prefix, None); // warm the font atlas

            let glyphs = |shapes: &[egui::Shape]| {
                shapes
                    .iter()
                    .filter(|s| match s {
                        egui::Shape::Text(t) => !t.galley.text().starts_with("101"),
                        _ => false,
                    })
                    .count()
            };

            let (idle, id) = paint_value_field(&ctx, prefix, None);
            paint_value_field(&ctx, prefix, Some(id)); // focus lands next frame
            let (focused, _) = paint_value_field(&ctx, prefix, Some(id));

            assert!(glyphs(&idle) > 0, "the prefix is not painted at rest");
            assert!(
                glyphs(&focused) > 0,
                "the prefix vanished while the field was being typed into"
            );
        }
    }

    /// Focusing a field does not move the value, and the only outline is the row's own.
    #[test]
    fn focusing_a_value_field_outlines_the_row_and_leaves_the_value_put() {
        for prefix in [Prefix::Text("W"), Prefix::Icon(icon::ROTATE_RIGHT)] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            paint_value_field(&ctx, prefix, None); // warm the font atlas

            let value_x = |shapes: &[egui::Shape]| {
                shapes
                    .iter()
                    .find_map(|s| match s {
                        egui::Shape::Text(t) if t.galley.text().starts_with("101") => Some(t.pos.x),
                        _ => None,
                    })
                    .expect("the value is painted")
            };
            let outlines = |shapes: &[egui::Shape]| {
                shapes
                    .iter()
                    .filter_map(|s| match s {
                        egui::Shape::Rect(r) if r.stroke.width > 0.0 => Some(r.rect),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };

            let (idle, id) = paint_value_field(&ctx, prefix, None);
            paint_value_field(&ctx, prefix, Some(id)); // focus lands next frame
            let (focused, _) = paint_value_field(&ctx, prefix, Some(id));

            assert!(
                (value_x(&idle) - value_x(&focused)).abs() < 0.01,
                "the value moved on focus: {} -> {}",
                value_x(&idle),
                value_x(&focused)
            );

            let rings = outlines(&focused);
            let row = outlines(&idle)[0];
            assert!(
                rings.iter().all(|r| *r == row),
                "a focused field drew an outline that is not the row's {row:?}: {rings:?}"
            );
            assert!(
                value_x(&focused) - row.min.x >= 9.0,
                "the value sits {}px from the outline",
                value_x(&focused) - row.min.x
            );
        }
    }

    /// The window the wrap tests run in: room for two trigger bands and a gap, which `wrap_scrub` requires.
    const WINDOW: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::pos2(1200.0, 800.0),
    };

    /// Lays out one `value_field` over `v` fed `events`; returns its box and the full output.
    fn drive_value_field_out(
        ctx: &egui::Context,
        v: &mut f64,
        events: Vec<egui::Event>,
    ) -> (egui::Rect, egui::FullOutput) {
        let mut rect = egui::Rect::NOTHING;
        let out = ctx.run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(WINDOW),
                ..Default::default()
            },
            |ui| {
                ui.set_width(284.0);
                rect = value_field(
                    ui,
                    egui::vec2(200.0, 28.0),
                    Prefix::Text("W"),
                    v,
                    Scrub::whole(1.0 / SCRUB_SCALE),
                    |d| d.custom_formatter(number(2)),
                )
                .rect;
            },
        );
        (rect, out)
    }

    fn drive_value_field(ctx: &egui::Context, v: &mut f64, events: Vec<egui::Event>) -> egui::Rect {
        drive_value_field_out(ctx, v, events).0
    }

    /// Where a pass asked the pointer to be moved to, if it did.
    fn wrapped_to(out: &egui::FullOutput) -> Option<egui::Pos2> {
        out.viewport_output.values().find_map(|v| {
            v.commands.iter().find_map(|c| match c {
                egui::ViewportCommand::CursorPosition(p) => Some(*p),
                _ => None,
            })
        })
    }

    /// How many decimal places the field painted this pass, read off the shapes.
    fn digits_shown(out: &egui::FullOutput) -> usize {
        let text = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) => {
                    let s = t.galley.text();
                    s.starts_with(|c: char| c.is_ascii_digit() || c == '-')
                        .then(|| s.to_owned())
                }
                _ => None,
            })
            .expect("the value is painted");
        text.split_once('.').map_or(0, |(_, frac)| frac.len())
    }

    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// A press on the prefix and a drag right moves the number; driven with real pointer events.
    #[test]
    fn dragging_a_value_field_by_its_prefix_moves_the_number() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut v = 100.0_f64;
        let rect = drive_value_field(&ctx, &mut v, vec![]); // warm fonts and geometry

        let on_prefix = egui::pos2(rect.left() + 2.0, rect.center().y);
        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(on_prefix), press(on_prefix, true)],
        );
        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(on_prefix + egui::vec2(20.0, 0.0))],
        );
        assert!(
            v > 100.0,
            "a drag starting on the prefix left the value at {v}"
        );
        drive_value_field(
            &ctx,
            &mut v,
            vec![press(on_prefix + egui::vec2(20.0, 0.0), false)],
        );
    }

    /// A scrub that reaches the window's edge asks (via `CursorPosition`) for the pointer on the far side, horizontally only.
    #[test]
    fn a_scrub_that_reaches_an_edge_wraps_the_pointer_to_the_far_side() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut v = 100.0_f64;
        let rect = drive_value_field(&ctx, &mut v, vec![]);
        let start = rect.center();

        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(start), press(start, true)],
        );
        let at_edge = egui::pos2(WRAP_EDGE - 4.0, start.y);
        let (_, out) =
            drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(at_edge)]);

        let to = wrapped_to(&out).expect("reaching the left edge should wrap the pointer");
        assert!(
            to.x > WINDOW.right() - WRAP_EDGE * 3.0 && to.x < WINDOW.right() - WRAP_EDGE,
            "wrapped to {to:?}, which is not just inside the right-hand edge"
        );
        assert_eq!(to.y, at_edge.y, "the vertical axis must not be touched");

        let (_, quiet) = drive_value_field_out(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(egui::pos2(600.0, 400.0))],
        );
        assert!(
            wrapped_to(&quiet).is_none(),
            "a scrub in open space wrapped"
        );
    }

    /// The pass after a wrap does not move the value (`scrub_speed_for` zeroes the teleport's delta), and the next real move scrubs again.
    #[test]
    fn the_pass_after_a_wrap_swallows_the_teleports_own_delta() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut v = 100.0_f64;
        let rect = drive_value_field(&ctx, &mut v, vec![]);
        let start = rect.center();
        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(start), press(start, true)],
        );

        let at_edge = egui::pos2(WRAP_EDGE - 4.0, start.y);
        let (_, out) =
            drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(at_edge)]);
        let to = wrapped_to(&out).expect("the wrap");
        let before = v;

        drive_value_field(&ctx, &mut v, vec![egui::Event::PointerMoved(to)]);
        assert_eq!(
            v,
            before,
            "the teleport moved the value by {} — it must cost nothing but its own pass",
            v - before
        );

        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(to - egui::vec2(20.0, 0.0))],
        );
        assert!(v < before, "the drag stopped scrubbing after the wrap: {v}");
    }

    /// A teleport is swallowed whenever it arrives, not only on the next pass; real motion in between scrubs normally.
    #[test]
    fn a_teleport_that_lands_late_is_still_swallowed() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut v = 100.0_f64;
        let rect = drive_value_field(&ctx, &mut v, vec![]);
        let start = rect.center();
        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(start), press(start, true)],
        );

        let edge = egui::pos2(WRAP_EDGE - 4.0, start.y);
        let (_, out) = drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(edge)]);
        let to = wrapped_to(&out).expect("the wrap");

        let mut at = edge;
        for _ in 0..4 {
            let before = v;
            at = egui::pos2(at.x + 3.0, at.y);
            let (_, quiet) =
                drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(at)]);
            assert!(
                wrapped_to(&quiet).is_none(),
                "a second wrap was requested while the first was still in flight"
            );
            assert!(
                v > before,
                "real movement during the wait stopped scrubbing at {v}"
            );
        }

        let before = v;
        drive_value_field(&ctx, &mut v, vec![egui::Event::PointerMoved(to)]);
        assert_eq!(
            v,
            before,
            "the late teleport moved the value by {}",
            v - before
        );

        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(to - egui::vec2(15.0, 0.0))],
        );
        assert!(v < before, "the drag stopped scrubbing after the wrap: {v}");
    }

    /// A wrap does not change the digits shown (`number` pins the precision the zeroed speed would blow up).
    #[test]
    fn a_wrap_does_not_change_how_many_digits_a_field_shows() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut v = 411.672_736_201_535_26_f64;
        let rect = drive_value_field(&ctx, &mut v, vec![]);
        let start = rect.center();
        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(start), press(start, true)],
        );

        let edge = egui::pos2(WRAP_EDGE - 4.0, start.y);
        let (_, out) = drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(edge)]);
        let to = wrapped_to(&out).expect("the wrap");
        assert!(digits_shown(&out) <= 2, "before the teleport");

        let (_, landing) = drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(to)]);
        assert!(
            digits_shown(&landing) <= 2,
            "the teleport pass showed {} decimals",
            digits_shown(&landing)
        );
    }

    /// A wrap request the platform never answers is given up after `WRAP_GIVE_UP` passes, and a fresh one is allowed.
    #[test]
    fn an_unanswered_wrap_request_is_eventually_given_up_on() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut v = 100.0_f64;
        let rect = drive_value_field(&ctx, &mut v, vec![]);
        let start = rect.center();
        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(start), press(start, true)],
        );

        let edge = egui::pos2(WRAP_EDGE - 4.0, start.y);
        let (_, out) = drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(edge)]);
        assert!(wrapped_to(&out).is_some(), "the first request");

        for _ in 0..WRAP_GIVE_UP {
            drive_value_field(&ctx, &mut v, vec![egui::Event::PointerMoved(edge)]);
        }
        let (_, again) = drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(edge)]);
        assert!(
            wrapped_to(&again).is_some(),
            "after giving up, a fresh request should be allowed"
        );
    }

    /// The digits start one prefix width and one gap in from the field's edge.
    #[test]
    fn the_number_still_sits_a_prefix_and_a_gap_from_the_fields_edge() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        for prefix in [
            Prefix::Text("W"),
            Prefix::Icon(icon::ROTATE_RIGHT),
            Prefix::Label("flex-grow"),
        ] {
            paint_value_field(&ctx, prefix, None); // warm the font atlas
            let (shapes, _) = paint_value_field(&ctx, prefix, None);
            let texts: Vec<_> = shapes
                .iter()
                .filter_map(|s| match s {
                    egui::Shape::Text(t) => Some((t.pos.x, t.galley.text().to_owned())),
                    _ => None,
                })
                .collect();
            let value = texts
                .iter()
                .find(|(_, t)| t.starts_with("101"))
                .expect("the value is painted");
            let label = texts
                .iter()
                .find(|(_, t)| !t.starts_with("101"))
                .expect("the prefix is painted");
            let want = ctx.fonts_mut(|f| {
                let lead = match prefix {
                    Prefix::Text(t) => f.layout_no_wrap(
                        t.to_owned(),
                        egui::FontId::proportional(PREFIX_PT),
                        egui::Color32::WHITE,
                    ),
                    Prefix::Icon(g) => f.layout_no_wrap(
                        g.to_owned(),
                        theme::icon_font(PREFIX_ICON_PT),
                        egui::Color32::WHITE,
                    ),
                    Prefix::Label(t) => f.layout_no_wrap(
                        t.to_owned(),
                        egui::FontId::proportional(SUFFIX_PT),
                        egui::Color32::WHITE,
                    ),
                };
                (lead.size().x + PREFIX_GAP).round()
            });
            assert!(
                (value.0 - label.0 - want).abs() < 0.51,
                "the number is {} from the prefix, not {want}",
                value.0 - label.0
            );
        }
    }

    /// A bare field's number starts where a prefix would, with no gap reserved.
    /// flip: `prefix_inset` reserving the gap for an empty run → fails at "where `W` stands".
    #[test]
    fn a_bare_field_reserves_no_prefix_gap() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let x_of = |prefix: Prefix, starts: &str| {
            paint_value_field(&ctx, prefix, None);
            let (shapes, _) = paint_value_field(&ctx, prefix, None);
            shapes
                .iter()
                .find_map(|s| match s {
                    egui::Shape::Text(t) if t.galley.text().starts_with(starts) => Some(t.pos.x),
                    _ => None,
                })
                .expect("painted")
        };
        let prefix_at = x_of(Prefix::Text("W"), "W");
        let bare_at = x_of(Prefix::Text(""), "101");
        assert!(
            (bare_at - prefix_at).abs() < 0.51,
            "the bare number is at {bare_at}, not where `W` stands at {prefix_at}"
        );
    }

    /// **A text field takes a press anywhere on its height**, not only on its
    /// line of text, and shows the I-beam there.
    ///
    /// flip: `text_field` giving its edit `Frame::NONE` rather than `edit_frame`
    /// → fails at the edit spanning the row (one 15-point line in 26).
    #[test]
    fn a_text_field_takes_a_press_on_its_whole_height() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut text = String::new();
        let mut pass = |events: Vec<egui::Event>| {
            let mut field = None;
            let out = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    let resp = text_field(ui, egui::vec2(200.0, CONTROL_H), &mut text, "Name");
                    field = Some((resp.rect, resp.has_focus()));
                },
            );
            let (rect, focused) = field.expect("drawn");
            (rect, focused, out.platform_output.cursor_icon)
        };
        let (rect, ..) = pass(Vec::new());
        assert!(
            rect.height() >= CONTROL_H - FIELD_BORDER_H - 0.5,
            "the edit spans the row: {rect:?}"
        );
        // Three points under the hairline, well above the line of text.
        let at = egui::pos2(rect.left() + 30.0, rect.top() + 3.0);
        for _ in 0..3 {
            pass(vec![egui::Event::PointerMoved(at)]);
        }
        let (.., icon) = pass(vec![egui::Event::PointerMoved(at)]);
        assert_eq!(icon, egui::CursorIcon::Text);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        pass(vec![button(true)]);
        pass(vec![button(false)]);
        let (_, focused, _) = pass(Vec::new());
        assert!(focused, "the press focused the field");
    }

    /// A search box holding text shows a clear button with an arrow pointer; a
    /// press on it empties the field, reports a change and keeps the keyboard.
    ///
    /// flip: `search_box` laying its row out left to right, so the edit takes the
    /// whole row before the ✕ → fails at the arrow pointer.
    #[test]
    fn a_search_box_clears_from_its_button() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let id = egui::Id::new("search-clear-test");
        let mut text = "card".to_owned();
        let mut pass = |events: Vec<egui::Event>| {
            let mut seen = None;
            let out = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    let resp = search_box(ui, egui::vec2(200.0, CONTROL_H), &mut text, "Find", id);
                    seen = Some((resp.changed(), resp.has_focus(), text.clone()));
                },
            );
            let (changed, focused, now) = seen.expect("drawn");
            (changed, focused, now, out.platform_output.cursor_icon)
        };
        pass(Vec::new());
        ctx.memory_mut(|m| m.request_focus(id));
        pass(Vec::new());
        // The ✕ sits at the field's right, its inset in from the edge.
        let at = egui::pos2(
            200.0 - SEARCH_CLEAR_INSET - SEARCH_CLEAR / 2.0,
            CONTROL_H / 2.0,
        );
        for _ in 0..3 {
            pass(vec![egui::Event::PointerMoved(at)]);
        }
        let (.., icon) = pass(vec![egui::Event::PointerMoved(at)]);
        assert_eq!(icon, egui::CursorIcon::Default, "an arrow over the ✕");
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        pass(vec![button(true)]);
        let (changed, _, now, _) = pass(vec![button(false)]);
        assert_eq!(now, "", "the press emptied the field");
        assert!(changed, "and the caller heard it");
        let (_, focused, ..) = pass(Vec::new());
        assert!(focused, "the keyboard stays in the field");
    }

    /// A search box is as wide holding text, ✕ and all, as it is empty.
    ///
    /// flip: `search_box` laying its row out left to right, the edit sized short of
    /// the ✕ → fails at the width. Sizing the edit wrong inside the right-to-left
    /// row does not bite: the row spans the magnifier to the ✕ either way.
    #[test]
    fn a_search_box_keeps_its_width_when_the_clear_button_shows() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // The painted ground: the row's allocation is `size` whatever its frame does.
        let width = |text: &str| {
            let mut text = text.to_owned();
            let mut out = None;
            for _ in 0..2 {
                out = Some(ctx.run_ui(Default::default(), |ui| {
                    let id = egui::Id::new("search-width-test");
                    search_box(ui, egui::vec2(200.0, CONTROL_H), &mut text, "Find", id);
                }));
            }
            let field = color::field();
            out.expect("ran")
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Rect(r) if r.fill == field => Some(r.rect.width()),
                    _ => None,
                })
                .fold(0.0, f32::max)
        };
        let empty = width("");
        assert!(
            empty > 190.0,
            "fixture: the empty field fills its row, {empty}"
        );
        assert_eq!(width("card"), empty, "the ✕ costs the field no width");
    }

    /// A colour typed into a hex field replaces the one there; driven with real events.
    #[test]
    fn a_hex_field_is_retyped_rather_than_inserted_into() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut text = "6D8CD9".to_owned();
        let run = |events: Vec<egui::Event>, text: &mut String| {
            let mut rect = egui::Rect::NOTHING;
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(text)
                            .char_limit(COLOUR_CHAR_LIMIT)
                            .desired_width(62.0),
                    );
                    select_all_on_focus(ui, &resp, text);
                    rect = resp.rect;
                },
            );
            rect
        };
        let at = run(vec![], &mut text).center(); // learn the geometry
        run(
            vec![
                egui::Event::PointerMoved(at),
                press(at, true),
                press(at, false),
            ],
            &mut text,
        );
        // The selection is stored behind that frame's caret placement, so typing lands a frame later.
        run(vec![egui::Event::Text("F".to_owned())], &mut text);
        assert_eq!(
            text, "F",
            "typing into a focused hex field should replace the colour, not extend it"
        );
    }

    /// The filled lock and hexagon land centred on the point they are given, the
    /// lock also when turned with a rotated frame.
    ///
    /// flip: `paint_glyph_centred` placing the galley's corner on `center` → fails
    /// at the centring assertion.
    #[test]
    fn the_filled_marks_centre_on_their_point() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let centre = egui::pos2(40.0, 30.0);
        for (name, rot) in [
            ("lock", 0.0),
            ("lock", std::f32::consts::FRAC_PI_2),
            ("hexagon", 0.0),
        ] {
            let out = ctx.run_ui(Default::default(), |ui| {
                let p = ui.painter();
                if name == "lock" {
                    paint_lock_filled(p, centre, 16.0, egui::Color32::WHITE, rot);
                } else {
                    paint_hexagon_filled(p, centre, 16.0, egui::Color32::WHITE);
                }
            });
            let texts: Vec<_> = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t),
                    _ => None,
                })
                .collect();
            assert_eq!(texts.len(), 1, "{name}: one glyph");
            let at = texts[0].visual_bounding_rect().center();
            assert!(
                at.distance(centre) < 1.0,
                "{name} at {rot} lands at {at:?}, not on {centre:?}"
            );
        }
    }

    /// A label too long for its button is elided inside it: the galley is elided and its ink stays within the button's rect.
    #[test]
    fn a_long_action_button_label_is_elided_inside_the_button() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        // The Export panel's button width.
        const W: f32 = 284.0 - CARD_MARGIN_X * 2.0 - 28.0 - 7.0;

        for long in [
            "Export Ellipse 11312312 3123123132",
            "Export Ellipse 11312312 3123123132 copy copy copy",
        ] {
            let mut rect = egui::Rect::NOTHING;
            let out = ctx.run_ui(Default::default(), |ui| {
                rect = action_button(
                    ui,
                    theme::icon::EXPORT,
                    long,
                    FieldButton::Off,
                    egui::vec2(W, 28.0),
                )
                .rect;
            });
            let texts: Vec<&egui::epaint::TextShape> = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t),
                    _ => None,
                })
                .collect();
            assert_eq!(texts.len(), 2, "the button paints a glyph and a word");
            let word = texts
                .iter()
                .find(|t| t.galley.job.text == long)
                .expect("the label is one of the two");
            assert!(
                word.galley.elided,
                "{long:?} was laid out in full — {:.1}pt of word, {:.1}pt of button, \
                 and `p.text` clips against nothing",
                word.galley.size().x,
                W,
            );
            for t in &texts {
                let ink = egui::Rect::from_min_size(t.pos, t.galley.size());
                assert!(
                    ink.left() >= rect.left() && ink.right() <= rect.right(),
                    "{long:?}: ink spans {:.1}..{:.1}, outside the button's {:.1}..{:.1}",
                    ink.left(),
                    ink.right(),
                    rect.left(),
                    rect.right()
                );
            }
        }
    }
}

#[cfg(test)]
mod head_tests {
    use super::*;

    /// A header's `+` and `⋯` sit `HEAD_MORE_GAP` apart and their press targets meet mid-gap.
    ///
    /// flip: putting back the `⋯`'s `add_space(-item_spacing)` → fails "the boxes are the gap apart".
    #[test]
    fn a_heads_plus_and_more_keep_their_own_presses() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
        let run = |events: Vec<egui::Event>| {
            let mut clicks = HeadClicks::default();
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    clicks = section_head_full(
                        ui,
                        "Fill",
                        true,
                        Some(HeadAction::new(icon::PLUS, "Add")),
                        None,
                        None,
                        true,
                        None,
                        true,
                    );
                },
            );
            clicks
        };
        run(Vec::new());
        let at_rest = run(Vec::new());
        let (plus, more) = (
            at_rest.action_rect.expect("a +"),
            at_rest.more_rect.expect("a ⋯"),
        );
        assert_eq!(
            more.left() - plus.right(),
            HEAD_MORE_GAP,
            "the boxes are the gap apart"
        );
        let click = |p: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            run(vec![egui::Event::PointerMoved(p)]);
            run(vec![press(true), press(false)])
        };
        let y = more.center().y;
        let on_more = click(egui::pos2(more.left() + 1.0, y));
        assert!(on_more.more && !on_more.acted, "the ⋯'s edge is the ⋯'s");
        let mid = (plus.right() + more.left()) / 2.0;
        let left = click(egui::pos2(mid - 1.0, y));
        assert!(left.acted && !left.more, "left of the gap's middle, the +");
        let right = click(egui::pos2(mid + 1.0, y));
        assert!(right.more && !right.acted, "right of it, the ⋯");
    }

    /// A press on the centre of a menu head, where the glyphs are painted, clicks it.
    #[test]
    fn a_menu_head_is_clicked_on_its_own_glyph() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run_ui(Default::default(), |ui| {
            rect = menu_head(
                ui,
                crate::theme::icon_text(icon::EYE, 16.0, theme::text::muted()),
                1.0,
                false,
            )
            .rect;
        });
        assert!(rect.is_positive(), "the head allocated nothing");
        assert_eq!(rect.height(), HEAD_H);

        let centre = rect.center();
        let press = |pressed| egui::Event::PointerButton {
            pos: centre,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let raw = egui::RawInput {
            events: vec![egui::Event::PointerMoved(centre), press(true), press(false)],
            ..Default::default()
        };
        let mut clicked = false;
        let _ = ctx.run_ui(raw, |ui| {
            clicked = menu_head(
                ui,
                crate::theme::icon_text(icon::EYE, 16.0, theme::text::muted()),
                1.0,
                false,
            )
            .clicked();
        });
        assert!(clicked, "a press on the glyph did not reach the head");
    }

    /// A menu head hovered or `open` paints the `hover()` ground and mark and caret in `text()`; at rest it paints neither.
    ///
    /// flip: ignoring `open` → fails "an open head stays lit with the pointer away".
    #[test]
    fn a_menu_head_lights_under_the_pointer_like_an_icon_button() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let head = |ui: &mut egui::Ui, open: bool| {
            menu_head(
                ui,
                crate::theme::icon_text(icon::EYE, 16.0, theme::text::muted()),
                5.0,
                open,
            )
        };
        // Answers (rect, ground fills, text runs in `text()`, text runs) for one frame.
        let frame = |ctx: &egui::Context, raw: egui::RawInput, open: bool| {
            let mut rect = egui::Rect::NOTHING;
            let out = ctx.run_ui(raw, |ui| rect = head(ui, open).rect);
            let (mut grounds, mut lit, mut texts) = (0, 0, 0);
            for cs in &out.shapes {
                match &cs.shape {
                    egui::Shape::Rect(r) if r.fill == theme::color::hover() => grounds += 1,
                    egui::Shape::Text(t) => {
                        texts += 1;
                        if t.override_text_color == Some(color::text()) {
                            lit += 1;
                        }
                    }
                    _ => {}
                }
            }
            (rect, grounds, lit, texts)
        };

        let (rect, grounds, lit, texts) = frame(&ctx, Default::default(), false);
        assert_eq!(texts, 2, "the fixture draws a mark and a caret");
        assert_eq!((grounds, lit), (0, 0), "a head at rest lights nothing");

        let (_, grounds, lit, _) = frame(&ctx, Default::default(), true);
        assert_eq!(
            (grounds, lit),
            (1, 2),
            "an open head stays lit with the pointer away"
        );

        // One frame for the hover to register, one to read it back.
        let over = egui::RawInput {
            events: vec![egui::Event::PointerMoved(rect.center())],
            ..Default::default()
        };
        let _ = frame(&ctx, over, false);
        let (_, grounds, lit, _) = frame(&ctx, Default::default(), false);
        assert_eq!(grounds, 1, "a hovered head paints the HOVER ground");
        assert_eq!(lit, 2, "and draws its mark and its caret in TEXT");

        let away = egui::RawInput {
            events: vec![egui::Event::PointerGone],
            ..Default::default()
        };
        let _ = frame(&ctx, away, false);
        let (_, grounds, lit, _) = frame(&ctx, Default::default(), false);
        assert_eq!((grounds, lit), (0, 0), "closed and left, it goes dark");
    }

    /// A card's eyebrow takes the arrow, not an I-beam (the selectable-labels control proves the hover registered).
    #[test]
    fn a_card_eyebrow_takes_the_arrow_rather_than_the_i_beam() {
        let over_the_eyebrow = |selectable: bool| {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            if selectable {
                ctx.all_styles_mut(|s| s.interaction.selectable_labels = true);
            }
            let head = |ui: &mut egui::Ui| {
                section_head(ui, "Appearance", true, None, true);
            };
            let out = ctx.run_ui(Default::default(), head);
            let at = out
                .shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Text(t) if t.galley.job.text == "APPEARANCE" => {
                        Some(egui::Rect::from_min_size(t.pos, t.galley.size()).center())
                    }
                    _ => None,
                })
                .expect("the eyebrow is painted");
            // Interaction state is last frame's, so hold the pointer for several frames.
            let mut icon = egui::CursorIcon::default();
            for _ in 0..3 {
                let raw = egui::RawInput {
                    events: vec![egui::Event::PointerMoved(at)],
                    ..Default::default()
                };
                icon = ctx.run_ui(raw, head).platform_output.cursor_icon;
            }
            icon
        };
        assert_eq!(
            over_the_eyebrow(false),
            egui::CursorIcon::Default,
            "the header is chrome and a click target — it must not offer to select its own word"
        );
        assert_eq!(
            over_the_eyebrow(true),
            egui::CursorIcon::Text,
            "with egui's default restored the pointer is over the word — \
             so the arrow above is the style's doing and not a missed hover"
        );
    }

    /// A menu head's width grows by exactly the `gap` it is asked for.
    #[test]
    fn a_menu_heads_width_is_its_two_glyphs_and_the_gap() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let measure = |gap: f32| {
            let mut w = 0.0;
            let _ = ctx.run_ui(Default::default(), |ui| {
                w = menu_head(
                    ui,
                    crate::theme::icon_text(icon::EYE, 16.0, theme::text::muted()),
                    gap,
                    false,
                )
                .rect
                .width();
            });
            w
        };
        assert!((measure(5.0) - measure(1.0) - 4.0).abs() < 1e-3);
    }

    /// A head's trailing pad (`HEAD_PAD`) is the inset an icon button gives its glyph.
    #[test]
    fn a_head_and_an_icon_button_inset_their_glyphs_alike() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        /// The glyph size the top bar's marks and its undo/redo squares share.
        const GLYPH: f32 = 16.0;
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.horizontal(|ui| {
                icon_button(ui, icon::ARROW_U_UP_LEFT, HEAD_H, GLYPH, false, true);
                menu_head(
                    ui,
                    crate::theme::icon_text(icon::EYE, GLYPH, theme::text::muted()),
                    5.0,
                    false,
                );
            });
        });

        let inks: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.visual_bounding_rect()),
                _ => None,
            })
            .collect();
        assert!(inks.len() >= 2, "expected two marks, got {inks:?}");

        let button_inset = (HEAD_H - GLYPH) / 2.0;
        assert!(
            (button_inset - HEAD_PAD).abs() < 1e-3,
            "a {HEAD_H}pt button around a {GLYPH}pt glyph insets it \
             {button_inset}, but a head pads by {HEAD_PAD}"
        );
    }

    /// A rule's hairline sits on the device-pixel grid its width needs, within half a pixel of its column's centre, at several scales.
    #[test]
    fn a_rules_hairline_lands_on_a_pixel_inside_its_own_column() {
        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            ctx.set_pixels_per_point(ppp);
            let _ = ctx.run_ui(Default::default(), |_| {});

            // A fractional origin: start the row with something of non-integer width.
            let mut column = egui::Rect::NOTHING;
            let out = ctx.run_ui(Default::default(), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    menu_head(ui, egui::RichText::new("100%").size(11.5), 5.0, false);
                    column = ui.scope(|ui| rule(ui, 16.0)).response.rect;
                });
            });
            let painted: Vec<f32> = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::LineSegment { points, .. } if points[0].x == points[1].x => {
                        Some(points[0].x)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(painted.len(), 1, "{ppp}x: expected one hairline");
            let x = painted[0];

            let device = x * ppp;
            let want_centre = hairline_on_pixel_centre(ppp, 1.0);
            let off = match want_centre {
                true => (device - device.floor() - 0.5).abs(),
                false => (device - device.round()).abs(),
            };
            assert!(
                off < 1e-3,
                "{ppp}x: a 1pt line is {} device pixels, so it wants a pixel {}; \
                 the hairline at {x} is {device} in device space",
                (1.0 * ppp).floor(),
                if want_centre { "centre" } else { "edge" }
            );
            let drift = (x - column.center().x).abs();
            assert!(
                drift <= 0.5 / ppp + 1e-3,
                "{ppp}x: hairline at {x} is {drift} from the centre of {column:?}, \
                 more than the half-pixel the grid costs"
            );
        }
    }

    /// Fields and the button beside them paint grounds of the requested height on one baseline (`FIELD_BORDER_H`).
    #[test]
    fn a_field_row_is_one_height_on_one_baseline() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        const H: f32 = 28.0;
        let out = ctx.run_ui(Default::default(), |ui| {
            let mut a = 101.5_f64;
            let mut b = 202.5_f64;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                value_field(
                    ui,
                    egui::vec2(99.0, H),
                    Prefix::Text("X"),
                    &mut a,
                    Scrub::whole(0.5),
                    |d| d,
                );
                value_field(
                    ui,
                    egui::vec2(99.0, H),
                    Prefix::Text("Y"),
                    &mut b,
                    Scrub::whole(0.5),
                    |d| d,
                );
                field_button(ui, icon::CORNERS_SEPARATE, H, FieldButton::Off);
            });
        });

        let grounds: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill == color::field() => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(grounds.len(), 3, "expected three grounds, got {grounds:?}");
        for g in &grounds {
            assert_eq!(g.height(), H, "{g:?} is not {H} tall");
            assert_eq!(
                g.min.y, grounds[0].min.y,
                "{g:?} is off the row's baseline {:?}",
                grounds[0]
            );
        }
    }
}

#[cfg(test)]
mod dim_tests {
    use super::*;

    /// `Ui::disable` fades grounds, strokes and glyphs alike by `disabled_alpha`, explicitly-coloured text included.
    ///
    /// Reads the galley's mesh vertices: `job.sections[..].format.color` is the layout input, never adjusted.
    #[test]
    fn disabling_a_scope_fades_everything_including_explicitly_coloured_text() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let draw = |off: bool| {
            ctx.run_ui(Default::default(), |ui| {
                ui.scope(|ui| {
                    if off {
                        ui.disable();
                    }
                    ui.label(egui::RichText::new("A").color(theme::text::muted()));
                    ui.label("B");
                    let p = ui.painter();
                    p.text(
                        egui::pos2(50.0, 50.0),
                        egui::Align2::LEFT_TOP,
                        "C",
                        egui::FontId::proportional(12.0),
                        theme::text::muted(),
                    );
                    p.rect_filled(
                        egui::Rect::from_min_size(egui::pos2(0.0, 80.0), egui::vec2(9.0, 9.0)),
                        egui::CornerRadius::ZERO,
                        color::accent(),
                    );
                });
            })
        };
        let ink = |out: &egui::FullOutput, want: &str| -> egui::Color32 {
            out.shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == want => {
                        let v =
                            t.galley.rows.iter().find_map(|r| {
                                r.row.visuals.mesh.vertices.first().map(|v| v.color)
                            })?;
                        Some(if v == egui::Color32::PLACEHOLDER {
                            t.fallback_color
                        } else {
                            v
                        })
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no text shape for {want:?}"))
        };
        let ground = |out: &egui::FullOutput| -> egui::Color32 {
            out.shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if r.fill.a() > 0 => Some(r.fill),
                    _ => None,
                })
                .expect("no filled rect")
        };
        let (on, off) = (draw(false), draw(true));

        assert_ne!(ground(&on), ground(&off), "a ground must fade");
        for name in ["A", "B", "C"] {
            let (lit, dim) = (ink(&on, name), ink(&off, name));
            assert_ne!(lit, dim, "{name} did not fade");
            // Halved: `Visuals::disabled_alpha`.
            assert!(
                (i32::from(dim.a()) - i32::from(lit.a()) / 2).abs() <= 1,
                "{name} faded from {lit:?} to {dim:?}, which is not the 0.5 \
                 `disabled_alpha` says"
            );
        }
    }

    /// A gate inside a gate does not fade twice (`disable_unless`); the unguarded nesting does.
    #[test]
    fn nesting_a_disabled_gate_does_not_double_fade() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let alpha = |nest: bool, guarded: bool| -> u8 {
            let out = ctx.run_ui(Default::default(), |ui| {
                let body = |ui: &mut egui::Ui| {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(8.0, 8.0)),
                        egui::CornerRadius::ZERO,
                        color::accent(),
                    );
                };
                match (nest, guarded) {
                    (false, _) => {
                        ui.add_enabled_ui(false, body);
                    }
                    (true, true) => {
                        ui.add_enabled_ui(false, |ui| {
                            disable_unless(ui, false, body);
                        });
                    }
                    (true, false) => {
                        ui.add_enabled_ui(false, |ui| {
                            ui.add_enabled_ui(false, body);
                        });
                    }
                }
            });
            out.shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if r.fill.a() > 0 => Some(r.fill.a()),
                    _ => None,
                })
                .expect("the gated rect")
        };
        let once = alpha(false, false);
        assert!(once < 255, "the gate did nothing, so this proves nothing");
        assert_eq!(
            alpha(true, true),
            once,
            "a guarded gate inside a disabled parent faded a second time"
        );
        assert!(
            alpha(true, false) < once,
            "the unguarded nesting no longer double-fades, so the guard is moot \
             and `disable_unless` can go"
        );
    }

    /// Disabled is one ink however reached, and it clears both "switched off" tiers by `MARGIN` levels.
    #[test]
    fn unavailable_and_switched_off_are_different_greys() {
        const OFF_HIDDEN_ROW: u8 = 95; // HIDDEN_OPACITY (0.6) over muted()
        const OFF_LAYER_ROW: u8 = 102; // layers.rs, absolute
        const MARGIN: i32 = 15;

        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let faded = ctx.run_ui(Default::default(), |ui| {
            ui.add_enabled_ui(false, |ui| {
                icon_button(ui, theme::icon::EYE, 22.0, 16.0, false, true);
            });
        });
        let scoped = faded
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) => t
                    .galley
                    .rows
                    .iter()
                    .find_map(|r| r.row.visuals.mesh.vertices.first().map(|v| v.color.a())),
                _ => None,
            })
            .expect("the button's glyph");

        assert_eq!(
            scoped,
            theme::text::disabled().a(),
            "the two routes to `disabled` paint different inks: a faded scope gives \
             {scoped}, the constant {}",
            theme::text::disabled().a()
        );
        for (what, off) in [
            ("a hidden row", OFF_HIDDEN_ROW),
            ("a layer row", OFF_LAYER_ROW),
        ] {
            let gap = i32::from(off) - i32::from(theme::text::disabled().a());
            assert!(
                gap >= MARGIN,
                "{what} is off at {off} and a disabled control is at {}, only {gap} \
                 levels apart — dim alone cannot say which",
                theme::text::disabled().a()
            );
        }
    }

    /// A control disabled by its flag, by a scope, or by both paints the same ink.
    #[test]
    fn a_control_disabled_twice_over_is_no_darker_than_one_disabled_once() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let ink = |build: &dyn Fn(&mut egui::Ui)| -> u8 {
            ctx.run_ui(Default::default(), |ui| build(ui))
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) => t
                        .galley
                        .rows
                        .iter()
                        .find_map(|r| r.row.visuals.mesh.vertices.first().map(|v| v.color.a())),
                    _ => None,
                })
                .expect("the button's glyph")
        };
        let btn = |on: bool| {
            move |ui: &mut egui::Ui| {
                icon_button(ui, theme::icon::EYE, 22.0, 16.0, false, on);
            }
        };

        let by_flag = ink(&btn(false));
        let by_scope = ink(&|ui: &mut egui::Ui| {
            ui.add_enabled_ui(false, btn(true));
        });
        let by_both = ink(&|ui: &mut egui::Ui| {
            ui.add_enabled_ui(false, btn(false));
        });

        assert_eq!(
            by_flag, by_scope,
            "the two ways of disabling one button paint different inks"
        );
        assert_eq!(
            by_both, by_flag,
            "disabled by flag *and* by scope came out {by_both} against {by_flag} — \
             the two channels are compounding, so a row that uses both is uneven"
        );
    }

    /// Alpha the corner mask of a `swatch` was painted at, if one was; the mask uses the ground colour.
    fn mask_alpha(out: &egui::FullOutput, bg: egui::Color32) -> Option<u8> {
        out.shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Mesh(m) => m
                .vertices
                .iter()
                .find(|v| {
                    let (a, b) = (v.color.to_srgba_unmultiplied(), bg.to_srgba_unmultiplied());
                    a[0..3] == b[0..3]
                })
                .map(|v| v.color.a()),
            _ => None,
        })
    }

    /// A dimmed row fades but its swatch's corner mask stays opaque (`set_opacity(1.0)` escapes the enclosing `multiply_opacity`).
    #[test]
    fn a_dimmed_row_fades_everything_but_the_swatch() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let chip = egui::Color32::from_rgb(0xEB, 0x6E, 0x5A);
        let draw = |escape: bool| {
            ctx.run_ui(Default::default(), |ui| {
                ui.scope(|ui| {
                    ui.multiply_opacity(0.7);
                    ui.scope(|ui| {
                        if escape {
                            ui.set_opacity(1.0);
                        }
                        swatch(ui, 16.0, color::field(), Swatch::Solid(chip));
                    });
                });
            })
        };

        let faded = mask_alpha(&draw(false), color::field()).expect("a corner mask, faded");
        let kept = mask_alpha(&draw(true), color::field()).expect("a corner mask, escaped");
        assert!(
            faded < 255,
            "the dim did nothing, so this test proves nothing: mask alpha {faded}"
        );
        assert_eq!(
            kept, 255,
            "the swatch's corner mask must stay opaque inside a dimmed row"
        );
    }
}

#[cfg(test)]
mod swatch_hairline_tests {
    use super::*;

    /// One swatch's frame in the dark theme, plus the rect it allocated.
    fn draw(chip: egui::Color32) -> (egui::FullOutput, egui::Rect) {
        draw_in(chip, crate::theme::color::Theme::Dark)
    }

    /// One swatch's frame in `theme`, plus the rect it allocated.
    fn draw_in(
        chip: egui::Color32,
        theme: crate::theme::color::Theme,
    ) -> (egui::FullOutput, egui::Rect) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        crate::theme::apply(&ctx, theme);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut rect = egui::Rect::NOTHING;
        let out = ctx.run_ui(Default::default(), |ui| {
            rect = swatch(ui, 16.0, color::field(), Swatch::Solid(chip)).rect;
        });
        (out, rect)
    }

    /// The chip's hairline (the only stroked rect) and its index in the frame.
    fn hairline(out: &egui::FullOutput) -> (usize, &egui::epaint::RectShape) {
        let mut found = out
            .shapes
            .iter()
            .enumerate()
            .filter_map(|(i, c)| match &c.shape {
                egui::Shape::Rect(r) if r.stroke.width > 0.0 => Some((i, r)),
                _ => None,
            });
        let one = found.next().expect("the chip's hairline: a stroked rect");
        assert!(found.next().is_none(), "expected exactly one stroked rect");
        one
    }

    /// `top` composited over an opaque `under`, in gamma space (the GPU blends linearly).
    fn over(top: egui::Color32, under: egui::Color32) -> egui::Color32 {
        let (t, u) = (top.to_srgba_unmultiplied(), under.to_srgba_unmultiplied());
        let a = t[3] as f32 / 255.0;
        let mix = |i: usize| (t[i] as f32 * a + u[i] as f32 * (1.0 - a)).round() as u8;
        egui::Color32::from_rgb(mix(0), mix(1), mix(2))
    }

    /// The largest per-channel gap between two opaque colours, in 8-bit levels.
    fn apart(a: egui::Color32, b: egui::Color32) -> i32 {
        (0..3)
            .map(|i| {
                (a.to_srgba_unmultiplied()[i] as i32 - b.to_srgba_unmultiplied()[i] as i32).abs()
            })
            .max()
            .unwrap()
    }

    /// A chip the colour of its row stays visible in both themes: `2F2F2F` and
    /// black (which sets `MARGIN`) on the dark field, white and `F1F1F2` on the
    /// light one.
    ///
    /// flip: `from_black_alpha(76)` for the dark hairline → fails the margin on the
    /// `2F2F2F` chip; `from_white_alpha(76)` for the light one → fails on white.
    #[test]
    fn a_chip_the_colour_of_its_row_is_still_visible() {
        use crate::theme::color::Theme;
        const MARGIN: i32 = 25;
        for (theme, name, chip) in [
            (
                Theme::Dark,
                "the reported 2F2F2F",
                egui::Color32::from_rgb(0x2F, 0x2F, 0x2F),
            ),
            (Theme::Dark, "black", egui::Color32::BLACK),
            (Theme::Light, "white", egui::Color32::WHITE),
            (
                Theme::Light,
                "the light field's F1F1F2",
                egui::Color32::from_rgb(0xF1, 0xF1, 0xF2),
            ),
        ] {
            let (out, _) = draw_in(chip, theme);
            let (_, line) = hairline(&out);
            let edge = over(line.stroke.color, chip);
            assert!(
                apart(edge, color::field()) >= MARGIN,
                "{theme:?}, {name}: the chip's edge composites to {edge:?}, only {} levels \
                 from the row's own {:?} — the chip reads as a hole in the row",
                apart(edge, color::field()),
                color::field(),
            );
        }
    }

    /// The hairline is painted after the corner mask (or the mask bites it) and stays inside the chip's rect.
    #[test]
    fn the_hairline_goes_over_the_mask_and_does_not_grow_the_chip() {
        let (out, rect) = draw(egui::Color32::from_rgb(0xEB, 0x6E, 0x5A));
        let mask = out
            .shapes
            .iter()
            .position(|c| matches!(c.shape, egui::Shape::Mesh(_)))
            .expect("the corner mask");
        let (at, line) = hairline(&out);
        assert!(
            at > mask,
            "the hairline is painted at {at} and the corner mask at {mask}, \
             so the mask bites four pieces out of it"
        );
        let grew = egui::Shape::Rect(line.clone()).visual_bounding_rect();
        assert!(
            grew.min.x >= rect.min.x && grew.max.x <= rect.max.x,
            "the hairline reaches {grew:?}, outside the {rect:?} the chip allocated"
        );
    }
}

#[cfg(test)]
mod segment_border_tests {
    use super::*;

    /// A `segmented` track wears the field hairline and its cells sit inside it.
    ///
    /// flip: dropping the `rect_stroke` → fails at `hairline`. Leaving `inner` at `track.shrink(PAD)` → fails on the cell's height (24 against 22), not its position.
    #[test]
    fn a_segmented_track_wears_the_fields_hairline_and_its_cells_sit_inside_it() {
        const CELL_H: f32 = 24.0;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            segmented(ui, 120.0, CELL_H, 3, 0, |_, _, _, _| {});
        });

        let rects: Vec<&egui::epaint::RectShape> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) => Some(r),
                _ => None,
            })
            .collect();
        // The track is the only rect `CELL_H + 4` tall.
        let track = rects
            .iter()
            .find(|r| (r.rect.height() - (CELL_H + 4.0)).abs() < 0.01)
            .map(|r| r.rect)
            .expect("a track");
        let hairline = rects
            .iter()
            .find(|r| r.rect == track && r.stroke.width > 0.0)
            .expect("the track's hairline");
        assert_eq!(
            hairline.stroke.width, SEGMENT_BORDER,
            "the hairline is {}pt where every other border in the chrome is {SEGMENT_BORDER}",
            hairline.stroke.width
        );
        assert_eq!(
            hairline.stroke.color,
            theme::color::field_border(),
            "the hairline every other control wears, so a track beside a field \
             reads as one family"
        );

        // The raised cell is `1 + 2` in from the track at both ends.
        let cell = rects
            .iter()
            .find(|r| r.rect.height() < track.height() && r.fill != egui::Color32::TRANSPARENT)
            .map(|r| r.rect)
            .expect("the selected cell");
        assert!(
            (cell.height() - (CELL_H - 2.0)).abs() < 0.01,
            "the cell paints {}pt inside a {}pt track, which leaves it lying on \
             the hairline rather than inside it",
            cell.height(),
            track.height()
        );
        assert!(
            (cell.min.x - (track.min.x + 3.0)).abs() < 0.01,
            "the first cell starts {}pt in from the track, not the border's 1 plus \
             the padding's 2",
            cell.min.x - track.min.x
        );
    }
}

#[cfg(test)]
mod transform_row_tests {
    use super::*;

    /// The inspector card's content width: a 284px card less `CARD_MARGIN_X` either side.
    const CARD: f32 = 284.0 - CARD_MARGIN_X * 2.0;
    const GAP: f32 = 7.0;
    const H: f32 = 28.0;

    /// Every field ground laid out by `body`, left to right.
    fn grounds(mut body: impl FnMut(&mut egui::Ui)) -> Vec<egui::Rect> {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_max_width(CARD);
            ui.set_min_width(CARD);
            body(ui);
        });
        let mut rects: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill == color::field() => Some(r.rect),
                _ => None,
            })
            .collect();
        rects.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        rects
    }

    /// A unit longer than its field leaves is cut and the field keeps its width (a negative number box panics egui).
    ///
    /// flip: `Extend` back in the unit's `into_galley` → fails the width assertion (124 for 90).
    #[test]
    fn a_unit_wider_than_its_field_is_cut_and_the_field_keeps_its_width() {
        let w = 90.0;
        let rects = grounds(|ui| {
            let mut v = 120.0_f64;
            value_field_suffixed(
                ui,
                egui::vec2(w, H),
                Prefix::Text("H"),
                Some(Suffix {
                    text: "fit content",
                    clickable: true,
                    tooltip: "",
                }),
                &mut v,
                Scrub::whole(0.5),
                |d| d,
            );
        });
        assert_eq!(rects.len(), 1, "one field ground: {rects:?}");
        assert!(
            rects[0].width() <= w + 0.5,
            "the field grew to {} for a {w} slot",
            rects[0].width()
        );
    }

    /// Tab goes from one field's digits to the next field's, then to a clickable
    /// unit, and stops on no other rect (the prefix strip).
    ///
    /// `None` in the sweep is egui's own end-of-list gap.
    #[test]
    fn tabbing_a_row_of_fields_steps_from_digits_to_digits() {
        let (order, unknown) = tab_sweep();
        assert!(
            unknown.is_empty(),
            "tab stopped on {} rect(s) that are not a field's digits or unit: {unknown:?}",
            unknown.len()
        );
        assert_eq!(
            order.iter().flatten().take(3).collect::<Vec<_>>(),
            ["X digits", "Y digits", "Y unit"],
            "X should reach Y in one press, and its unit in the next; full sweep was {order:?}"
        );
    }

    /// Six Tab presses over a plain field and a suffixed one: what was focused
    /// after each, and every focused rect that was neither field's digits.
    fn tab_sweep() -> (Vec<Option<String>>, Vec<String>) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let fw = (CARD - GAP) / 2.0;
        let mut ids = Vec::new();
        let frame = |events: Vec<egui::Event>, ids: &mut Vec<(egui::Id, String)>| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_width(CARD);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = GAP;
                        let mut a = 96.0_f64;
                        let rx = value_field(
                            ui,
                            egui::vec2(fw, H),
                            Prefix::Text("X"),
                            &mut a,
                            Scrub::whole(0.5),
                            |d| d,
                        );
                        let (ry, unit) = value_field_unit(
                            ui,
                            egui::vec2(fw, H),
                            Prefix::Text("Y"),
                            Some(Suffix {
                                text: "px",
                                clickable: true,
                                tooltip: "",
                            }),
                            &mut a,
                            Scrub::whole(0.5),
                            |d| d,
                        );
                        if ids.is_empty() {
                            ids.push((rx.id, "X digits".into()));
                            ids.push((ry.id, "Y digits".into()));
                            ids.push((unit.expect("clickable").id, "Y unit".into()));
                        }
                    });
                },
            );
        };
        frame(Vec::new(), &mut ids);
        let tab = || egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        let (mut order, mut unknown) = (Vec::new(), Vec::new());
        for _ in 0..6 {
            frame(vec![tab()], &mut ids);
            let name = ctx.memory(|m| m.focused()).map(|id| {
                ids.iter()
                    .find(|(i, _)| *i == id)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| {
                        // Name the rect and sense: the offender draws nothing, so an `Id` alone says little.
                        let what = ctx
                            .read_response(id)
                            .map(|r| format!("{:?} {:?}", r.rect, r.sense))
                            .unwrap_or_else(|| "no response".into());
                        unknown.push(what.clone());
                        what
                    })
            });
            order.push(name);
        }
        (order, unknown)
    }

    /// W is exactly as wide as X and the W/H/chain row reaches the card's far edge (`FIELD_BORDER_H`).
    #[test]
    fn w_lines_up_with_x_and_the_row_fills_the_card() {
        let fw = (CARD - GAP) / 2.0;

        let xy = grounds(|ui| {
            let mut a = 96.0_f64;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for _ in 0..2 {
                    value_field(
                        ui,
                        egui::vec2(fw, H),
                        Prefix::Text("X"),
                        &mut a,
                        Scrub::whole(0.5),
                        |d| d,
                    );
                }
            });
        });
        let wh = grounds(|ui| {
            let mut a = 528.0_f64;
            let rest = ui.available_width() - fw - H - GAP * 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                value_field(
                    ui,
                    egui::vec2(fw, H),
                    Prefix::Text("W"),
                    &mut a,
                    Scrub::whole(0.5),
                    |d| d,
                );
                value_field(
                    ui,
                    egui::vec2(rest, H),
                    Prefix::Text("H"),
                    &mut a,
                    Scrub::whole(0.5),
                    |d| d,
                );
                field_button(ui, icon::ASPECT_RATIO, H, FieldButton::Off);
            });
        });

        assert_eq!(xy.len(), 2, "X and Y: {xy:?}");
        assert_eq!(wh.len(), 3, "W, H and the chain: {wh:?}");
        assert_eq!(xy[0].width(), wh[0].width(), "W is not as wide as X");
        assert_eq!(xy[0].max.x, wh[0].max.x, "W's right edge is not X's");
        assert!(
            (xy[1].max.x - wh[2].max.x).abs() < 0.01,
            "the rows do not end together: {:?} vs {:?}",
            xy[1],
            wh[2]
        );
    }

    /// The four quarter-turn and mirror buttons split the card evenly and fill it.
    #[test]
    fn the_four_transform_buttons_split_the_card_evenly() {
        let bs = grounds(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                let quarter = (ui.available_width() - 3.0 * GAP) / 4.0;
                for g in [
                    icon::ROTATE_LEFT,
                    icon::ROTATE_RIGHT,
                    icon::FLIP_HORIZONTAL,
                    icon::FLIP_VERTICAL,
                ] {
                    field_button_sized(ui, g, egui::vec2(quarter, H), FieldButton::Off);
                }
            });
        });
        assert_eq!(bs.len(), 4, "four buttons: {bs:?}");
        for b in &bs {
            assert_eq!(b.width(), bs[0].width(), "uneven: {bs:?}");
            assert_eq!(b.height(), H, "{b:?} is not {H} tall");
        }
        assert!(
            (bs[3].max.x - bs[0].min.x - CARD).abs() < 0.01,
            "the row does not fill the card: {:?}..{:?} of {CARD}",
            bs[0].min.x,
            bs[3].max.x
        );
    }

    /// A paint row leaves exactly the eye button's width beside it and shares its baseline (`FIELD_BORDER_H`).
    #[test]
    fn a_paint_row_and_its_eye_fill_the_card_on_one_baseline() {
        const ROW_GAP: f32 = 6.0;
        let bs = grounds(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ROW_GAP;
                let size = egui::vec2(ui.available_width() - H - ROW_GAP, H);
                field_row(ui, size, |ui| {
                    // As the real row: a `field_row` shrink-wraps, so the right-aligned group fills it.
                    ui.label("E9E9ED");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label("100%");
                    });
                });
                field_button(ui, icon::EYE, H, FieldButton::Off);
            });
        });
        assert_eq!(bs.len(), 2, "the row and its eye: {bs:?}");
        assert_eq!(bs[1].width(), H, "the eye is not square");
        assert_eq!(bs[0].min.y, bs[1].min.y, "off the row's baseline: {bs:?}");
        assert_eq!(bs[0].height(), bs[1].height(), "different heights: {bs:?}");
        assert!(
            (bs[1].max.x - bs[0].min.x - CARD).abs() < 0.01,
            "the row does not fill the card: {bs:?}"
        );
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    /// Selected digits stay opaque in every helper that strips a `DragValue`'s chrome.
    ///
    /// `selection.stroke` is also the colour selected glyphs are repainted in, so a transparent `Stroke::NONE` hides them.
    #[test]
    fn selected_digits_are_never_painted_transparent() {
        for what in ["value_field", "bare_drag_value", "badge_field"] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let _ = ctx.run_ui(Default::default(), |_| {});

            let mut v = 12345.0_f64;
            let field = |ui: &mut egui::Ui, v: &mut f64| match what {
                "bare_drag_value" => {
                    let dv = egui::DragValue::new(v).custom_formatter(number(2));
                    field_row(ui, egui::vec2(120.0, 28.0), |ui| bare_drag_value(ui, dv))
                }
                "badge_field" => badge_field(ui, "W", 120.0, v, Scrub::whole(0.5), |d| {
                    d.custom_formatter(number(2))
                }),
                _ => value_field(
                    ui,
                    egui::vec2(120.0, 28.0),
                    Prefix::Icon(icon::LINE_SEGMENT),
                    v,
                    Scrub::whole(0.5),
                    |d| d.custom_formatter(number(2)),
                ),
            };

            // A double-click puts a `DragValue` into keyboard editing.
            let mut rect = egui::Rect::NOTHING;
            let _ = ctx.run_ui(Default::default(), |ui| rect = field(ui, &mut v).rect);
            let c = rect.center();
            let click = |pressed| egui::Event::PointerButton {
                pos: c,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let key = |k, modifiers| egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            for evs in [
                vec![egui::Event::PointerMoved(c), click(true), click(false)],
                vec![click(true), click(false)],
                // Home, then Shift+End: select everything.
                vec![
                    key(egui::Key::Home, egui::Modifiers::NONE),
                    key(egui::Key::End, egui::Modifiers::SHIFT),
                ],
            ] {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        events: evs,
                        ..Default::default()
                    },
                    |ui| {
                        field(ui, &mut v);
                    },
                );
            }

            let out = ctx.run_ui(Default::default(), |ui| {
                field(ui, &mut v);
            });
            let digits = out
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == "12345" => Some(t.galley.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{what}: the field's digits were never painted"));
            let row = &digits.rows[0].row;

            // The selection fill is in this same mesh; without it the opaque-glyph check proves nothing.
            let fill = ctx.style_of(egui::Theme::Dark).visuals.selection.bg_fill;
            let selected = row
                .visuals
                .mesh
                .vertices
                .iter()
                .any(|v| v.color == fill && fill.a() > 0);
            assert!(
                selected,
                "{what}: nothing was selected, so this test proves nothing"
            );

            let glyphs = &row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()];
            assert!(
                glyphs.iter().any(|v| v.color.a() > 0),
                "{what}: every selected glyph was painted transparent — the digits are \
                 invisible behind the selection fill"
            );
        }
    }
}

#[cfg(test)]
mod scrub_tests {
    use super::*;
    use crate::theme::icon;

    /// Drags `dx` horizontally from the prefix or the digits and returns the resulting value.
    fn drag(scrub: Scrub, start_in_prefix: bool, dx: f32) -> f64 {
        drag_in_steps(scrub, start_in_prefix, dx, 1)
    }

    /// `drag`, with the travel split over `steps` passes, as a slow hand delivers it.
    fn drag_in_steps(scrub: Scrub, start_in_prefix: bool, dx: f32, steps: u32) -> f64 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut v = 100.0_f64;
        let mut field = |ctx: &egui::Context, events: Vec<egui::Event>| {
            let out = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    value_field(
                        ui,
                        egui::vec2(120.0, 28.0),
                        Prefix::Icon(icon::LINE_SEGMENT),
                        &mut v,
                        scrub.clone(),
                        |d| d.custom_formatter(number(2)),
                    );
                },
            );
            // The painted ground, not the response rect: that shrinks if the prefix strip stops sensing.
            out.shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Rect(r) if (r.rect.height() - 28.0).abs() < 0.51 => Some(r.rect),
                    _ => None,
                })
                .expect("the field paints its ground")
        };
        field(&ctx, Vec::new());
        let rect = field(&ctx, Vec::new());
        let from = if start_in_prefix {
            egui::pos2(rect.left() + FIELD_PAD_X + 4.0, rect.center().y)
        } else {
            rect.center()
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let to = egui::pos2(from.x + dx, from.y);
        field(
            &ctx,
            vec![egui::Event::PointerMoved(from), button(from, true)],
        );
        for step in 1..=steps {
            let at = egui::pos2(from.x + dx * step as f32 / steps as f32, from.y);
            field(&ctx, vec![egui::Event::PointerMoved(at)]);
        }
        field(&ctx, vec![button(to, false)]);
        v
    }

    /// A scrub starting on the prefix strip lands where one starting on the digits does; the two are different arithmetic paths.
    #[test]
    fn a_scrub_starting_on_the_prefix_moves_the_value_the_same_way() {
        // 37px is not a whole number of steps.
        for scrub in [Scrub::whole(0.5), Scrub::whole(0.25), Scrub::fine(0.02, 2)] {
            let digits = drag(scrub.clone(), false, 37.0);
            let prefix = drag(scrub.clone(), true, 37.0);
            assert_ne!(digits, 100.0, "{scrub:?}: the digits did not scrub at all");
            assert_eq!(
                digits, prefix,
                "{scrub:?}: dragging the prefix landed somewhere else than dragging \
                 the number"
            );
        }
    }

    /// A slow hand on a slow field's prefix moves the value as far as on its digits, though no single pass makes a whole unit.
    #[test]
    fn a_slow_scrub_on_the_prefix_keeps_its_fraction_between_passes() {
        // 0.05 a point: 2pt a pass is a tenth of a unit, 80pt is four units.
        let scrub = Scrub::whole(0.1);
        let digits = drag_in_steps(scrub.clone(), false, 80.0, 40);
        let prefix = drag_in_steps(scrub.clone(), true, 80.0, 40);
        assert_eq!(
            digits, 104.0,
            "the digits are the reference and moved {digits}"
        );
        assert_eq!(prefix, digits, "the prefix strip dropped its slivers");
    }

    /// Scrubbing a `Scrub::whole` field lands on whole units, while a fine field keeps its fraction.
    #[test]
    fn scrubbing_a_whole_field_never_lands_on_a_fraction() {
        for dx in [7.0_f32, 13.0, 37.0, -23.0, 101.0] {
            for start_in_prefix in [false, true] {
                let got = drag(Scrub::whole(0.5), start_in_prefix, dx);
                assert_eq!(
                    got,
                    got.round(),
                    "a {dx}px drag (prefix: {start_in_prefix}) left {got}"
                );
            }
        }
        let fine = drag(Scrub::fine(0.02, 2), false, 37.0);
        assert_ne!(fine, fine.round(), "a fine field lost its fraction: {fine}");
    }

    /// `Scrub::settle` rounds a drag to whole units and clamps after rounding; typing is never rounded.
    #[test]
    fn a_whole_field_still_accepts_a_typed_fraction() {
        let scrub = Scrub::whole(0.5);
        assert_eq!(scrub.settle(109.25), 109.0, "a drag settles");
        let bounded = Scrub::whole(0.5).range(0.0..=100.0);
        assert_eq!(bounded.settle(150.0), 100.0);
        assert_eq!(bounded.settle(-5.0), 0.0);
        let fractional = Scrub::whole(0.5).range(0.0..=99.4);
        assert_eq!(fractional.settle(99.4), 99.0);
    }

    /// Two fields in one row share no widget id, asserted through egui's clash overlay.
    #[test]
    fn a_row_of_value_fields_has_no_id_clash() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        // Set, not read: egui defaults this to `cfg!(debug_assertions)`.
        ctx.options_mut(|o| o.warn_on_id_clash = true);
        assert!(
            ctx.options(|o| o.warn_on_id_clash),
            "the clash warning is off, so this test cannot fail"
        );

        let mut a = 80.0_f64;
        let mut b = 80.0_f64;
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_max_width(300.0);
            ui.horizontal(|ui| {
                for (p, v) in [(Prefix::Text("X"), &mut a), (Prefix::Text("Y"), &mut b)] {
                    value_field(ui, egui::vec2(120.0, 28.0), p, v, Scrub::whole(0.5), |d| {
                        d.custom_formatter(number(2))
                    });
                }
            });
        });

        let complaints: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text().contains("use of widget") => {
                    Some(t.galley.text().to_string())
                }
                _ => None,
            })
            .collect();
        assert!(
            complaints.is_empty(),
            "egui reported an id clash between two fields in one row: {complaints:?}"
        );
    }

    /// Each field's prefix strip scrubs only its own number.
    #[test]
    fn each_fields_prefix_scrubs_only_its_own_number() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let (mut a, mut b) = (80.0_f64, 80.0_f64);
        let run = |events: Vec<egui::Event>, a: &mut f64, b: &mut f64| {
            let out = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_width(300.0);
                    ui.horizontal(|ui| {
                        for (p, v) in [(Prefix::Text("X"), &mut *a), (Prefix::Text("Y"), &mut *b)] {
                            value_field(
                                ui,
                                egui::vec2(120.0, 28.0),
                                p,
                                v,
                                Scrub::whole(0.5),
                                |d| d.custom_formatter(number(2)),
                            );
                        }
                    });
                },
            );
            out.shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Rect(r) if (r.rect.height() - 28.0).abs() < 0.51 => Some(r.rect),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        run(Vec::new(), &mut a, &mut b);
        let grounds = run(Vec::new(), &mut a, &mut b);
        assert_eq!(grounds.len(), 2, "expected two field grounds: {grounds:?}");

        // The second field's prefix, so a mis-wired strip moves the first field.
        let from = egui::pos2(grounds[1].left() + FIELD_PAD_X + 4.0, grounds[1].center().y);
        let to = egui::pos2(from.x + 40.0, from.y);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(
            vec![egui::Event::PointerMoved(from), button(from, true)],
            &mut a,
            &mut b,
        );
        run(vec![egui::Event::PointerMoved(to)], &mut a, &mut b);
        run(vec![button(to, false)], &mut a, &mut b);

        assert_eq!(a, 80.0, "the first field moved when the second was dragged");
        assert!(b > 80.0, "the second field's prefix did not scrub it: {b}");
    }

    /// A fine field keeps the tenths of the star's 38.2% ratio under a drag, where a whole field destroys them.
    #[test]
    fn a_fine_field_keeps_the_tenths_a_whole_one_destroys() {
        /// Drags 8px right from `start` and collects the distinct values passed through.
        fn sweep(scrub: Scrub, start: f64) -> Vec<f64> {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let _ = ctx.run_ui(Default::default(), |_| {});
            let mut v = start;
            let mut seen = Vec::new();
            let run = |events: Vec<egui::Event>, v: &mut f64| {
                let out = ctx.run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        value_field(
                            ui,
                            egui::vec2(120.0, 28.0),
                            Prefix::Icon(crate::theme::icon::ANGLE),
                            v,
                            scrub.clone(),
                            |d| d.suffix("%").max_decimals(1),
                        );
                    },
                );
                out.shapes.iter().find_map(|c| match &c.shape {
                    egui::Shape::Rect(r) if (r.rect.height() - 28.0).abs() < 0.51 => Some(r.rect),
                    _ => None,
                })
            };
            run(Vec::new(), &mut v);
            let rect = run(Vec::new(), &mut v).expect("the field paints its ground");
            let from = rect.center();
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            run(
                vec![egui::Event::PointerMoved(from), button(from, true)],
                &mut v,
            );
            for px in 1..=8 {
                run(
                    vec![egui::Event::PointerMoved(egui::pos2(
                        from.x + px as f32,
                        from.y,
                    ))],
                    &mut v,
                );
                if seen.last() != Some(&v) {
                    seen.push(v);
                }
            }
            seen
        }

        // The inner-ratio field: every step stays on a tenth and the golden section survives.
        let fine = sweep(Scrub::fine(0.25, 1).range(5.0..=100.0), 38.2);
        for v in &fine {
            assert!(
                ((v * 10.0).round() - v * 10.0).abs() < 1e-9,
                "a tenths field produced {v}: {fine:?}"
            );
        }
        assert!(
            fine.iter().any(|v| (v - 38.2).abs() < 1e-9),
            "the golden section did not survive the start of the drag: {fine:?}"
        );

        // The first entry is the value before egui's drag threshold, so assert on what follows.
        let whole = sweep(Scrub::whole(0.25).range(5.0..=100.0), 38.2);
        let after: Vec<f64> = whole.iter().copied().skip(1).collect();
        assert!(!after.is_empty(), "the whole field never moved: {whole:?}");
        assert!(
            after.iter().all(|v| *v == v.round()),
            "a whole field produced a fraction: {whole:?}"
        );
        assert!(
            !after.iter().any(|v| (v - 38.2).abs() < 1e-9),
            "the whole rule was supposed to lose the golden section: {whole:?}"
        );
        assert_ne!(fine, whole, "the two rules are indistinguishable here");
    }

    /// An inverted `Scrub` range is swapped (as egui does) rather than panicking in `f64::clamp`; a NaN bound is dropped.
    ///
    /// A document-derived bound (an image fill's Crop scale) can invert.
    ///
    /// flip: restoring `self.range = range` → the first assertion panics inside `f64::clamp`.
    #[test]
    fn an_inverted_scrub_range_is_put_the_right_way_round_rather_than_fatal() {
        let inverted = Scrub::fine(0.5, 2).range(12_000.0..=10_000.0);
        assert_eq!(
            inverted.settle(0.0),
            10_000.0,
            "the floor is the smaller of the two, not the one written first"
        );
        assert_eq!(
            inverted.settle(13_000.0),
            12_000.0,
            "and the ceiling is the larger"
        );
        assert_eq!(
            inverted.settle(11_000.0),
            11_000.0,
            "so the interval between them is what the range means"
        );
        // Swapping a `NaN` bound would pin the field to one number.
        let nanny = Scrub::fine(0.5, 2).range(f64::NAN..=10_000.0);
        assert_eq!(
            nanny.settle(99_999.0),
            99_999.0,
            "no bound, not a pinned one"
        );

        assert_eq!(
            Scrub::fine(0.5, 2).range(0.0..=10_000.0).settle(12_000.0),
            10_000.0
        );

        let landed = drag(Scrub::fine(0.5, 2).range(12_000.0..=10_000.0), false, 24.0);
        assert!(
            landed.is_finite(),
            "a drag on a field with an inverted range must return a number: {landed}"
        );
        let on_prefix = drag(Scrub::fine(0.5, 2).range(12_000.0..=10_000.0), true, 24.0);
        assert!(
            on_prefix.is_finite(),
            "and so must one started on the prefix strip, which is the other door \
             into `settle`: {on_prefix}"
        );
    }
}

#[cfg(test)]
mod combo_chevron_tests {
    use super::*;

    /// Draws a closed `ComboBox`, with the app's icon or without, and returns its shapes.
    fn shapes(with_icon: bool) -> Vec<egui::epaint::Shape> {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            let combo = egui::ComboBox::from_id_salt("probe").selected_text("Value");
            let combo = if with_icon {
                combo.icon(combo_chevron)
            } else {
                combo
            };
            combo.show_ui(ui, |_| {});
        });
        out.shapes.into_iter().map(|c| c.shape).collect()
    }

    /// A closed dropdown paints one chevron glyph at `COMBO_CHEVRON_PT` in `combo_chevron_ink()`, and not egui's filled triangle.
    #[test]
    fn a_dropdown_closes_with_a_chevron_and_not_egui_s_triangle() {
        let painted = shapes(true);
        let caret: Vec<&egui::epaint::TextShape> = painted
            .iter()
            .filter_map(|s| match s {
                egui::epaint::Shape::Text(t) => Some(t),
                _ => None,
            })
            .filter(|t| t.galley.job.text.contains(icon::CARET_DOWN))
            .collect();
        assert_eq!(caret.len(), 1, "exactly one chevron on a closed dropdown");
        let format = &caret[0].galley.job.sections[0].format;
        assert_eq!(format.font_id.size, COMBO_CHEVRON_PT);
        assert_eq!(format.color, combo_chevron_ink());

        assert!(
            !painted.iter().any(is_triangle),
            "egui's own arrow is a 3-point convex polygon; none should be drawn"
        );
        assert!(
            shapes(false).iter().any(is_triangle),
            "…and without the icon it is, which is what this test is about"
        );
    }

    /// An open dropdown's list sits `DROPDOWN_GAP` off its field, below it near the
    /// top of the screen and above it near the bottom.
    /// flip: `menu_rows` without the layer translation → fails at "below".
    #[test]
    fn a_dropdown_list_opens_a_gap_off_its_field() {
        /// The field's rect and the list's on screen, with the field at `y`.
        fn open_at(y: f32) -> (egui::Rect, egui::Rect) {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let (mut field, mut layer) = (egui::Rect::NOTHING, None);
            for frame in 0..6 {
                let _ = ctx.run_ui(input(), |ui| {
                    ui.add_space(y);
                    let r = egui::ComboBox::from_id_salt("probe")
                        .selected_text("Value")
                        .show_ui(ui, |ui| {
                            menu_rows(ui);
                            for i in 0..8 {
                                ui.label(format!("Row {i}"));
                            }
                            layer = Some(ui.layer_id());
                        });
                    field = r.response.rect;
                    if frame == 0 {
                        egui::Popup::open_id(ui.ctx(), r.response.id.with("popup"));
                    }
                });
            }
            let layer = layer.expect("the list was shown");
            let rect = ctx
                .memory(|m| m.area_rect(layer.id))
                .expect("the list has an area");
            let to_global = ctx.layer_transform_to_global(layer).unwrap_or_default();
            (field, to_global * rect)
        }

        let (field, list) = open_at(40.0);
        assert_eq!(
            list.top() - field.bottom(),
            DROPDOWN_GAP,
            "below: {list:?} under {field:?}"
        );
        let (field, list) = open_at(560.0);
        assert_eq!(
            field.top() - list.bottom(),
            DROPDOWN_GAP,
            "above: {list:?} over {field:?}"
        );
    }

    fn is_triangle(shape: &egui::epaint::Shape) -> bool {
        matches!(shape, egui::epaint::Shape::Path(p) if p.points.len() == 3 && p.fill != egui::Color32::TRANSPARENT)
    }
}

#[cfg(test)]
mod combo_height_tests {
    use super::*;

    /// The height a `ComboBox` paints with `interact_size.y` set to `y`, or the theme's own.
    fn painted(y: Option<f32>) -> f32 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut out = 0.0;
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.scope(|ui| {
                if let Some(y) = y {
                    ui.spacing_mut().interact_size.y = y;
                    ui.spacing_mut().button_padding.y = 0.0;
                }
                out = egui::ComboBox::from_id_salt("probe")
                    .icon(combo_chevron)
                    .width(268.0)
                    .selected_text(glyph_and_text(icon::TEXT_AA, "Roboto"))
                    .show_ui(ui, |_| {})
                    .response
                    .rect
                    .height();
            });
        });
        out
    }

    /// A combo paints exactly `interact_size.y` (its border is inside its rect, so `FIELD_BORDER_H` is not subtracted); the theme default is 24.
    #[test]
    fn a_combos_height_is_its_interact_size_and_the_hairline_is_not_subtracted() {
        assert_eq!(painted(Some(28.0)), 28.0, "a 28px row asks for 28");
        assert_eq!(
            painted(Some(28.0 - FIELD_BORDER_H)),
            26.0,
            "subtracting the hairline is how the variant field came out 2px short"
        );
        assert_eq!(
            painted(None),
            24.0,
            "and saying nothing is how the family field came out 4px short"
        );
    }
}

#[cfg(test)]
mod meter_tests {
    use super::*;

    /// One painted `value_field_metered` as its ground, zero tick and bar rects, from `.shapes`.
    ///
    /// The tick and bar share a colour and are taken in paint order (the tick first).
    fn parts(frac: f32) -> (egui::Rect, egui::Rect, egui::Rect) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut value = 40.0_f64;
        let out = ctx.run_ui(Default::default(), |ui| {
            value_field_metered(
                ui,
                egui::vec2(180.0, 26.0),
                Prefix::Text("Contrast"),
                frac,
                &mut value,
                Scrub::whole(0.25).range(-100.0..=100.0),
                |d| d,
            );
        });
        let rects = |want: egui::Color32| -> Vec<egui::Rect> {
            out.shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Rect(r) if r.fill == want => Some(r.rect),
                    _ => None,
                })
                .collect()
        };
        let wash = rects(meter_wash());
        assert_eq!(
            wash.len(),
            2,
            "a metered field paints a tick and a bar, in that order"
        );
        (
            rects(color::field())
                .first()
                .copied()
                .expect("the field paints a ground"),
            wash[0],
            wash[1],
        )
    }

    /// The bar grows from the middle and the two signs mirror (one sign alone also fits a left-anchored bar).
    #[test]
    fn the_meter_grows_from_the_middle_and_mirrors_across_it() {
        let (ground, _, plus) = parts(0.5);
        let (_, _, minus) = parts(-0.5);
        // Inside the hairline: the bar clears half of `FIELD_BORDER_H` at each edge.
        let ink = ground.shrink(FIELD_BORDER_H / 2.0);
        let mid = ink.center().x;

        assert!(
            ground.contains_rect(plus) && ground.contains_rect(minus),
            "a bar escaped its field: +{plus:?} −{minus:?} in ground {ground:?}"
        );
        assert!(
            (plus.left() - mid).abs() < 0.51 && (minus.right() - mid).abs() < 0.51,
            "the bars must meet at the field's middle ({mid}): + starts at {} and \
             − ends at {}",
            plus.left(),
            minus.right()
        );
        // Half the travel is a quarter of the field.
        for (name, bar) in [("+0.5", plus), ("-0.5", minus)] {
            assert!(
                (bar.width() - ink.width() * 0.25).abs() < 0.51,
                "a bar at {name} is {} wide where a quarter of the field is {}",
                bar.width(),
                ink.width() * 0.25
            );
            assert!(
                (bar.height() - ink.height()).abs() < 0.51,
                "the {name} bar is {} tall in {} of field ink",
                bar.height(),
                ink.height()
            );
        }
    }

    /// A metered field's value is right-aligned (past the field's middle); a plain field's sits by its prefix.
    #[test]
    fn a_metered_fields_value_sits_at_the_far_end_where_a_plain_ones_sits_by_its_label() {
        fn digits_x(metered: bool) -> (f32, egui::Rect) {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let _ = ctx.run_ui(Default::default(), |_| {});
            let mut value = 40.0_f64;
            let mut field = egui::Rect::ZERO;
            let out = ctx.run_ui(Default::default(), |ui| {
                let size = egui::vec2(180.0, 26.0);
                field = match metered {
                    true => value_field_metered(
                        ui,
                        size,
                        Prefix::Text("Contrast"),
                        0.4,
                        &mut value,
                        Scrub::whole(0.25).range(-100.0..=100.0),
                        |d| d,
                    ),
                    false => value_field(
                        ui,
                        size,
                        Prefix::Text("Contrast"),
                        &mut value,
                        Scrub::whole(0.25).range(-100.0..=100.0),
                        |d| d,
                    ),
                }
                .rect;
            });
            // The number is the rightmost text; the prefix is laid out first.
            let x = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.pos.x),
                    _ => None,
                })
                .fold(f32::NEG_INFINITY, f32::max);
            (x, field)
        }

        let (metered, field) = digits_x(true);
        let (plain, _) = digits_x(false);
        assert!(
            metered > plain,
            "the metered field's number is at {metered} and the plain one's at \
             {plain} — the metered one has to be further right"
        );
        assert!(
            metered > field.center().x,
            "the metered number sits at {metered}, left of the field's middle at {}",
            field.center().x
        );
    }

    /// Zero paints no bar and either extreme fills half the field.
    #[test]
    fn the_meter_is_empty_at_zero_and_fills_one_half_at_each_extreme() {
        let (_, _, empty) = parts(0.0);
        assert_eq!(
            empty.width(),
            0.0,
            "an unadjusted row must draw no bar — only its zero tick"
        );

        for (name, frac) in [("+1", 1.0_f32), ("-1", -1.0)] {
            let (ground, _, full) = parts(frac);
            let ink = ground.shrink(FIELD_BORDER_H / 2.0);
            assert!(
                (full.width() - ink.width() / 2.0).abs() < 0.51,
                "a bar at {name} is {} wide where half the field's ink is {}",
                full.width(),
                ink.width() / 2.0
            );
            assert!(
                ground.contains_rect(full),
                "the {name} bar ran outside its field: {full:?} in {ground:?}"
            );
        }
    }

    /// The zero tick is a hairline at the field's middle at every value, nonzero included, stopping short of the border.
    #[test]
    fn the_zero_tick_is_a_hairline_at_the_middle_whatever_the_value() {
        for (name, frac) in [("0", 0.0_f32), ("+0.4", 0.4), ("-1", -1.0)] {
            let (ground, tick, _) = parts(frac);
            let ink = ground.shrink(FIELD_BORDER_H / 2.0);
            assert!(
                (tick.width() - METER_TICK_W).abs() < 0.01,
                "the tick at {name} is {} wide, not the hairline it has to be",
                tick.width()
            );
            assert!(
                (tick.center().x - ink.center().x).abs() < 0.51,
                "the tick at {name} sits at {} where the field's middle is {}",
                tick.center().x,
                ink.center().x
            );
            assert!(
                (tick.height() - ink.height()).abs() < 0.51,
                "the tick at {name} is {} tall in {} of ink",
                tick.height(),
                ink.height()
            );
            assert!(
                tick.top() > ground.top() && tick.bottom() < ground.bottom(),
                "the tick at {name} touches the field's border: {tick:?} in {ground:?}"
            );
        }
    }

    /// Each pair of bar corners rounds only when the bar reaches that end, for both signs.
    #[test]
    fn a_bars_corners_are_square_except_where_it_meets_an_edge() {
        let corners = |frac: f32| {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let _ = ctx.run_ui(Default::default(), |_| {});
            let mut value = 40.0_f64;
            let out = ctx.run_ui(Default::default(), |ui| {
                value_field_metered(
                    ui,
                    egui::vec2(180.0, 26.0),
                    Prefix::Text("Contrast"),
                    frac,
                    &mut value,
                    Scrub::whole(0.25).range(-100.0..=100.0),
                    |d| d,
                );
            });
            // The second wash rect is the bar; the first is the square zero tick.
            let washes: Vec<egui::CornerRadius> = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Rect(r) if r.fill == meter_wash() => Some(r.corner_radius),
                    _ => None,
                })
                .collect();
            assert_eq!(washes.len(), 2, "a tick and a bar");
            washes[1]
        };

        for (name, frac) in [("+0.5", 0.5_f32), ("-0.5", -0.5)] {
            let part = corners(frac);
            assert_eq!(
                (part.nw, part.sw, part.ne, part.se),
                (0, 0, 0, 0),
                "the {name} bar touches neither edge, so every corner is square — \
                 a rounded one is a capsule floating in the field"
            );
        }

        let right = corners(1.0);
        assert_eq!(
            (right.ne, right.se),
            (5, 5),
            "at +1 the bar meets the field's right corners and must follow them"
        );
        assert_eq!(
            (right.nw, right.sw),
            (0, 0),
            "and its left end is still in the middle of the field"
        );

        let left = corners(-1.0);
        assert_eq!(
            (left.nw, left.sw),
            (5, 5),
            "at −1 it meets the left corners — the pair the old left-anchored bar \
             rounded unconditionally"
        );
        assert_eq!(
            (left.ne, left.se),
            (0, 0),
            "and its right end is the middle"
        );
    }
}

#[cfg(test)]
mod expr_field_tests {
    use super::*;

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// One pass over a lone `value_field`, returning its painted ground for the next pass to aim at.
    fn pass(ctx: &egui::Context, v: &mut f64, events: Vec<egui::Event>) -> egui::Rect {
        let out = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                value_field(
                    ui,
                    egui::vec2(120.0, 28.0),
                    Prefix::Text("W"),
                    v,
                    Scrub::whole(0.5),
                    |d| d.custom_formatter(number(2)),
                );
            },
        );
        let mut rect = egui::Rect::NOTHING;
        for c in &out.shapes {
            if let egui::Shape::Rect(r) = &c.shape
                && (r.rect.height() - 28.0).abs() < 0.51
            {
                rect = r.rect;
            }
        }
        rect
    }

    /// Types `text` into the field's keyboard face a character at a time, returning the value after each and after Enter.
    fn typed(text: &str) -> Vec<f64> {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = 100.0;
        pass(&ctx, &mut v, Vec::new());
        let rect = pass(&ctx, &mut v, Vec::new());
        let at = rect.center();
        // A click with no travel swaps the `DragValue` for a `TextEdit` and selects its digits.
        pass(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(at), button(at, true)],
        );
        pass(&ctx, &mut v, vec![button(at, false)]);
        assert_eq!(v, 100.0, "focusing the field must not have changed it");
        let mut trail = Vec::new();
        for ch in text.chars() {
            pass(&ctx, &mut v, vec![egui::Event::Text(ch.to_string())]);
            trail.push(v);
        }
        pass(
            &ctx,
            &mut v,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        pass(&ctx, &mut v, Vec::new());
        trail.push(v);
        trail
    }

    /// A width field takes `135*2` and reads 270; the trail pins that `135*` is rejected and the value stays 135.
    #[test]
    fn a_width_field_evaluates_the_sum_typed_into_it() {
        assert_eq!(
            typed("135*2"),
            vec![1.0, 13.0, 135.0, 135.0, 270.0, 270.0],
            "135*2 did not come out 270 — the last entry is after Enter"
        );
    }

    /// A plain number typed into a field is the number.
    #[test]
    fn a_plain_number_is_unaffected_by_the_parser_that_can_take_sums() {
        assert_eq!(*typed("42").last().unwrap(), 42.0);
        assert_eq!(*typed("33.5").last().unwrap(), 33.5);
        assert_eq!(*typed("-8").last().unwrap(), -8.0);
    }
}

#[cfg(test)]
mod operator_glyph_tests {
    use eframe::egui;

    /// The advance and atlas entry of the glyph at `index` of `text`.
    fn glyph(ctx: &egui::Context, text: &str, index: usize) -> (f32, [f32; 2]) {
        let galley = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(12.5),
                egui::Color32::WHITE,
            )
        });
        let g = galley
            .rows
            .iter()
            .flat_map(|r| r.row.glyphs.iter())
            .nth(index)
            .expect("the fixture is shorter than the glyph asked for");
        (g.advance_width, [g.uv_rect.offset.x, g.uv_rect.offset.y])
    }

    /// An operator's glyph changes between digits because Inter's `calt` swaps in a figure-aligned alternate; fails if Inter stops.
    #[test]
    fn an_operator_between_digits_is_the_fonts_alternate_glyph_not_ours() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let alone = glyph(&ctx, "10 *", 3);
        let between = glyph(&ctx, "10 * 3", 3);
        assert_ne!(
            alone, between,
            "Inter's `calt` no longer swaps the asterisk between figures — the \
             typing-time glyph shift this explains is gone, and so is the reason \
             for this test"
        );
        assert!(
            between.0 > alone.0,
            "the figure asterisk is the wider of the two: {between:?} against {alone:?}"
        );
        // `-` keeps its glyph and is lifted to the digits' centre line.
        let (hyphen_alone, hyphen_between) = (glyph(&ctx, "10 -", 3), glyph(&ctx, "10 - 3", 3));
        assert_eq!(
            hyphen_alone.0, hyphen_between.0,
            "the hyphen is not substituted, only moved"
        );
        assert_ne!(
            hyphen_alone.1, hyphen_between.1,
            "and moved is what it is: {hyphen_alone:?} against {hyphen_between:?}"
        );
    }
}

#[cfg(test)]
mod context_menu_chrome_tests {
    use super::*;

    /// A viewport, in points, with nothing covered.
    fn ctx_at(ppp: f32) -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        ctx.set_pixels_per_point(ppp);
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            },
            |_| {},
        );
        ctx
    }

    /// A `menu_frame` sized by `menu_inner_w` paints the width it is named, for every `(width, pad)` pair the app uses.
    ///
    /// It pins `menu_inner_w`'s arithmetic only.
    ///
    /// flip: `set_width(w - pad * 2.0)` → every row is out by `2 * MENU_BORDER`.
    #[test]
    fn a_menu_card_paints_the_width_it_is_named() {
        let ctx = ctx_at(1.0);
        for (name, w, pad) in [
            ("context menu", 210.0, 5.0),
            ("zoom", 150.0, 5.0),
            ("view/snap", 160.0, 5.0),
            ("popover", crate::app::POPOVER_W, 11.0),
        ] {
            let mut painted = 0.0_f32;
            let _ = ctx.run_ui(Default::default(), |ui| {
                painted = menu_frame(pad)
                    .show(ui, |ui| {
                        ui.set_width(menu_inner_w(w, pad));
                        ui.label("x");
                    })
                    .response
                    .rect
                    .width();
            });
            assert!(
                (painted - w).abs() < 0.01,
                "the {name} card is named {w} and paints {painted}"
            );
        }
    }

    /// A menu opened at any corner stays inside the viewport at 100% and 150%, and lies wholly on one side of the press on each axis (the flip).
    #[test]
    fn a_menu_opened_at_any_corner_stays_inside_the_viewport() {
        let size = egui::vec2(210.0, 460.0);
        for ppp in [1.0, 1.5] {
            let ctx = ctx_at(ppp);
            let screen = ctx.content_rect();
            assert!(
                size.y < screen.height(),
                "the fixture menu has to fit, or the clamp answers for the flip"
            );
            for at in [
                screen.left_top(),
                screen.right_top(),
                screen.left_bottom(),
                screen.right_bottom(),
                // Just inside the bottom-right, where the flip must fire without the clamp saving it.
                screen.right_bottom() - egui::vec2(3.0, 3.0),
            ] {
                let pos = menu_place(&ctx, at, size);
                let placed = egui::Rect::from_min_size(pos, size);
                assert!(
                    screen.contains_rect(placed),
                    "at {at:?} ppp {ppp}: {placed:?} escapes {screen:?}"
                );
                // Half a point of slack for device-pixel rounding at 150%.
                assert!(
                    pos.x >= at.x - 0.5 || pos.x + size.x <= at.x + 0.5,
                    "at {at:?} ppp {ppp}: {placed:?} straddles the press in x"
                );
                assert!(
                    pos.y >= at.y - 0.5 || pos.y + size.y <= at.y + 0.5,
                    "at {at:?} ppp {ppp}: {placed:?} straddles the press in y"
                );
            }
        }
    }

    /// A menu taller than the viewport clamps to its top (the head rows) rather than flips.
    #[test]
    fn a_menu_taller_than_the_viewport_is_pinned_to_its_top() {
        let ctx = ctx_at(1.0);
        let screen = ctx.content_rect();
        let size = egui::vec2(210.0, screen.height() + 200.0);
        let pos = menu_place(&ctx, screen.center(), size);
        assert_eq!(pos.y, screen.top(), "a too-tall menu must keep its head");
    }

    /// A dimmed menu row is still hoverable (so its tooltip can land) and refuses the click.
    ///
    /// flip: `Sense::click()` inside a `ui.disable()`d scope → `hovered()` is false.
    #[test]
    fn a_dimmed_menu_row_is_still_hoverable_and_still_refuses_the_click() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let at = egui::pos2(60.0, 12.0);
        let mut hovered = false;
        let mut clicked = false;
        for pass in 0..4 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 200.0),
                    )),
                    events: if pass == 2 {
                        vec![
                            egui::Event::PointerMoved(at),
                            egui::Event::PointerButton {
                                pos: at,
                                button: egui::PointerButton::Primary,
                                pressed: true,
                                modifiers: Default::default(),
                            },
                            egui::Event::PointerButton {
                                pos: at,
                                button: egui::PointerButton::Primary,
                                pressed: false,
                                modifiers: Default::default(),
                            },
                        ]
                    } else {
                        vec![egui::Event::PointerMoved(at)]
                    },
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(200.0);
                    let resp = menu_row(
                        ui,
                        MenuRow::new(icon::TRASH, "Delete").enabled(false),
                        MENU_ROW_H,
                    );
                    hovered |= resp.hovered();
                    clicked |= resp.clicked();
                },
            );
            let _ = out;
        }
        assert!(
            hovered,
            "a dimmed row must be hoverable, or it cannot say why"
        );
        assert!(!clicked, "a dimmed row must not be clickable");
    }

    /// How many rows painted the hover ground (the only `hover()` rect a bare `menu_row` draws).
    fn grounds(out: &egui::FullOutput) -> usize {
        out.shapes
            .iter()
            .filter(|c| match &c.shape {
                egui::Shape::Rect(r) => r.fill == color::hover(),
                _ => false,
            })
            .count()
    }

    /// A checkable row with no glyph ticks its glyph column when on, and on or off
    /// sets its label in the one label column, so a menu of them stays aligned.
    ///
    /// flip: `bare` in `menu_row` ignoring `row.checked` → fails at "one column".
    #[test]
    fn a_checkable_row_without_a_glyph_keeps_the_label_column() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(200.0);
            menu_row(ui, MenuRow::new("", "On").checked(true), MENU_ROW_H);
            menu_row(ui, MenuRow::new("", "Off").checked(false), MENU_ROW_H);
        });
        let left = |s: &str| {
            out.shapes.iter().find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == s => Some(t.pos.x),
                _ => None,
            })
        };
        let (on, off) = (left("On").expect("on"), left("Off").expect("off"));
        assert_eq!(on, off, "one column");
        assert_eq!(off, MENU_LABEL_X, "the label column");
        assert!(
            left(icon::CHECK).is_some(),
            "the tick shows on the checked row"
        );
    }

    /// A keyboard highlight paints exactly one ground even with the pointer resting on another row (`docs/context-menus.md` §8), a `submenu_row` included.
    ///
    /// flip: `row.highlight == Some(true) || resp.hovered()` in `menu_row`, or
    /// `submenu_row` ignoring `row.highlight` → the count is 2.
    #[test]
    fn a_keyboard_highlight_paints_one_ground_and_the_pointer_cannot_add_another() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let on_first = egui::pos2(60.0, MENU_ROW_H * 0.5);
        let labels = ["One", "Two", "Three"];
        let mut counts = Vec::new();
        // Four passes (interaction state is last frame's); only the last is measured.
        for keyboard in [false, true] {
            let mut last = 0;
            for _ in 0..4 {
                let out = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(300.0, 200.0),
                        )),
                        events: vec![egui::Event::PointerMoved(on_first)],
                        ..Default::default()
                    },
                    |ui| {
                        ui.set_width(200.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (i, label) in labels.iter().enumerate() {
                            let mut style = MenuRow::new(icon::TRASH, label);
                            if keyboard {
                                style = style.highlight(i == 2);
                            }
                            // The pointer's row opens a list, as a context menu's can.
                            match i {
                                0 => submenu_row(ui, style, false),
                                _ => menu_row(ui, style, MENU_ROW_H),
                            };
                        }
                    },
                );
                last = grounds(&out);
            }
            counts.push(last);
        }
        assert_eq!(
            counts[0], 1,
            "the pointer has to be painting a ground for this test to be about anything"
        );
        assert_eq!(
            counts[1], 1,
            "the keyboard's highlight has to paint, and the pointer must not add a second"
        );
    }

    /// `menu_place` clamps after it rounds (at ppp 1.5 the limit 1263 rounds outward to 1263.333); an ordinary placement still snaps.
    ///
    /// `at` is outside the viewport on purpose: on screen, the clamp can never bite after a flip.
    ///
    /// flip: `snap(pos.clamp(…))` → the clamped assertion fails (1263.3334 against 1263.0).
    #[test]
    fn a_menu_is_clamped_after_it_is_rounded_and_not_before() {
        let ctx = ctx_at(1.5);
        let size = egui::vec2(137.0, 40.0);
        // Inside a frame: outside one, `content_rect()` is a 10,000pt fallback.
        let place = |at: egui::Pos2| {
            let mut out = egui::Pos2::ZERO;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1400.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| out = menu_place(ui.ctx(), at, size),
            );
            out
        };
        let limit = 1400.0 - size.x;

        assert_ne!(
            (limit * 1.5).round() / 1.5,
            limit,
            "the fixture: 1263 must not be on the 1.5x pixel grid"
        );

        let placed = place(egui::pos2(1450.0, 100.0));
        assert_eq!(
            placed.x,
            limit,
            "the clamp is the last word: a rounded-then-clamped menu is on screen \
             for certain, where clamping and then rounding puts it {} past the \
             edge",
            (limit * 1.5).round() / 1.5 - limit
        );

        let crisp = place(egui::pos2(100.3, 100.3));
        assert_eq!(
            crisp.x,
            (100.3_f32 * 1.5).round() / 1.5,
            "away from an edge the rounding is the answer and nothing clamps"
        );
    }

    /// A submenu with no room below its row ends at the screen's bottom, its
    /// height still beside the row.
    /// flip: passing `row.top() - lift` to `menu_place` unclamped → fails at the
    /// bottom assertion (694 against 820).
    #[test]
    fn a_submenu_short_of_room_shifts_up_beside_its_row() {
        let ctx = ctx_at(1.0);
        let row = egui::Rect::from_min_size(egui::pos2(100.0, 700.0), egui::vec2(200.0, 24.0));
        let size = egui::vec2(180.0, 160.0);
        let mut at = egui::Pos2::ZERO;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 820.0),
                )),
                ..Default::default()
            },
            |ui| at = flyout_place(ui.ctx(), row, size, 6.0),
        );
        assert_eq!(at.y + size.y, 820.0);
        assert!(at.y <= row.top() && row.bottom() <= at.y + size.y);
    }
}

/// The type family row's warning state (`glyph_and_text_tinted`).
#[cfg(test)]
mod family_row_chrome_tests {
    use super::*;

    /// A `warn()`-tinted glyph survives into the galley a `ComboBox` paints, while the label stays full-strength `text()`.
    #[test]
    fn a_warn_tinted_combo_glyph_reaches_the_painted_galley() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::ComboBox::from_id_salt("probe")
                    .selected_text(glyph_and_text_tinted(
                        icon::TEXT_AA,
                        "Nonesuch",
                        theme::color::warn(),
                    ))
                    .show_ui(ui, |_| {});
            },
        );
        // A galley mesh's vertex colour is the ink colour.
        let mut colours = Vec::new();
        for clipped in &out.shapes {
            if let egui::Shape::Text(t) = &clipped.shape {
                for row in &t.galley.rows {
                    colours.extend(row.row.visuals.mesh.vertices.iter().map(|v| v.color));
                }
            }
        }
        colours.sort_by_key(|c| c.to_array());
        colours.dedup();
        assert!(
            colours.contains(&theme::color::warn()),
            "the warning colour never reached the galley: {colours:?}"
        );
        assert!(
            colours.contains(&color::text()),
            "and the label must stay full strength beside it: {colours:?}"
        );
    }
}

#[cfg(test)]
mod hairline_tests {
    //! The device-pixel snap the chrome's three rules share.

    use super::*;

    /// How much of each device pixel a `width`-point line centred at `at` covers (a copy of the helper in `rulers.rs`).
    fn coverage(at: f32, ppp: f32, width: f32) -> Vec<f32> {
        let centre = at * ppp;
        let (lo, hi) = (centre - width * ppp / 2.0, centre + width * ppp / 2.0);
        let mut out = Vec::new();
        let mut p = lo.floor();
        while p < hi - 1e-6 {
            out.push((p + 1.0).min(hi) - p.max(lo));
            p += 1.0;
        }
        out
    }

    /// A 1pt hairline covers whole device pixels at an even integer scale; fractional scales give the best a fractional width can.
    ///
    /// flip: `hairline_on_pixel_centre` forced to `true` → fails at ppp 2.0 (`[0.5, 1.0, 0.5]` against `[1.0, 1.0]`) and at no other scale.
    #[test]
    fn a_hairline_covers_whole_pixels_wherever_the_scale_allows_it() {
        for (ppp, want) in [
            (1.0f32, vec![1.0f32]),
            (1.25, vec![0.125, 1.0, 0.125]),
            (1.5, vec![0.25, 1.0, 0.25]),
            (1.75, vec![0.375, 1.0, 0.375]),
            (2.0, vec![1.0, 1.0]),
            (3.0, vec![1.0, 1.0, 1.0]),
        ] {
            let got = coverage(hairline_in_column(100.3, ppp, 1.0), ppp, 1.0);
            assert_eq!(got.len(), want.len(), "at ppp {ppp}: {got:?}");
            for (g, w) in got.iter().zip(&want) {
                assert!((g - w).abs() < 1e-4, "at ppp {ppp}: {got:?} want {want:?}");
            }
        }
    }

    /// The snap never moves a rule more than half a device pixel from its column's centre.
    ///
    /// flip: odd arm as `(round + 0.5)` → fails at ppp 1.0 (1.0pt drift); even arm as `floor` → fails at ppp 2.0 (0.4975pt against 0.25).
    #[test]
    fn the_snap_never_pushes_a_rule_out_of_its_own_column() {
        for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0, 3.0] {
            let mut worst = 0.0f32;
            for k in 0..400 {
                let centre = 100.0 + k as f32 / 400.0;
                worst = worst.max((hairline_in_column(centre, ppp, 1.0) - centre).abs());
            }
            assert!(
                worst <= 0.5 / ppp + 1e-4,
                "at ppp {ppp} the snap moved a 1pt rule {worst:.4}pt from its centre, \
                 where half a device pixel is {:.4}pt — so it is drifting further out \
                 of its own column than the grid costs",
                0.5 / ppp
            );
        }
    }

    /// `ui::rule` paints its `LineSegment` on the snapped coordinate at 200%; the arithmetic tests above would pass beside a call site with its own copy.
    #[test]
    fn the_align_rows_divider_paints_on_the_snapped_coordinate() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        ctx.set_pixels_per_point(2.0);
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(10.3);
                    rule(ui, 24.0);
                });
            },
        );
        let segments: Vec<[egui::Pos2; 2]> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::LineSegment { points, .. } => Some(*points),
                _ => None,
            })
            .collect();
        assert_eq!(segments.len(), 1, "one rule, one segment: {segments:?}");
        let [a, b] = segments[0];
        assert!(
            (a.x - b.x).abs() < 1e-6,
            "the divider is vertical: {a:?} {b:?}"
        );
        assert!(
            (a.x * 2.0 - (a.x * 2.0).round()).abs() < 1e-4,
            "at 200% a 2-device-pixel line must land on a pixel edge; it painted at \
             x = {} (device {})",
            a.x,
            a.x * 2.0
        );
    }
}

#[cfg(test)]
mod ramp_tests {
    //! The gradient chip against the gradient on the page.

    use super::*;

    /// The ramp mesh's columns, as `(x, colour)` in build order.
    fn columns(ctx: &egui::Context, stops: &[(f32, egui::Color32)]) -> Vec<(f32, egui::Color32)> {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 20.0));
        let out = ctx.run_ui(Default::default(), |ui| {
            paint_ramp(ui.painter(), rect, stops);
        });
        out.shapes
            .into_iter()
            .filter_map(|c| match c.shape {
                egui::Shape::Mesh(m) => Some(m),
                _ => None,
            })
            .flat_map(|m| {
                m.vertices
                    .iter()
                    .map(|v| (v.pos.x, v.color))
                    .collect::<Vec<_>>()
            })
            // Two vertices per column, top and bottom, so every second one.
            .step_by(2)
            .collect()
    }

    /// A descending stop list is clamped as the canvas clamps it (columns never walk backwards, the first stop's colour fills the box).
    ///
    /// flip: dropping the `monotonic_offset` call → fails the non-decreasing assertion (`0` after `100`).
    #[test]
    fn a_descending_ramp_is_clamped_the_way_the_canvas_clamps_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (red, blue) = (
            egui::Color32::from_rgb(255, 0, 0),
            egui::Color32::from_rgb(0, 0, 255),
        );

        let up = columns(&ctx, &[(0.0, blue), (1.0, red)]);
        assert_eq!(up.len(), 4, "two ends plus a column per stop: {up:?}");
        assert_eq!(
            up.iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![0.0, 0.0, 100.0, 100.0]
        );
        assert_eq!((up[0].1, up[3].1), (blue, red), "blue at the left: {up:?}");

        let down = columns(&ctx, &[(1.0, red), (0.0, blue)]);
        assert!(
            down.windows(2).all(|w| w[0].0 <= w[1].0 + 1e-6),
            "the columns must not walk backwards: {down:?}"
        );
        assert_eq!(
            down[0].1, red,
            "SVG corrects the second offset up to the first, so the ramp is the \
             first stop's colour across the box: {down:?}"
        );
    }
}

#[cfg(test)]
mod slider_ink_tests {
    //! How far a `slider`'s drawing leaves the row it allocates.

    use super::*;

    /// The knob's shadow bleeds exactly one point below the row and the other three sides are tight.
    ///
    /// flip: shadow offset `center().y + 1.0` to `+ 2.0` → the bottom assertion fails (`2` against `1`); to `center().y` → it fails the other way.
    #[test]
    fn a_sliders_shadow_leaves_the_row_by_exactly_one_point() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut value = 0.5_f64;
        let mut row = egui::Rect::NOTHING;
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                row = slider(ui, 120.0, &mut value, 0.0..=1.0).rect;
            },
        );
        let ink = out
            .shapes
            .iter()
            .map(|c| c.shape.visual_bounding_rect())
            .filter(|r| r.is_finite() && r.is_positive())
            .reduce(|a, b| a.union(b))
            .expect("the slider painted something");

        assert_eq!(row.height(), SLIDER_H, "the row is the constant");
        assert_eq!(
            (ink.max.y - row.max.y).round(),
            1.0,
            "the shadow's reach below the row moved: row {row:?}, ink {ink:?}"
        );
        for (side, over) in [
            ("top", row.min.y - ink.min.y),
            ("left", row.min.x - ink.min.x),
            ("right", ink.max.x - row.max.x),
        ] {
            assert!(
                over <= 0.0,
                "the drawing grew {side}wards out of its row by {over}: \
                 row {row:?}, ink {ink:?}"
            );
        }
    }
}

#[cfg(test)]
mod zero_sense_hover_tests {
    //! What a widget that senses nothing is hovered by.

    use super::*;

    /// A zero-sense widget (`Sense::empty()`) is hovered only while nothing interactive is registered above it.
    ///
    /// flip: dropping the `ui.interact(…, Sense::click())` line → the second assertion fails (the row is hovered again).
    #[test]
    fn a_row_that_senses_nothing_is_hovered_only_while_it_is_on_top() {
        let hovered_with_a_widget_over_it = |covered: bool| {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let at = egui::pos2(50.0, 10.0);
            let mut out = false;
            for _ in 0..2 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(200.0, 60.0),
                        )),
                        events: vec![egui::Event::PointerMoved(at)],
                        ..Default::default()
                    },
                    |ui| {
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(200.0, 20.0), egui::Sense::empty());
                        if covered {
                            ui.interact(rect, ui.id().with("over"), egui::Sense::click());
                        }
                        out = resp.hovered();
                    },
                );
            }
            out
        };

        assert!(
            hovered_with_a_widget_over_it(false),
            "the control: a zero-sense row on its own does see the pointer, which \
             is what makes a dimmed menu row explainable at all"
        );
        assert!(
            !hovered_with_a_widget_over_it(true),
            "and it stops the moment something interactive is registered over the \
             same rect — so 'it wins the hit test' was never the reason"
        );
    }
}

#[cfg(test)]
mod segmented_disabled_tests {
    //! What a disabled segmented cell claims.

    use super::*;

    /// Tabs through a three-cell segmented control with cell `disabled` unavailable and returns how many distinct widgets the keyboard reaches.
    fn tab_stops(disabled: usize) -> usize {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut seen: Vec<egui::Id> = Vec::new();
        let frame = |events: Vec<egui::Event>| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 120.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    segmented_enabled(ui, 240.0, 24.0, 3, 0, |i| i != disabled, |_, _, _, _| {});
                },
            );
        };
        frame(Vec::new());
        let tab = || egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        // More presses than cells, so the order wraps and meets every stop.
        for _ in 0..6 {
            frame(vec![tab()]);
            if let Some(id) = ctx.memory(|m| m.focused())
                && !seen.contains(&id)
            {
                seen.push(id);
            }
        }
        seen.len()
    }

    /// A disabled segmented cell is not a tab stop (`Sense::click()` includes `FOCUSABLE`), while an all-enabled control keeps all three.
    ///
    /// flip: `Sense::click()` for every cell → fails the second assertion (3 against 2); `Sense::hover()` for every cell → fails the first (0 against 3).
    #[test]
    fn a_disabled_segmented_cell_is_not_a_tab_stop() {
        // 3 is out of range, so nothing is disabled: the control.
        assert_eq!(
            tab_stops(3),
            3,
            "the fixture: three live cells are three tab stops, or this test \
             cannot tell a fix from a control that reaches nothing"
        );
        assert_eq!(
            tab_stops(1),
            2,
            "and a cell that refuses the click must not take the focus either — \
             Tab stopping there swallows Return for a control that does nothing"
        );
    }
}

#[cfg(test)]
mod hairline_weight_tests {
    //! A menu separator's rule keeps its weight as the display scale rises.

    use super::*;

    /// The height `menu_sep` paints, in points, at `ppp`.
    fn painted_rule(ppp: f32) -> f32 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        ctx.set_pixels_per_point(ppp);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_max_width(200.0);
            menu_sep(ui);
        });
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.rect.height() < MENU_SEP_H => Some(r.rect.height()),
                _ => None,
            })
            .fold(0.0_f32, f32::max)
    }

    /// A separator is a nominal 1pt rule snapped to `round(ppp).max(1.0)` whole device pixels, matching `MENU_BORDER`'s weight.
    ///
    /// flip: restoring `1.0 / ppp` → fails the third assertion; a bare `1.0` → fails the first (not a whole number of device pixels).
    #[test]
    fn a_menu_separator_holds_its_weight_as_the_display_scale_rises() {
        for ppp in [1.0_f32, 1.5, 2.0, 3.0] {
            let painted = painted_rule(ppp);
            let device = painted * ppp;
            assert!(
                (device - device.round()).abs() < 1e-3,
                "at ppp {ppp} the rule paints {painted}pt = {device} device pixels, \
                 which is not a whole number — the snapping is what keeps it from \
                 reading as a colour change"
            );
            assert!(
                device >= 1.0,
                "at ppp {ppp} the rule paints {device} device pixels — a hairline \
                 thinner than one pixel is a hairline the display cannot draw"
            );
            // Exact, not a half-pixel tolerance: at ppp 1.5, 0.667 and 1.333 are equally far from 1pt.
            let want = (1.0_f32 * ppp).round().max(1.0);
            assert!(
                (device - want).abs() < 1e-3,
                "at ppp {ppp} the rule paints {painted}pt = {device} device pixels \
                 where a nominal 1pt snaps to {want} — off by this much it no \
                 longer matches MENU_BORDER 5pt away in the same card"
            );
        }
    }

    /// The rule fits inside the `MENU_SEP_H` that `menu::height` reserves, at every scale.
    #[test]
    fn the_rule_still_fits_the_height_the_menu_reserved_for_it() {
        for ppp in [1.0_f32, 1.5, 2.0, 3.0] {
            let painted = painted_rule(ppp);
            assert!(
                painted < MENU_SEP_H,
                "at ppp {ppp} the rule is {painted}pt inside an allocation of \
                 {MENU_SEP_H}pt — a rule that outgrew its own air would move every \
                 menu that has one"
            );
        }
    }
}

#[cfg(test)]
mod picture_swatch_tests {
    //! A picture chip is drawn at the brush's own alpha.

    use super::*;

    /// The vertex colour of each textured mesh a chip paints (callers assert exactly one).
    fn chip_tint(ctx: &egui::Context, alpha: f32) -> Vec<egui::Color32> {
        let out = ctx.run_ui(Default::default(), |ui| {
            // Built inside: `run_ui` takes an `FnMut` and `Swatch` is not `Copy`.
            swatch(
                ui,
                16.0,
                egui::Color32::BLACK,
                Swatch::Picture {
                    id: picture(),
                    alpha,
                },
            );
        });
        out.shapes
            .into_iter()
            .filter_map(|c| match c.shape {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
                    Some(m.vertices.first().map(|v| v.color).unwrap_or_default())
                }
                _ => None,
            })
            .collect()
    }

    /// A texture id that is not the font atlas, so `chip_tint` can tell the chip's mesh from glyphs.
    fn picture() -> egui::TextureId {
        egui::TextureId::User(7)
    }

    /// A 30% picture fill draws a fainter chip than an opaque one, whose tint is the identity `WHITE`.
    #[test]
    fn a_faded_picture_chip_is_drawn_faded() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let opaque = chip_tint(&ctx, 1.0);
        assert_eq!(opaque.len(), 1, "one textured mesh, which is the picture");
        assert_eq!(
            opaque[0],
            egui::Color32::WHITE,
            "an opaque picture keeps image's identity multiplier — a chip drawn in \
             any other colour is a photograph in one hue"
        );

        let faint = chip_tint(&ctx, 0.3);
        assert_eq!(faint.len(), 1);
        assert_ne!(
            faint[0],
            egui::Color32::WHITE,
            "a 30% image fill drew an opaque chip — the defect this test is for"
        );
        assert!(
            faint[0].a() < opaque[0].a(),
            "and it has to be fainter than the opaque one, not merely different \
             ({} vs {})",
            faint[0].a(),
            opaque[0].a()
        );
    }

    /// The chip tint is a grey multiplier (r = g = b), so the picture keeps its hues.
    #[test]
    fn the_chip_tint_stays_grey_so_the_picture_keeps_its_hues() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let t = chip_tint(&ctx, 0.4)[0];
        assert_eq!(
            (t.r(), t.g()),
            (t.g(), t.b()),
            "r = g = b, or the chip is tinting the photograph rather than fading it"
        );
    }
}

#[cfg(test)]
mod double_click_tests {
    //! `ui::double_clicked`'s counting, with egui's own answer beside it as the control.

    use super::double_clicked;

    const AREA: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(400.0, 300.0),
    };

    /// One pass with `events` over a widget covering the area: ours, then egui's `Response::double_clicked`.
    fn pass(ctx: &egui::Context, events: Vec<egui::Event>) -> (bool, bool) {
        let mut out = (false, false);
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(AREA),
                events,
                ..Default::default()
            },
            |ui| {
                let resp = ui.interact(AREA, egui::Id::new("probe"), egui::Sense::click());
                out = (double_clicked(ui, &resp), resp.double_clicked());
            },
        );
        out
    }

    /// A click at `at`, a pass per event; what the release pass answered.
    fn click(ctx: &egui::Context, at: egui::Pos2) -> (bool, bool) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        pass(ctx, vec![egui::Event::PointerMoved(at)]);
        pass(ctx, vec![button(true)]);
        pass(ctx, vec![button(false)])
    }

    /// A double is the click before, near and soon; a third click is single, and a click then a fast pair elsewhere is a double (egui says no).
    ///
    /// flip: dropping the `!l.double` term from the chain → fails "a third click on one spot is a single one".
    #[test]
    fn a_double_click_is_the_click_before_near_and_soon() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |_| {});
        pass(&ctx, Vec::new());
        let (a, b) = (egui::pos2(50.0, 50.0), egui::pos2(300.0, 200.0));
        assert!(!click(&ctx, a).0, "a first click is a single one");
        assert_eq!(
            click(&ctx, a),
            (true, true),
            "a fast second on the spot is a double"
        );
        assert!(
            !click(&ctx, a).0,
            "a third click on one spot is a single one"
        );
        assert!(click(&ctx, a).0, "and a fourth starts the next double");

        // Clear of every window, then a click at A and a fast pair at B.
        for _ in 0..60 {
            pass(&ctx, Vec::new());
        }
        click(&ctx, a);
        assert!(!click(&ctx, b).0, "a click elsewhere starts a fresh chain");
        assert_eq!(
            click(&ctx, b),
            (true, false),
            "the pair at B is a double — and egui, counting it a triple off the click \
             at A, says no: the quirk this function exists for"
        );

        // Slower than the delay: two singles.
        for _ in 0..60 {
            pass(&ctx, Vec::new());
        }
        click(&ctx, a);
        for _ in 0..30 {
            pass(&ctx, Vec::new());
        }
        assert!(!click(&ctx, a).0, "half a second apart is two clicks");
    }
}

#[cfg(test)]
mod split_button_tests {
    use super::*;

    /// A live `split_button`'s word is the primary's ink, as a primary
    /// `label_button`'s is, and the two keep the same air around a word.
    ///
    /// flip: `split_button` laying its word out in `color::text()` → fails at "ink".
    #[test]
    fn a_split_button_word_wears_the_primary_ink() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let word = |shapes: &[egui::epaint::ClippedShape], text: &str| {
            shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Text(t) if t.galley.job.text == text => Some(t.clone()),
                    _ => None,
                })
                .expect("the word is painted")
        };
        // What a glyph is drawn in: its section's colour, or the fallback for one
        // laid out in the placeholder.
        let ink = |t: &egui::epaint::TextShape| match t.galley.job.sections[0].format.color {
            egui::Color32::PLACEHOLDER => t.fallback_color,
            c => c,
        };
        let mut split = None;
        let out = ctx.run_ui(Default::default(), |ui| {
            split = Some(split_button(ui, "Merge", CONTROL_H, true).resp.rect);
        });
        let s = word(&out.shapes, "Merge");
        assert_eq!(ink(&s), color::accent_ink(), "ink");
        let mut plain = egui::Rect::NOTHING;
        let out = ctx.run_ui(Default::default(), |ui| {
            let w = s.galley.size().x + LABEL_BUTTON_PAD_X * 2.0;
            plain = label_button(ui, "Merge", egui::vec2(w, CONTROL_H), FieldButton::Primary).rect;
        });
        assert_eq!(ink(&word(&out.shapes, "Merge")), color::accent_ink());
        let split = split.unwrap();
        assert_eq!(split.width() - SPLIT_CARET_W, plain.width(), "the same air");
    }
}
