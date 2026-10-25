//! Shared chrome widgets (§9.2).
//!
//! The small vocabulary every panel is built from — cards, eyebrows, icon
//! buttons, fields, segmented controls, colour ramps. Nothing here knows about
//! the document.

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
        .fill(color::CARD)
        .stroke(egui::Stroke::new(MENU_BORDER, theme::color::text_a(15)))
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 24,
            spread: 0,
            color: egui::Color32::from_black_alpha(82),
        })
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::same(pad as i8))
}

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
    /// The chord this row answers to, right-aligned in [`theme::text::FAINT`].
    pub accel: Option<&'a str>,
    /// `Some` makes this a checkable row. The glyph column carries the state (the
    /// glyph turns [`color::ACCENT`], the label full strength); no tick column.
    pub checked: Option<bool>,
    /// A row that destroys something: [`theme::color::DANGER`] on glyph and label
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
    /// Paint the filled hexagon ([`paint_hexagon_filled`]) in the glyph's place, a
    /// main's mark, which the bundled Regular font has no glyph for.
    pub filled_hexagon: bool,
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
            filled_hexagon: false,
        }
    }
    pub fn filled_hexagon(mut self, on: bool) -> Self {
        self.filled_hexagon = on;
        self
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
        p.rect_filled(rect, egui::CornerRadius::same(4), theme::color::text_a(20));
    }
    // Three tiers: disabled, resting, on or hovered.
    let (glyph_col, label_col, accel_col) = match () {
        _ if !row.enabled => (
            theme::text::DISABLED,
            theme::color::text_a(65),
            theme::color::text_a(49),
        ),
        _ if row.danger => (
            color::DANGER,
            color::DANGER,
            color::DANGER.gamma_multiply(0.55),
        ),
        _ if on => (color::ACCENT, color::TEXT, theme::text::FAINT),
        _ if hovered => (color::TEXT, color::TEXT, theme::text::FAINT),
        _ => (theme::text::DIM, theme::text::MUTED, theme::text::FAINT),
    };
    // A row handed `""` sets its label at the row's own inset; no menu mixes the two.
    let bare = row.glyph.is_empty() && !row.filled_hexagon;
    let glyph_at = egui::pos2(rect.left() + 8.0 + 7.5, rect.center().y);
    if row.filled_hexagon {
        paint_hexagon_filled(p, glyph_at, 15.0, glyph_col);
    } else if !bare {
        p.text(
            glyph_at,
            egui::Align2::CENTER_CENTER,
            row.glyph,
            theme::icon_font(15.0),
            glyph_col,
        );
    }
    let label_x = if bare { 8.0 } else { 8.0 + 15.0 + 9.0 };
    let accel_w = match row.accel {
        Some(accel) => {
            let r = p.text(
                egui::pos2(rect.right() - 8.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                accel,
                egui::FontId::proportional(10.5),
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
        egui::TextFormat::simple(egui::FontId::proportional(11.5), label_col),
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

/// The air between a menu row's label and its accelerator; small so every fixed
/// label stays whole (`a_long_reset_label_is_cut_inside_the_row_and_fixed_labels_are_whole`).
const MENU_ACCEL_GAP: f32 = 2.0;

/// The total height one separator occupies — the rule plus its air, both sides.
///
/// [`menu_sep`] allocates it and `menu::height` sums it before the menu is shown;
/// `a_menus_computed_height_is_the_height_it_paints` pins the two together.
pub const MENU_SEP_H: f32 = 1.0 + SEP_AIR * 2.0;

/// [`menu_frame`]'s border, on every side.
///
/// A `Frame`'s stroke grows its outer rect, so a card paints `2 * MENU_BORDER`
/// wider and taller than content plus margins; `menu::height` counts it.
pub const MENU_BORDER: f32 = 1.0;

/// The width to give the `Ui` inside a [`menu_frame`] card that is to paint `card`
/// points wide, with `pad` of inner margin. The design's widths are the outer
/// card, which `menu_place`, [`OndinApp::popover_left`] and the right-aligning
/// anchors are all handed.
///
/// [`OndinApp::popover_left`]: crate::app::OndinApp::popover_left
pub const fn menu_inner_w(card: f32, pad: f32) -> f32 {
    card - pad * 2.0 - MENU_BORDER * 2.0
}

/// The air above and below a separator's rule.
const SEP_AIR: f32 = 3.0;

/// The hairline between two groups of a menu (`docs/context-menus.md` §8).
///
/// One point of `text_a(20)` with [`SEP_AIR`] either side, snapped to whole device
/// pixels and inside [`MENU_SEP_H`]
/// (`the_rule_still_fits_the_height_the_menu_reserved_for_it`).
pub fn menu_sep(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), MENU_SEP_H),
        egui::Sense::empty(),
    );
    let ppp = ui.ctx().pixels_per_point();
    // A point, in whole device pixels, never thinner than one.
    let weight = (1.0 * ppp).round().max(1.0) / ppp;
    let y = (rect.center().y * ppp).round() / ppp;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(rect.left(), y),
            egui::pos2(rect.right(), y + weight),
        ),
        0,
        theme::color::text_a(20),
    );
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

/// Height of a top-bar dropdown's head, and so of its hit target: the design's
/// control height.
const HEAD_H: f32 = 26.0;

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
    let lay = |ui: &egui::Ui, text: egui::RichText| {
        egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    };
    let lead = lay(ui, lead);
    let caret = lay(
        ui,
        theme::icon_text(icon::CARET_DOWN, 12.0, theme::text::DIM),
    );

    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(
            HEAD_LEAD + lead.size().x + gap + caret.size().x + HEAD_PAD,
            HEAD_H,
        ),
        egui::Sense::click(),
    );
    // Centred rather than baseline-aligned: different sizes and fonts.
    let p = ui.painter();
    let at = |g: &std::sync::Arc<egui::Galley>, x: f32| {
        egui::pos2(x, rect.center().y - g.size().y / 2.0)
    };
    let lit = open || resp.hovered();
    if lit {
        p.rect_filled(rect, egui::CornerRadius::same(5), theme::color::HOVER);
    }
    let ink_of = |g: &std::sync::Arc<egui::Galley>, pos: egui::Pos2| {
        if lit {
            p.galley_with_override_text_color(pos, g.clone(), color::TEXT);
        } else {
            p.galley(pos, g.clone(), egui::Color32::PLACEHOLDER);
        }
    };
    let ink = rect.left() + HEAD_LEAD;
    ink_of(&lead, at(&lead, ink));
    ink_of(&caret, at(&caret, ink + lead.size().x + gap));
    resp
}

/// A dropdown row that carries a checkmark, as in the View and Snap menus.
///
/// The tick keeps its 13pt column whether or not it shows, so labels stay in one
/// line. Off is also a dimmer label (72%), so the state reads at a glance.
pub fn menu_check(ui: &mut egui::Ui, label: &str, on: bool, height: f32) -> bool {
    use menu_check_row::{GAP, PAD_X, TICK_W};

    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let hovered = resp.hovered();
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, egui::CornerRadius::same(4), theme::color::text_a(20));
    }
    if on {
        p.text(
            egui::pos2(rect.left() + PAD_X + TICK_W / 2.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon::CHECK,
            theme::icon_font(13.0),
            color::ACCENT,
        );
    }
    p.text(
        egui::pos2(rect.left() + PAD_X + TICK_W + GAP, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.5),
        if on || hovered {
            color::TEXT
        } else {
            theme::text::MUTED
        },
    );
    resp.clicked()
}

/// [`menu_check`]'s row geometry, shared with [`menu_check_width`].
mod menu_check_row {
    /// The design's row: 8px inset, a 13px glyph column, 7px to the label.
    pub const PAD_X: f32 = 8.0;
    pub const TICK_W: f32 = 13.0;
    pub const GAP: f32 = 7.0;
}

/// The painted width a dropdown of [`menu_check`] rows needs so its widest
/// label fits — `floor` or more, the row's inset on the right as on the left.
pub fn menu_check_width(ctx: &egui::Context, labels: &[&str], pad: f32, floor: f32) -> f32 {
    use menu_check_row::{GAP, PAD_X, TICK_W};
    let widest = labels
        .iter()
        .map(|l| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    (*l).to_string(),
                    egui::FontId::proportional(11.5),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            })
        })
        .fold(0.0, f32::max);
    let row = PAD_X + TICK_W + GAP + widest + PAD_X;
    (row + 2.0 * pad + 2.0 * MENU_BORDER).ceil().max(floor)
}

/// A small-caps section eyebrow (`LAYERS`, `TRANSFORM`, …).
pub fn eyebrow(label: &str) -> egui::RichText {
    egui::RichText::new(label.to_uppercase())
        .size(EYEBROW_PT)
        .color(theme::text::FAINT)
}

/// An [`eyebrow`]'s size, named for the popover sections that draw a marked one
/// by hand (`typography::section_marked`).
pub const EYEBROW_PT: f32 = 9.5;

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
        .fill(color::CARD)
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
    section_head_full(ui, label, open, action, badge, None, false, sense)
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
/// `more` puts a `⋯` last at the right-hand end ([`HeadClicks::more`]).
#[allow(clippy::too_many_arguments)]
pub fn section_head_full(
    ui: &mut egui::Ui,
    label: &str,
    open: bool,
    action: Option<HeadAction>,
    badge: Option<&str>,
    overrides: Option<HeadOverrides<'_>>,
    more: bool,
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
                    theme::text::FAINT,
                ));
                ui.label(eyebrow(label));
                if let Some(o) = overrides {
                    let w = ui
                        .painter()
                        .layout_no_wrap(
                            o.reset.to_owned(),
                            egui::FontId::proportional(11.0),
                            theme::text::MUTED,
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

            if action.is_some() || badge.is_some() || more {
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
        let p = ui.painter();
        if hovered {
            let target = ui.interact(
                slot,
                ui.id().with(("section-reset", label)),
                egui::Sense::click(),
            );
            let ground = if target.hovered() {
                color::HOVER
            } else {
                color::FIELD
            };
            p.rect_filled(slot, 4.0, ground);
            let mid = slot.center().y;
            p.text(
                egui::pos2(slot.left() + HEAD_RESET_PAD + 6.0, mid),
                egui::Align2::CENTER_CENTER,
                icon::ARROW_COUNTER_CLOCKWISE,
                theme::icon_font(12.0),
                theme::text::MUTED,
            );
            p.text(
                egui::pos2(slot.left() + HEAD_RESET_PAD + 16.0, mid),
                egui::Align2::LEFT_CENTER,
                o.reset,
                egui::FontId::proportional(11.0),
                theme::text::STRONG,
            );
            if target.clicked() {
                clicks.reset = true;
                // The header's toggle sits under the chip; the reset is the press.
                clicks.toggled = false;
            }
        } else {
            override_dot(p, egui::pos2(slot.left() + 2.0, slot.center().y));
            p.text(
                egui::pos2(slot.left() + 8.0, slot.center().y),
                egui::Align2::LEFT_CENTER,
                o.count.to_string(),
                egui::FontId::proportional(10.0),
                theme::text::MUTED,
            );
        }
    }
    clicks
}

/// The override mark: a 4-pt neutral dot centred at `at`, on a card header, beside
/// an overridden field's label, and in a list item's trailing slot. Neutral
/// because the accent means selected.
pub fn override_dot(p: &egui::Painter, at: egui::Pos2) {
    p.circle_filled(at, 2.0, theme::text::MUTED);
}

/// The centre of every override dot a frame painted (2-pt muted circle, also the
/// layers panel's row dot).
#[cfg(test)]
pub(crate) fn painted_override_dots(out: &egui::FullOutput) -> Vec<egui::Pos2> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Pos2>) {
        match shape {
            egui::Shape::Circle(c) if c.radius == 2.0 && c.fill == theme::text::MUTED => {
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
        egui::FontId::proportional(10.0),
        color::ACCENT_100,
    );
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 12.0, 18.0),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, 4.0, color::ACCENT.gamma_multiply(0.18));
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        color::ACCENT_100,
    );
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
        .fill(color::FIELD)
        .stroke(egui::Stroke::new(1.0, color::FIELD_BORDER))
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
    ui.allocate_ui_with_layout(
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
    )
    .inner
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
    let frame = field_frame().stroke(egui::Stroke::new(1.0, color::ACCENT_700));
    framed_row(ui, size, frame, add)
}

/// A single-line **text** field on the same ground as [`value_field`]. A bare
/// `ui.add(TextEdit…)` differs in its frame, its inset and a focus ring nothing
/// else draws. `Frame::NONE` turns the ring off but also drops `TextEdit::margin`
/// (read only when no frame was passed), so padding goes in the frame.
/// `size.y` is the painted ground, hairline included — see [`field_row`].
pub fn text_field(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    text: &mut String,
    hint: &str,
    pt: f32,
) -> egui::Response {
    field_row(ui, size, |ui| {
        ui.add(
            egui::TextEdit::singleline(text)
                // The inset is the plate's; a `margin` here would be ignored.
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .font(egui::FontId::proportional(pt))
                .hint_text(egui::RichText::new(hint).size(pt)),
        )
    })
}

/// Inner padding of a dropdown's search field ([`search_field`]), its own rather
/// than `button_padding`, which suits controls a third this width.
pub const SEARCH_PAD: egui::Margin = egui::Margin::symmetric(7, 5);

/// The search field at the head of a dropdown list that filters as you type: the
/// theme's borderless ground and [`SEARCH_PAD`] inside it.
///
/// `width` is the field's outer width; the padding comes out of it.
pub fn search_field(
    ui: &mut egui::Ui,
    text: &mut String,
    id: egui::Id,
    hint: &str,
    width: f32,
) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .margin(SEARCH_PAD)
            .desired_width(width - SEARCH_PAD.sum().x)
            .hint_text(hint),
    )
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
/// A word (`X`, `LH`) or a Phosphor glyph, both in the same 38%-of-text grey. One
/// enum so a call site cannot pick the wrong size or grey.
#[derive(Clone, Copy)]
pub enum Prefix {
    /// A short word — `X`, `W`, `LH`.
    Text(&'static str),
    /// A Phosphor glyph.
    Icon(&'static str),
    /// A CSS property's name — `flex-grow`, `min-width` — at the unit's size
    /// rather than [`PREFIX_PT`], so a whole word does not take the number's width.
    Label(&'static str),
}

/// Type size of a [`Prefix::Text`].
///
/// 13, not the design's 10.5: at 10.5 a capital inks 8pt tall against a Phosphor
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
const SUFFIX_PT: f32 = 11.0;
/// How far the chip hangs past the field's content box into its own padding, so
/// the chip finishes inside the hairline and belongs to the field.
const SUFFIX_BLEED: f32 = 4.0;
/// The room a unit leaves its number before it is cut short with an ellipsis:
/// four digits at the field's type size.
const UNIT_DIGITS_MIN: f32 = 28.0;

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
    let inset = (lead.size().x + PREFIX_GAP).round();
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
    let at = egui::pos2(strip.left(), strip.center().y - lead.size().y / 2.0);
    painter.galley(at, lead, ink);
    at
}

impl Prefix {
    /// The prefix as styled text: the 38% grey, with size and family per variant.
    fn rich_text(self) -> egui::RichText {
        match self {
            Prefix::Text(t) => egui::RichText::new(t)
                .size(PREFIX_PT)
                .color(theme::text::FAINT),
            Prefix::Icon(g) => theme::icon_text(g, PREFIX_ICON_PT, theme::text::FAINT),
            Prefix::Label(t) => egui::RichText::new(t)
                .size(SUFFIX_PT)
                .color(theme::text::FAINT),
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
/// Its ink: `text::DIM`, the same as the menu heads' caret.
pub const COMBO_CHEVRON: egui::Color32 = theme::text::DIM;

/// Set up a dropdown's menu rows, inside a `ComboBox::show_ui` closure.
///
/// egui 0.35 sizes a `Button`'s margin as `button_padding − bg_stroke.width` and
/// puts the stroke back on a hovered row, so rows would shift on hover. Zeroing
/// `bg_stroke` on all four states makes both branches identical; the fill is the
/// hover cue. [`MENU_ROW_H`] is the height floor.
pub fn menu_rows(ui: &mut egui::Ui) {
    ui.spacing_mut().interact_size.y = MENU_ROW_H;
    let w = &mut ui.visuals_mut().widgets;
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.bg_stroke = egui::Stroke::NONE;
    }
}

/// Height of one row in a dropdown's menu: a floor over the natural height.
///
/// A floor only raises: a `selectable_label` is 23 at the theme's `button_padding.y`
/// of 4, which would ignore 22. Every dropdown sets that padding to 0 or 2 first; a
/// new caller that does not gets a 23pt row with no error
/// (`a_menu_rows_height_does_not_depend_on_its_state`).
pub const MENU_ROW_H: f32 = 22.0;

/// Height of one row in a menu that allocates its rows exactly: the context menus
/// (`docs/context-menus.md` §8) and the top bar's View and Snap menus. Two points
/// taller than a dropdown row ([`MENU_ROW_H`]) for the glyph and accelerator
/// column; unlike that floor, an argument to [`menu_row`] and [`menu_item`].
pub const MENU_ITEM_H: f32 = 24.0;

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

/// The arrow at the closed end of a `ComboBox` — a Phosphor chevron at one fixed
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
        COMBO_CHEVRON,
    );
}

/// One Phosphor glyph as widget text, for a control whose closed state is an icon
/// and nothing else, in full-strength ink since no label sits beside it.
pub fn glyph_only(glyph: &'static str) -> egui::WidgetText {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        glyph,
        0.0,
        egui::TextFormat {
            font_id: theme::icon_font(16.0),
            color: theme::text::STRONG,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.into()
}

/// A dimmed Phosphor glyph followed by a label, as one run of widget text.
///
/// For where a [`Prefix`] cannot reach: a `ComboBox`'s `selected_text` is a single
/// `WidgetText`.
pub fn glyph_and_text(glyph: &str, text: &str) -> egui::WidgetText {
    glyph_and_text_tinted(glyph, text, theme::text::FAINT)
}

/// [`glyph_and_text`] with the glyph in a colour of the caller's choosing.
///
/// The type family row wears `TEXT_AA` in [`theme::color::WARN`] when the family
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
            font_id: theme::icon_font(15.0),
            color: glyph_col,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.append(
        text,
        7.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(12.0),
            color: color::TEXT,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.into()
}

/// A numeric field with a dimmed prefix: the design's 28px recessed row holding
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
        color::ACCENT_400.r(),
        color::ACCENT_400.g(),
        color::ACCENT_400.b(),
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
pub const BADGE_PT: f32 = 10.5;
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
        let speed = scrub_speed_for_value(ui.ctx(), scrub.speed);
        *value = scrub.settle(*value + f64::from(lead_resp.drag_delta().x) * speed);
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
        egui::pos2(strip.left(), rect.center().y - lead.size().y / 2.0),
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
            // Rounded so the reserved space and the paint point are one number.
            let inset = (lead.size().x + PREFIX_GAP).round();

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
            let lead_resp = ui.interact(
                strip,
                ui.auto_id_with("prefix-scrub"),
                // Not `click_and_drag()`, which is `FOCUSABLE`: no tab stop here.
                egui::Sense::CLICK | egui::Sense::DRAG,
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
                let speed = scrub_speed_for_value(ui.ctx(), scrub.speed);
                *value = scrub.settle(*value + f64::from(lead_resp.drag_delta().x) * speed);
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
            resp = resp.union(lead_resp);
            if nudged || *value != before {
                resp.mark_changed();
            }
            // Painted after the value, so a selection block cannot land on the label.
            let at = egui::pos2(strip.left(), resp.rect.center().y - lead.size().y / 2.0);
            match mark {
                None => {
                    ui.painter().galley(at, lead, theme::text::FAINT);
                }
                Some(_) if ui.rect_contains_pointer(strip.union(resp.rect)) => {
                    ui.painter().text(
                        strip.left_center() + egui::vec2(lead.size().x / 2.0, 0.0),
                        egui::Align2::CENTER_CENTER,
                        icon::ARROW_COUNTER_CLOCKWISE,
                        theme::icon_font(12.0),
                        theme::text::STRONG,
                    );
                }
                Some(_) => {
                    let w = lead.size().x;
                    ui.painter().galley(at, lead, theme::text::STRONG);
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
                let mut ink = theme::text::FAINT;
                if s.clickable {
                    // Sensed before it is painted, so the ink is this frame's answer;
                    // wide, since two characters are small to aim at. No `FOCUSABLE`
                    // (a tab stop) and no pointing hand: the chrome never sets one.
                    let hit =
                        ui.interact(hit_rect, ui.auto_id_with("unit-suffix"), egui::Sense::CLICK);
                    let hit = if s.tooltip.is_empty() {
                        hit
                    } else {
                        hit.on_hover_text(s.tooltip)
                    };
                    if hit.is_pointer_button_down_on() {
                        ink = color::TEXT;
                    } else if hit.hovered() {
                        ink = theme::text::STRONG;
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

    // Painted after the row, on top of the field's own hairline.
    if scope.inner.0.has_focus() {
        ui.painter().rect_stroke(
            scope.response.rect,
            egui::CornerRadius::same(5),
            egui::Stroke::new(1.0, color::ACCENT_700),
            egui::StrokeKind::Inside,
        );
    }
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
        color::ACCENT_100
    } else if hovered {
        color::TEXT
    } else {
        resting
    };
    if active {
        p.rect_filled(rect, r5, color::ACCENT_800);
        p.rect_stroke(
            rect,
            r5,
            egui::Stroke::new(1.0, color::ACCENT),
            egui::StrokeKind::Inside,
        );
    } else if hovered {
        p.rect_filled(rect, r5, theme::color::HOVER);
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
    /// confirming button of a card that destroys something. Loud at rest.
    Danger,
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

/// The accent hairline around whatever the keyboard has focused, like CSS
/// `:focus-visible`. Call it once at the end of a modal's body (`settings::card`
/// does). It paints nothing unless a key put the focus there: `Tab` turns the mode
/// on and any pointer press turns it off. The rect is last frame's, since the
/// focused widget may not be drawn yet.
pub fn focus_ring(ui: &egui::Ui) {
    let ctx = ui.ctx();
    // Two reads and a write, never nested: the context lock is not re-entrant.
    let tabbed = ctx.input(|i| i.events.iter().any(is_tab));
    let pointed = ctx.input(|i| i.pointer.any_pressed());
    let key = egui::Id::new("focus-visible");
    let visible = ctx.data_mut(|d| {
        let was = d.get_temp::<bool>(key).unwrap_or(false);
        let now = (was || tabbed) && !pointed;
        d.insert_temp(key, now);
        now
    });
    if !visible {
        return;
    }
    let Some(id) = ctx.memory(|m| m.focused()) else {
        return;
    };
    let Some(rect) = ctx.read_response(id).map(|r| r.rect) else {
        return;
    };
    ui.painter().rect_stroke(
        rect.expand(FOCUS_GAP),
        egui::CornerRadius::same(BUTTON_R + FOCUS_GAP as u8),
        egui::Stroke::new(1.0, color::ACCENT),
        egui::StrokeKind::Outside,
    );
}

/// Whether an event is `Tab` going down, in either direction.
fn is_tab(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key {
            key: egui::Key::Tab,
            pressed: true,
            ..
        }
    )
}

/// A square button that sits *in* a field row rather than floating on the card:
/// the recessed ground and hairline of [`field_frame`], sized to match the value
/// fields beside it (the design's flip and per-corner buttons, 28×28).
pub fn field_button(
    ui: &mut egui::Ui,
    glyph: &str,
    box_size: f32,
    glyph_size: f32,
    state: FieldButton,
) -> egui::Response {
    field_button_sized(ui, glyph, egui::vec2(box_size, box_size), glyph_size, state)
}

/// [`field_button`] on a box that need not be square — the Transform panel's row
/// of four buttons splits the card evenly between them.
pub fn field_button_sized(
    ui: &mut egui::Ui,
    glyph: &str,
    box_size: egui::Vec2,
    glyph_size: f32,
    state: FieldButton,
) -> egui::Response {
    let (rect, resp, fg) = button_face(ui, box_size, state);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(glyph_size),
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
    glyph_size: f32,
    state: FieldButton,
    mark: Option<FieldMark<'_>>,
) -> (egui::Response, bool) {
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
        egui::FontId::proportional(11.5),
        fg,
    );
    resp
}

/// The ink a resting control's content takes: [`theme::text::MUTED`] while it can
/// be used, [`theme::text::DISABLED`] when it cannot. `enabled` is ignored under an
/// already-disabled `Ui` ([`disable_unless`]): faded `MUTED` is `text::DISABLED`.
fn resting_ink(ui: &egui::Ui, enabled: bool) -> egui::Color32 {
    match enabled || !ui.is_enabled() {
        true => theme::text::MUTED,
        false => theme::text::DISABLED,
    }
}

/// The ground, hairline and hover treatment every field-row button shares, and
/// the colour its content should be drawn in. Its corner is [`BUTTON_R`], which a
/// caller repainting over the face (the dashboard's *Delete project* wash) must
/// match. `pub` for callers whose content is neither a glyph nor a word.
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
    // step, ink fixed (the danger ramp has no twin of `ACCENT_100`).
    let (bg, fg) = match (state, hovered) {
        (FieldButton::On, false) => (color::ACCENT_900, color::ACCENT_200),
        (FieldButton::On, true) => (color::ACCENT_800, color::ACCENT_200),
        (FieldButton::Danger, false) => (color::DANGER_900, color::DANGER_200),
        (FieldButton::Danger, true) => (color::DANGER_800, color::DANGER_200),
        (_, true) => (color::HOVER, color::TEXT),
        (FieldButton::Disabled, false) => (color::FIELD, dead),
        (_, false) => (color::FIELD, theme::text::MUTED),
    };
    p.rect_filled(rect, r5, bg);
    p.rect_stroke(
        rect,
        r5,
        egui::Stroke::new(
            1.0,
            match state {
                FieldButton::On | FieldButton::Set => color::ACCENT_700,
                FieldButton::Danger => color::DANGER_700,
                FieldButton::Off | FieldButton::Disabled => color::FIELD_BORDER,
            },
        ),
        egui::StrokeKind::Inside,
    );
    (rect, resp, fg)
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
const ACTION_GLYPH_PT: f32 = 15.0;
/// What an [`action_button`]'s pair keeps clear of its own corners, so an elided
/// name ends in a `…` with air after it.
const ACTION_PAD: f32 = 10.0;
/// An [`action_button`]'s word.
const ACTION_LABEL_PT: f32 = 12.0;

/// The width an [`action_button`] needs to show `label` without eliding it,
/// measured off the constants the button lays itself out with.
pub fn action_button_w(ctx: &egui::Context, label: &str) -> f32 {
    let w = ctx
        .fonts_mut(|f| {
            f.layout_no_wrap(
                label.to_owned(),
                egui::FontId::proportional(ACTION_LABEL_PT),
                theme::text::MUTED,
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
    /// The design's inset, and the gap after the leading glyph.
    const PAD_X: f32 = 8.0;
    const GAP: f32 = 8.0;
    const GLYPH_PT: f32 = 14.0;
    let (rect, resp, fg) = button_face(ui, size, state);
    let p = ui.painter();
    p.text(
        egui::pos2(rect.left() + PAD_X + GLYPH_PT / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(GLYPH_PT),
        fg,
    );
    p.text(
        egui::pos2(rect.left() + PAD_X + GLYPH_PT + GAP, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.5),
        fg,
    );
    p.text(
        egui::pos2(rect.right() - PAD_X, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        match state {
            FieldButton::On => icon::CHECK,
            _ => icon::MINUS,
        },
        theme::icon_font(GLYPH_PT),
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
        ui.interact(hit, resp.id.with("grab"), egui::Sense::drag())
    } else {
        resp
    };
    let live = enabled && (resp.hovered() || resp.dragged());
    let fg = if live {
        theme::text::MUTED
    } else {
        theme::color::text_a(77)
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

/// Paint a filled lock centred in a `size`-tall box.
///
/// Drawn rather than set in type, since the bundled `Phosphor.ttf` is the Regular
/// weight only: Phosphor's `lock-simple-fill` geometry in its 256-unit box, a body
/// from `32,80` to `224,224` (16-unit corners) and a 16-wide shackle stroke along
/// radius 40 about `128,56`. `size` is the em size a glyph would be set at.
pub fn paint_lock_filled(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    color: egui::Color32,
    rot: f32,
) {
    for shape in lock_filled_shapes(center, size, color, rot) {
        painter.add(shape);
    }
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

/// The filled hexagon a main component carries, drawn like the lock above.
/// Phosphor's `hexagon` geometry in its 256-unit box, pointy-top, so it sits in
/// the box [`crate::theme::icon::HEXAGON`] outlines at the same `size`
/// (`the_filled_hexagon_sits_on_the_outline_glyphs_ink`).
pub fn paint_hexagon_filled(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    color: egui::Color32,
) {
    painter.add(hexagon_filled_shape(center, size, color));
}

/// The polygon [`paint_hexagon_filled`] draws, centred on `center`.
fn hexagon_filled_shape(center: egui::Pos2, size: f32, color: egui::Color32) -> egui::Shape {
    const POINTS: [(f32, f32); 6] = [
        (128.0, 20.0),
        (220.0, 72.0),
        (220.0, 184.0),
        (128.0, 236.0),
        (36.0, 184.0),
        (36.0, 72.0),
    ];
    let k = size / 256.0;
    let points = POINTS
        .iter()
        .map(|(x, y)| center + egui::vec2((x - 128.0) * k, (y - 128.0) * k))
        .collect();
    egui::Shape::convex_polygon(points, color, egui::Stroke::NONE)
}

/// The two shapes [`paint_lock_filled`] draws, split out so tests can check them.
fn lock_filled_shapes(
    center: egui::Pos2,
    size: f32,
    color: egui::Color32,
    rot: f32,
) -> Vec<egui::Shape> {
    /// Body and shackle in Phosphor's 256-unit design space.
    const BODY: (f32, f32, f32, f32) = (32.0, 80.0, 224.0, 224.0);
    const PIVOT: (f32, f32) = (128.0, 56.0);
    const BAND: f32 = 40.0;
    const BAND_W: f32 = 16.0;
    /// The ink spans y = 8..224; its middle, not the box's, lands on `center`.
    const INK_MID_Y: f32 = 116.0;

    let k = size / 256.0;
    let frame = Frame2::rotated(rot, center);
    let at = |x: f32, y: f32| frame.to_screen(egui::pos2((x - 128.0) * k, (y - INK_MID_Y) * k));

    // The shackle: one polyline, so the corners meet without a seam.
    const STEPS: usize = 12;
    let mut pts = vec![at(PIVOT.0 - BAND, BODY.1)];
    for i in 0..=STEPS {
        let t = std::f32::consts::PI * i as f32 / STEPS as f32;
        pts.push(at(PIVOT.0 - BAND * t.cos(), PIVOT.1 - BAND * t.sin()));
    }
    pts.push(at(PIVOT.0 + BAND, BODY.1));

    // A `RectShape` cannot be turned, so the body is a polygon in the same frame.
    let body = Frame2::rotated(
        rot,
        at(
            (BODY.0 + BODY.2) * 0.5,
            // In the `at` frame, y is measured from INK_MID_Y.
            (BODY.1 + BODY.3) * 0.5,
        ),
    );
    vec![
        egui::Shape::line(pts, egui::Stroke::new(BAND_W * k, color)),
        egui::Shape::convex_polygon(
            body.rounded_rect(
                egui::vec2((BODY.2 - BODY.0) * k, (BODY.3 - BODY.1) * k),
                16.0 * k,
            ),
            color,
            egui::Stroke::NONE,
        ),
    ]
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
    const PAD: f32 = 2.0;
    const GAP: f32 = 2.0;
    let (track, _) =
        ui.allocate_exact_size(egui::vec2(width, cell_h + PAD * 2.0), egui::Sense::empty());
    let r5 = egui::CornerRadius::same(5);
    let p = ui.painter();
    p.rect_filled(track, r5, theme::color::text_a(13));
    // Translucent rather than the solid `FIELD_BORDER`: the track sits on several grounds.
    p.rect_stroke(
        track,
        r5,
        egui::Stroke::new(SEGMENT_BORDER, theme::color::text_a(20)),
        egui::StrokeKind::Inside,
    );

    // The hairline comes out of the cells: a 28pt control is `1 + 2 + 22 + 2 + 1`.
    let inner = track.shrink(PAD + SEGMENT_BORDER);
    let cell_w = (inner.width() - GAP * (n.saturating_sub(1)) as f32) / n.max(1) as f32;
    let mut clicked = None;
    let mut reset = false;
    if mark.is_some() && selected >= n {
        override_dot(ui.painter(), track.right_top() + egui::vec2(-5.0, 5.0));
    }
    for i in 0..n {
        let rect = egui::Rect::from_min_size(
            egui::pos2(inner.min.x + i as f32 * (cell_w + GAP), inner.min.y),
            egui::vec2(cell_w, inner.height()),
        );
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
        if on {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(4), theme::color::text_a(33));
        } else if live && resp.hovered() {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(4), theme::color::text_a(18));
        }
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

/// A centred Phosphor glyph for a [`segmented`] cell — the icon twin of
/// [`segment_label`], for the controls whose choices are pictures (alignment).
pub fn segment_glyph(painter: &egui::Painter, rect: egui::Rect, glyph: &str, on: bool) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(15.0),
        if on { color::TEXT } else { theme::text::DIM },
    );
}

/// A [`segmented`] cell showing nothing but a dash: the mixed state, with no cell
/// raised, since blank would read as a bug and a raised first cell as a lie.
pub fn segment_mixed(painter: &egui::Painter, rect: egui::Rect) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "–",
        egui::FontId::proportional(11.0),
        theme::text::FAINT,
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
const SLIDER_RAIL_H: f32 = 3.0;

/// A value between two bounds: a hairline rail, filled in accent to the knob.
/// Hand-painted: the design's rail and knob are out of reach of `Spacing` and
/// `Visuals` without restyling every `egui::Slider`. The knob's centre travels
/// between `half..width - half` so a knob touching the end never reads as short.
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
    if let Some(pos) = resp.interact_pointer_pos()
        && hi > lo
    {
        let t = f64::from(((pos.x - rect.left() - half) / travel).clamp(0.0, 1.0));
        let next = lo + t * (hi - lo);
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
            color::ACCENT_600,
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
        if resp.is_pointer_button_down_on() || resp.hovered() {
            color::TEXT
        } else {
            color::ACCENT_100
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
        (true, false) => (color::ACCENT_600, color::ACCENT_100),
        (true, true) => (color::ACCENT, color::ACCENT_100),
        (false, false) => (theme::color::text_a(33), theme::text::DIM),
        (false, true) => (theme::color::text_a(46), theme::text::MUTED),
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
pub const SWITCH_ROW_LABEL_PT: f32 = 11.5;

/// The ink of a toggle-list row's label: [`theme::text::STRONG`] when the row is
/// on, [`theme::text::MUTED`] when it is off. A row that is not a switch still
/// owes the two tiers (see [`switch_row`]).
pub fn switch_row_ink(on: bool) -> egui::Color32 {
    if on {
        theme::text::STRONG
    } else {
        theme::text::MUTED
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
                theme::text::FAINT,
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
pub const SEGMENT_LABEL_PT: f32 = 10.0;

/// Centred text for a [`segmented`] cell.
pub fn segment_label(painter: &egui::Painter, rect: egui::Rect, text: &str, on: bool) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(SEGMENT_LABEL_PT),
        if on { color::TEXT } else { theme::text::DIM },
    );
}

/// A [`segment_label`] for a cell that is present but unavailable — see
/// [`segmented_enabled`]. [`theme::text::DISABLED`], not the resting tier, or the
/// cell would look merely unselected.
pub fn segment_label_disabled(painter: &egui::Painter, rect: egui::Rect, text: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(SEGMENT_LABEL_PT),
        theme::text::DISABLED,
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
/// a rounded square with [`SWATCH_HAIRLINE`] round it, the checkerboard under a
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
        egui::Stroke::new(1.0, SWATCH_HAIRLINE),
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
/// White, not the picker's black, which is invisible on a dark chip.
///
/// 76/255 is set by the black chip: grey 76 against `color::FIELD`'s 47. The
/// picker's `stop_bar`, `sv_plane`, `hue_slider` and `alpha_slider` mask without
/// it; their ground is `color::CARD` under a bright ramp, which wants translucent
/// black. Premultiplied for `const`; equals `Color32::from_white_alpha(76)`.
pub const SWATCH_HAIRLINE: egui::Color32 = egui::Color32::from_rgba_premultiplied(76, 76, 76, 76);

/// A small colour chip: checkerboard, the paint, and [`SWATCH_HAIRLINE`] over the
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
        egui::Stroke::new(1.0, SWATCH_HAIRLINE),
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
        ui.painter()
            .rect_filled(rect.expand(3.0), egui::CornerRadius::same(7), color::HOVER);
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
        for prefix in [Prefix::Text("W"), Prefix::Icon(icon::ARROW_CLOCKWISE)] {
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
        for prefix in [Prefix::Text("W"), Prefix::Icon(icon::ARROW_CLOCKWISE)] {
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
            Prefix::Icon(icon::ARROW_CLOCKWISE),
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

    /// Rasterizes `lock_filled_shapes` onto a pixel grid, `true` where it inks.
    fn lock_coverage(size: f32, center: egui::Pos2, w: usize, h: usize) -> Vec<bool> {
        let mut tess = egui::epaint::tessellator::Tessellator::new(
            1.0,
            egui::epaint::TessellationOptions::default(),
            [16, 16],
            vec![],
        );
        let mut mesh = egui::epaint::Mesh::default();
        for s in lock_filled_shapes(center, size, egui::Color32::WHITE, 0.0) {
            tess.tessellate_shape(s, &mut mesh);
        }
        let side = |p: egui::Pos2, q: egui::Pos2, r: egui::Pos2| {
            (p.x - r.x) * (q.y - r.y) - (q.x - r.x) * (p.y - r.y)
        };
        let mut grid = vec![false; w * h];
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[tri[i] as usize].pos);
            for y in 0..h {
                for x in 0..w {
                    let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                    let (d0, d1, d2) = (side(p, a, b), side(p, b, c), side(p, c, a));
                    let mixed =
                        (d0 < 0.0 || d1 < 0.0 || d2 < 0.0) && (d0 > 0.0 || d1 > 0.0 || d2 > 0.0);
                    grid[y * w + x] |= !mixed;
                }
            }
        }
        grid
    }

    /// The filled lock has a solid body and a banded shackle (with a hole), inside its em box.
    #[test]
    fn the_filled_lock_has_a_body_and_a_shackle_with_a_hole() {
        const SIZE: f32 = 15.0;
        const W: usize = 20;
        let centre = egui::pos2(10.0, 10.0);
        let grid = lock_coverage(SIZE, centre, W, W);
        let ink = |x: usize, y: usize| grid[y * W + x];

        let rows: Vec<usize> = (0..W).filter(|y| (0..W).any(|x| ink(x, *y))).collect();
        let cols: Vec<usize> = (0..W).filter(|x| (0..W).any(|y| ink(*x, y))).collect();
        let (top, bottom) = (rows[0], rows[rows.len() - 1]);
        let (left, right) = (cols[0], cols[cols.len() - 1]);

        // One pixel of slack for rounding.
        let half = SIZE / 2.0;
        assert!(
            top as f32 >= centre.y - half - 1.0 && (bottom + 1) as f32 <= centre.y + half + 1.0,
            "lock ink spans rows {top}..{bottom}, outside a {SIZE}px box at {centre:?}"
        );
        assert!(
            left as f32 >= centre.x - half - 1.0 && (right + 1) as f32 <= centre.x + half + 1.0,
            "lock ink spans columns {left}..{right}"
        );

        let width_of = |y: usize| (0..W).filter(|x| ink(*x, y)).count();
        let widest = (top..=bottom).max_by_key(|y| width_of(*y)).unwrap();
        assert!(
            widest > (top + bottom) / 2,
            "the widest row ({widest}) should be the body, in the lower half"
        );

        // A row above the body with a gap in the middle: the shackle's two legs.
        let banded = (top..widest).any(|y| {
            let on: Vec<usize> = (0..W).filter(|x| ink(*x, y)).collect();
            !on.is_empty() && on.windows(2).any(|p| p[1] - p[0] > 1)
        });
        assert!(banded, "the shackle has no hole — it is a blob, not a lock");
    }

    /// The filled hexagon sits inside the outline `HEXAGON` glyph's ink box by about half the stroke on every side; the glyph is pointy-top.
    #[test]
    fn the_filled_hexagon_sits_on_the_outline_glyphs_ink() {
        const SIZE: f32 = 64.0;
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let galley = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                theme::icon::HEXAGON.to_owned(),
                theme::icon_font(SIZE),
                egui::Color32::WHITE,
            )
        });
        let row = &galley.rows[0];
        let g = &row.row.glyphs[0];
        let uv = g.uv_rect;
        assert!(!uv.is_nothing(), "the glyph rasterized");
        let cell = g.logical_rect().translate(row.pos.to_vec2());
        let origin = row.pos + g.pos.to_vec2() + uv.offset;
        // Locals first: nested `Context` accessors deadlock.
        let atlas = ctx.fonts(|f| f.image());
        let ppp = ctx.pixels_per_point();
        let (gw, gh) = (
            usize::from(uv.max[0] - uv.min[0]),
            usize::from(uv.max[1] - uv.min[1]),
        );
        let ink = |x: usize, y: usize| {
            let (ax, ay) = (usize::from(uv.min[0]) + x, usize::from(uv.min[1]) + y);
            atlas.pixels[ay * atlas.size[0] + ax].a() > 127
        };
        let rows: Vec<usize> = (0..gh).filter(|y| (0..gw).any(|x| ink(x, *y))).collect();
        let cols: Vec<usize> = (0..gw).filter(|x| (0..gh).any(|y| ink(*x, y))).collect();
        let (r0, r1) = (rows[0], *rows.last().unwrap());
        let (c0, c1) = (cols[0], *cols.last().unwrap());
        let ink_box = egui::Rect::from_min_max(
            origin + egui::vec2(c0 as f32, r0 as f32) / ppp,
            origin + egui::vec2((c1 + 1) as f32, (r1 + 1) as f32) / ppp,
        );
        let poly =
            hexagon_filled_shape(cell.center(), SIZE, egui::Color32::WHITE).visual_bounding_rect();
        let half_stroke = SIZE * 8.0 / 256.0;
        for (side, gap) in [
            ("left", poly.left() - ink_box.left()),
            ("right", ink_box.right() - poly.right()),
            ("top", poly.top() - ink_box.top()),
            ("bottom", ink_box.bottom() - poly.bottom()),
        ] {
            assert!(
                (gap - half_stroke).abs() <= 1.5,
                "{side}: the polygon sits {gap:.2} inside the ink, want about \
                 {half_stroke:.2} — ink {ink_box:?}, polygon {poly:?}"
            );
        }
        let width = |y: usize| (0..gw).filter(|x| ink(*x, y)).count();
        let near_top = width(r0 + (r1 - r0) / 10);
        let mid = (r0 + r1) / 2;
        let middle = (0..gw).rev().find(|x| ink(*x, mid)).unwrap_or(0)
            - (0..gw).find(|x| ink(*x, mid)).unwrap_or(0);
        assert!(
            (near_top as f32) < middle as f32 / 2.0,
            "the outline glyph is not pointy-top: {near_top} near the top, {middle} across"
        );
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
                crate::theme::icon_text(icon::EYE, 16.0, theme::text::MUTED),
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
                crate::theme::icon_text(icon::EYE, 16.0, theme::text::MUTED),
                1.0,
                false,
            )
            .clicked();
        });
        assert!(clicked, "a press on the glyph did not reach the head");
    }

    /// A menu head hovered or `open` paints the `HOVER` ground and mark and caret in `TEXT`; at rest it paints neither.
    ///
    /// flip: ignoring `open` → fails "an open head stays lit with the pointer away".
    #[test]
    fn a_menu_head_lights_under_the_pointer_like_an_icon_button() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let head = |ui: &mut egui::Ui, open: bool| {
            menu_head(
                ui,
                crate::theme::icon_text(icon::EYE, 16.0, theme::text::MUTED),
                5.0,
                open,
            )
        };
        // Answers (rect, ground fills, text runs in TEXT, text runs) for one frame.
        let frame = |ctx: &egui::Context, raw: egui::RawInput, open: bool| {
            let mut rect = egui::Rect::NOTHING;
            let out = ctx.run_ui(raw, |ui| rect = head(ui, open).rect);
            let (mut grounds, mut lit, mut texts) = (0, 0, 0);
            for cs in &out.shapes {
                match &cs.shape {
                    egui::Shape::Rect(r) if r.fill == theme::color::HOVER => grounds += 1,
                    egui::Shape::Text(t) => {
                        texts += 1;
                        if t.override_text_color == Some(color::TEXT) {
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
                    crate::theme::icon_text(icon::EYE, 16.0, theme::text::MUTED),
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
                    crate::theme::icon_text(icon::EYE, GLYPH, theme::text::MUTED),
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
                field_button(ui, icon::FRAME_CORNERS, H, 15.0, FieldButton::Off);
            });
        });

        let grounds: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill == color::FIELD => Some(r.rect),
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
                    ui.label(egui::RichText::new("A").color(theme::text::MUTED));
                    ui.label("B");
                    let p = ui.painter();
                    p.text(
                        egui::pos2(50.0, 50.0),
                        egui::Align2::LEFT_TOP,
                        "C",
                        egui::FontId::proportional(12.0),
                        theme::text::MUTED,
                    );
                    p.rect_filled(
                        egui::Rect::from_min_size(egui::pos2(0.0, 80.0), egui::vec2(9.0, 9.0)),
                        egui::CornerRadius::ZERO,
                        color::ACCENT,
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
                        color::ACCENT,
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
        const OFF_HIDDEN_ROW: u8 = 95; // HIDDEN_OPACITY (0.6) over MUTED
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
            theme::text::DISABLED.a(),
            "the two routes to `disabled` paint different inks: a faded scope gives \
             {scoped}, the constant {}",
            theme::text::DISABLED.a()
        );
        for (what, off) in [
            ("a hidden row", OFF_HIDDEN_ROW),
            ("a layer row", OFF_LAYER_ROW),
        ] {
            let gap = i32::from(off) - i32::from(theme::text::DISABLED.a());
            assert!(
                gap >= MARGIN,
                "{what} is off at {off} and a disabled control is at {}, only {gap} \
                 levels apart — dim alone cannot say which",
                theme::text::DISABLED.a()
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
                        swatch(ui, 16.0, color::FIELD, Swatch::Solid(chip));
                    });
                });
            })
        };

        let faded = mask_alpha(&draw(false), color::FIELD).expect("a corner mask, faded");
        let kept = mask_alpha(&draw(true), color::FIELD).expect("a corner mask, escaped");
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

    /// One swatch's frame, plus the rect it allocated.
    fn draw(chip: egui::Color32) -> (egui::FullOutput, egui::Rect) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut rect = egui::Rect::NOTHING;
        let out = ctx.run_ui(Default::default(), |ui| {
            rect = swatch(ui, 16.0, color::FIELD, Swatch::Solid(chip)).rect;
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

    /// A chip the colour of its row (`2F2F2F` on `FIELD`, and black, which sets `MARGIN`) stays visible.
    ///
    /// flip: `from_black_alpha(76)` for the hairline → fails the margin on the `2F2F2F` chip.
    #[test]
    fn a_chip_the_colour_of_its_row_is_still_visible() {
        const MARGIN: i32 = 25;
        for (name, chip) in [
            (
                "the reported 2F2F2F",
                egui::Color32::from_rgb(0x2F, 0x2F, 0x2F),
            ),
            ("black", egui::Color32::BLACK),
        ] {
            let (out, _) = draw(chip);
            let (_, line) = hairline(&out);
            let edge = over(line.stroke.color, chip);
            assert!(
                apart(edge, color::FIELD) >= MARGIN,
                "{name}: the chip's edge composites to {edge:?}, only {} levels from the \
                 row's own {:?} — the chip reads as a hole in the row",
                apart(edge, color::FIELD),
                color::FIELD,
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
            theme::color::text_a(20),
            "the design's `text 8%`, and translucent so it works on a card, in a \
             popover and in the dashboard header alike"
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
                egui::Shape::Rect(r) if r.fill == color::FIELD => Some(r.rect),
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

    /// Tab goes from one field's digits to the next field's and stops on no other rect (prefix strip, unit).
    ///
    /// `None` in the sweep is egui's own end-of-list gap.
    #[test]
    fn tabbing_a_row_of_fields_steps_from_digits_to_digits() {
        let (order, unknown) = tab_sweep();
        assert!(
            unknown.is_empty(),
            "tab stopped on {} rect(s) that are not a field's digits: {unknown:?}",
            unknown.len()
        );
        assert_eq!(
            order.iter().flatten().take(2).collect::<Vec<_>>(),
            ["X digits", "Y digits"],
            "X should reach Y in one press; full sweep was {order:?}"
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
                        let ry = value_field_suffixed(
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
                        )
                        .0;
                        if ids.is_empty() {
                            ids.push((rx.id, "X digits".into()));
                            ids.push((ry.id, "Y digits".into()));
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
                field_button(ui, icon::LINK_SIMPLE, H, 15.0, FieldButton::Off);
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
                    icon::ARROW_COUNTER_CLOCKWISE,
                    icon::ARROW_CLOCKWISE,
                    icon::FLIP_HORIZONTAL,
                    icon::FLIP_VERTICAL,
                ] {
                    field_button_sized(ui, g, egui::vec2(quarter, H), 15.0, FieldButton::Off);
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
                field_button(ui, icon::EYE, H, 14.0, FieldButton::Off);
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
        field(&ctx, vec![egui::Event::PointerMoved(to)]);
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

    /// A closed dropdown paints one chevron glyph at the design's size and ink, and not egui's filled triangle.
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
        assert_eq!(format.color, COMBO_CHEVRON);

        assert!(
            !painted.iter().any(is_triangle),
            "egui's own arrow is a 3-point convex polygon; none should be drawn"
        );
        assert!(
            shapes(false).iter().any(is_triangle),
            "…and without the icon it is, which is what this test is about"
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
            rects(color::FIELD)
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

    /// How many rows painted the hover ground (the only `text_a(20)` rect a bare `menu_row` draws).
    fn grounds(out: &egui::FullOutput) -> usize {
        out.shapes
            .iter()
            .filter(|c| match &c.shape {
                egui::Shape::Rect(r) => r.fill == theme::color::text_a(20),
                _ => false,
            })
            .count()
    }

    /// A keyboard highlight paints exactly one ground even with the pointer resting on another row (`docs/context-menus.md` §8).
    ///
    /// flip: `row.highlight == Some(true) || resp.hovered()` → the count is 2.
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
                            menu_row(ui, style, MENU_ROW_H);
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
}

/// The type family row's warning state (`glyph_and_text_tinted`).
#[cfg(test)]
mod family_row_chrome_tests {
    use super::*;

    /// A `WARN`-tinted glyph survives into the galley a `ComboBox` paints, while the label stays full-strength `TEXT`.
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
                        theme::color::WARN,
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
            colours.contains(&theme::color::WARN),
            "the warning colour never reached the galley: {colours:?}"
        );
        assert!(
            colours.contains(&color::TEXT),
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

    /// The knob's shadow bleeds exactly one point below the row (the design's `box-shadow: 0 1px 3px`) and the other three sides are tight.
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
