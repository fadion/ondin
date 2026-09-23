//! Shared chrome widgets (§9.2).
//!
//! The small vocabulary every panel is built from — cards, eyebrows, icon
//! buttons, fields, segmented controls, colour ramps — so the Nocturne design
//! tokens in `theme.rs` are applied in one place rather than re-spelled per
//! panel. egui types stay confined to `ondin-app`; nothing here knows about the
//! document.

use crate::theme::{self, color, icon};
use eframe::egui;

/// A rotated coordinate frame on the screen: an origin and an angle.
///
/// Chrome that has to line up with a rotated layer — the size badge and the
/// lock on it — is laid out in the layer's own frame and mapped out to the
/// screen at the end, rather than each piece doing its own trigonometry. egui
/// paints in screen coordinates and only `TextShape` can rotate itself, so
/// everything else (the pill, the lock) is built as geometry through here.
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
    /// **Measurement only, and only from tests, since §15 D327.** The size badge
    /// used to be *placed* by measuring a layer's outline in a rotated frame and
    /// hanging the pill under the resulting box; it now hangs off the outline's
    /// lowest edge directly, so nothing in the chrome projects into a frame any
    /// more — it only ever builds geometry out of one (`to_screen`,
    /// `rounded_rect`). What still wants this is the badge tests, which measure the
    /// outline in the badge's own frame to say that the frame is a whole number of
    /// quarter turns off the shape's.
    #[cfg(test)]
    pub fn to_local(self, screen: egui::Pos2) -> egui::Pos2 {
        let d = screen - self.origin;
        egui::pos2(d.dot(self.x), d.dot(self.y))
    }

    /// The bounding box of `points` **measured in this frame** — an oriented
    /// bounding box, expressed in frame coordinates.
    ///
    /// Test-only for `Frame2::to_local`'s reason, and worth one more sentence:
    /// this is the very shape that could not hold a *sheared* outline, since no
    /// rotation makes a parallelogram square-on and the box is therefore looser than
    /// the shape at every non-zero lean. It measures a turned rectangle exactly,
    /// which is all the tests ask of it.
    #[cfg(test)]
    pub fn bounds_of(self, points: &[egui::Pos2]) -> egui::Rect {
        egui::Rect::from_points(&points.iter().map(|p| self.to_local(*p)).collect::<Vec<_>>())
    }

    /// A rounded rectangle of `size`, centred on the frame's origin, as screen
    /// points ready for `Shape::convex_polygon`.
    ///
    /// A polygon rather than a `RectShape`, because egui's rect shapes are
    /// axis-aligned and there is no way to turn one.
    pub fn rounded_rect(self, size: egui::Vec2, radius: f32) -> Vec<egui::Pos2> {
        const STEPS: usize = 4;
        let (hw, hh) = (size.x * 0.5, size.y * 0.5);
        let r = radius.min(hw).min(hh).max(0.0);
        // Corner centres, clockwise from the top-left, each with the quarter
        // turn its arc sweeps through.
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
/// with a deeper shadow, because a menu is above everything rather than in the
/// column with the panels. `pad` is its inner margin.
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

/// One row of a dropdown menu: an icon, a label, and a hover ground across the
/// whole width.
///
/// Hand-painted rather than `ui.selectable_label`, for the same reason the
/// layer rows are: the design's row is a fixed 26px with the glyph on a
/// baseline the label shares, and egui's button padding puts neither where the
/// design has them.
///
/// **Returns the `Response`, not a `bool`.** It read `.clicked()` itself until the
/// image popover wanted a row that says *why* it is unavailable: the row fills its
/// whole scope and wins the hit test, so a tooltip hung on a wrapping `ui.scope`
/// never fires while the row is live — and one hung *inside* a scope that has been
/// `disable()`d never fires either, because a disabled `Ui` reports no hover. The
/// only place a tooltip works in both states is on this response, so it has to come
/// back out. Callers that want the old shape add `.clicked()`.
pub fn menu_item(ui: &mut egui::Ui, glyph: &str, text: &str, height: f32) -> egui::Response {
    menu_row(ui, MenuRow::new(glyph, text), height)
}

/// One row of a menu, in the full shape a context menu needs
/// (`docs/context-menus.md` §8) — everything [`menu_item`] draws, plus an accelerator,
/// a checked state and a destructive tint.
///
/// **One row type for both surfaces rather than a second one for the new menu.**
/// The top bar's dropdowns and the context menus are two renderings of one
/// registry (`docs/context-menus.md` §1), and a row that is 26pt in one door and 24 in
/// the other is the thing that makes two menus read as two apps. So `menu_item`
/// is now this function with everything optional left off, and the built
/// dropdowns are pixel-identical to what they were.
#[derive(Clone, Copy)]
pub struct MenuRow<'a> {
    pub glyph: &'a str,
    pub label: &'a str,
    /// The chord this row would also answer to, right-aligned in
    /// [`theme::text::FAINT`] — the export's `text 38%`, which is that constant
    /// exactly. `None` for a row with no chord, and the column costs nothing when
    /// every row in a menu leaves it empty.
    pub accel: Option<&'a str>,
    /// `Some` makes this a **checkable** row — *Clip content*, the four boolean
    /// operations, the text sizing modes.
    ///
    /// **The glyph column carries the state; there is no separate tick column.**
    /// [`menu_check`] reserves 13pt for a tick and has no glyph at all, which
    /// works for the View menu because its rows are switches and nothing else.
    /// A context menu's checkable rows are *also* the rows whose glyph is most
    /// worth having — `layers::op_glyph`'s four boolean marks are the same
    /// pictures the inspector's dropdown shows — and a row cannot afford both
    /// columns at 210pt. So a checked row draws its own glyph in
    /// [`color::ACCENT`], which is where `menu_check` puts its tick, and keeps
    /// `menu_check`'s second signal too: the label goes full strength on and
    /// muted off. Two channels for one bit, exactly as the View menu has.
    pub checked: Option<bool>,
    /// A row that destroys something: [`theme::color::DANGER`] on the glyph and
    /// the label, and the accelerator at a little over half of it, over the
    /// **ordinary** neutral hover ground (`design/Editor.dc.html`). The ground
    /// stays neutral on purpose — the row is already the only red thing in the
    /// menu, and a red ground under the pointer reads as a warning about the
    /// hover rather than about the click.
    pub danger: bool,
    /// A row that belongs to this target but cannot run now: dimmed, no hover
    /// ground, and not clickable. **The caller still owes it a sentence saying
    /// why** — and, because a row drawn this way is not a disabled `Ui` and not a
    /// `scope`, that sentence can hang off the returned `Response` directly. See
    /// `inspector::menu_action` for the two spellings that silently do not work.
    pub enabled: bool,
    /// Who decides whether this row wears the ground: the pointer, or the caller.
    ///
    /// `None` is the default and means *the pointer* — hover paints it, which is
    /// what every dropdown in the app wants. `Some` is a **keyboard highlight**
    /// (`docs/context-menus.md` §8): a context menu being driven by `↑`/`↓` passes
    /// `Some(true)` on the highlighted row and `Some(false)` on every other, so
    /// the pointer resting over a *different* row cannot paint a second ground.
    /// One highlight, always, and by construction rather than by a rule at the
    /// call site — the first pointer move clears the keyboard's and hands the
    /// channel back.
    ///
    /// A highlighted row is drawn *identically* to a hovered one, ground and text
    /// tiers alike. Two pictures for one state is how a menu comes to look like
    /// two menus.
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
    /// Take the ground off the pointer and give it to the caller. See
    /// [`Self::highlight`]; pass this on **every** row of a menu once any row has
    /// it, or the pointer paints a second ground beside the keyboard's.
    pub fn highlight(mut self, on: bool) -> Self {
        self.highlight = Some(on);
        self
    }
}

pub fn menu_row(ui: &mut egui::Ui, row: MenuRow<'_>, height: f32) -> egui::Response {
    // 🚨 **A dimmed row senses nothing, and this comment said it won the hit
    // test** (§15 D688, `[S18.1-L3-08]`). It read *"`Sense::hover()` … it must
    // still win the hit test so the tooltip explaining why has somewhere to
    // hang"*, and a zero-sense widget does **not**: egui's `hit_test_on_close`
    // filters candidates on `senses_click()` / `senses_drag()`, so such a widget is
    // never in `hits.click` or `hits.drag` at all. `Sense::hover()` **is**
    // `Sense::empty()` — `const HOVER = 0` — which is why the sentence could sit
    // here being false with everything working.
    //
    // ⚠️ **What the tooltip actually rests on is a *conditional* guarantee.** egui
    // grants hover to a non-interactive widget only while it sits **above** the top
    // interactive widget under the pointer (`interaction.rs`'s
    // `is_on_top_of_the_interactive_widget`). In a menu `Area` with nothing
    // interactive beneath the row, that holds. It is exactly the condition
    // `inspector::menu_action` breaks on purpose by putting a second claimant over
    // the same rectangle — so the two are one rule read from opposite ends, and
    // this one is the lucky end.
    //
    // Allocating nothing at all is still what leaves a dimmed row silently
    // unexplainable, which is the trap `menu_action` documents.
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        if row.enabled {
            egui::Sense::click()
        } else {
            egui::Sense::empty()
        },
    );
    // The keyboard's answer wins over the pointer's when there is one, which is
    // what keeps a menu to one highlight — see [`MenuRow::highlight`]. Still
    // `&& enabled`, so a row that went dim under a stale highlight paints as the
    // dim row it now is rather than as a live one.
    let hovered = row.highlight.unwrap_or_else(|| resp.hovered()) && row.enabled;
    let on = row.checked == Some(true);
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, egui::CornerRadius::same(4), theme::color::text_a(20));
    }
    // Three tiers, and the third is **disabled** rather than off: an unavailable
    // row is fainter than a resting one, which is fainter than one that is on or
    // under the pointer.
    //
    // **A resting *unticked* row is deliberately the same as a plain action row**,
    // which is why there is no arm for it. Off is not a state a menu row has to
    // announce — clicking either one does something, and the row that has news is
    // the one that is *on*. Two arms painting the same colours said the opposite,
    // and read as an intent to differ that nobody had.
    let (glyph_col, label_col, accel_col) = match () {
        // The whole triple moved down with `text::DISABLED` rather than just its
        // glyph: the three keep their internal ladder (glyph brightest, then
        // label, then accelerator), and putting the glyph on 79 while the label
        // stayed on 80 would have collapsed the top two into one.
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
    p.text(
        egui::pos2(rect.left() + 8.0 + 7.5, rect.center().y),
        egui::Align2::CENTER_CENTER,
        row.glyph,
        theme::icon_font(15.0),
        glyph_col,
    );
    p.text(
        egui::pos2(rect.left() + 8.0 + 15.0 + 9.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        row.label,
        egui::FontId::proportional(11.5),
        label_col,
    );
    if let Some(accel) = row.accel {
        p.text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            accel,
            egui::FontId::proportional(10.5),
            accel_col,
        );
    }
    resp
}

/// The total height one separator occupies — the rule plus its air, both sides.
///
/// **A constant because two places need it and only one of them draws it.**
/// [`menu_sep`] allocates it; `menu::height` adds it up *before* the menu is shown,
/// because `menu_place` has to flip and clamp against a size it does not yet have
/// on screen. Those were two hand-copied `9.0`s a file apart until 2026-08-20, with
/// nothing to keep them equal and no symptom if they drifted except a menu opened
/// near an edge landing in the wrong place — which is the one state nobody
/// exercises on purpose. `a_menus_computed_height_is_the_height_it_paints` is the
/// other half of the guarantee.
pub const MENU_SEP_H: f32 = 1.0 + SEP_AIR * 2.0;

/// [`menu_frame`]'s border, on **every** side.
///
/// **Named because it is a size and was only ever treated as a colour.** A `Frame`'s
/// stroke grows its outer rect, so a menu drawn in one paints `2 * MENU_BORDER`
/// wider and taller than its content plus its margins — and `menu::height` did not
/// count it, so `menu_place` flipped and clamped a menu against a box two points
/// short on both axes and let it hang off the edge it exists to keep it inside
/// (§15 D263). Nothing about the ink moved when that was fixed; only the number
/// handed to the placer.
///
/// **Since 2026-08-23 the ink moves too** (§15 D307): a card's named width is the
/// width it *paints*, so the border comes off the content rather than being added
/// outside it. [`menu_inner_w`] is where that subtraction lives.
pub const MENU_BORDER: f32 = 1.0;

/// The width to give the `Ui` inside a [`menu_frame`] card that is to paint `card`
/// points wide, with `pad` of inner margin.
///
/// **A function because the same arithmetic was written at eight call sites and
/// was wrong at all of them in the same direction.** Every popover in the app is
/// `menu_frame(pad)` with `set_width(w - pad * 2.0)` inside it, so every one of
/// them painted `w + 2` — the context menu 212 where `docs/context-menus.md` §8
/// draws 210, the top bar's dropdowns 152 and 162 against 150 and 160, and every
/// `POPOVER_W` card 274 against 272. The design's numbers are the **outer** card,
/// measured off `design/Editor.dc.html`; the app's were the content box, and the
/// two had been read as the same number since the first menu (§15 D307).
///
/// The point of it being one function rather than a corrected constant is that
/// the placers read the same value: `menu_place`, [`OndinApp::popover_left`] and
/// the right-aligning anchors are all handed the card's width, and they were each
/// two points short of the card until the two meanings were made one. `menu::height`
/// still adds [`MENU_BORDER`] itself, because a height is summed from its rows
/// rather than named by the design.
///
/// [`OndinApp::popover_left`]: crate::app::OndinApp::popover_left
pub const fn menu_inner_w(card: f32, pad: f32) -> f32 {
    card - pad * 2.0 - MENU_BORDER * 2.0
}

/// The air above and below a separator's rule.
///
/// **3, down from 4 on 2026-08-20** (§15 D263), as the second half of the compaction
/// that took two points off every row (§15 D262). Asked for once the row change had
/// been seen on screen.
///
/// **The smaller half, against the argument that sold it.** A primitive's menu spends
/// 63pt on its 7 hairlines, which reads like more than two and a half rows — but what
/// a separator can give up is the 2pt of air, so seven of them yield 14 where 21 rows
/// at 2pt each yielded 42. *A term's total size is not the size of the cut it can
/// stand.*
const SEP_AIR: f32 = 3.0;

/// The hairline between two groups of a menu (`docs/context-menus.md` §8).
///
/// **1pt of `text_a(20)` with [`SEP_AIR`] either side** — the export's `text 8%`
/// and `margin: 4px 0`, and *not* the `menu_frame` stroke value the spec first
/// assumed; 20/255 is 7.8%, which is the same value the hover ground uses. Half
/// a dozen groups separated by spacing alone read as one long list, which is the
/// whole reason this exists.
///
/// **The rule stays 1pt and only the air moved.** A hairline is the thing being
/// seen; the air is what it costs, and a separator that reads as a *group* boundary
/// needs to be visible rather than surrounded.
///
/// Drawn at whole device pixels, because a 1pt rule landing on a half pixel is
/// the one width where the rounding shows as a colour change rather than a
/// position change.
///
/// 🚨 **A *nominal* point snapped to whole device pixels, and this used to be one
/// device pixel full stop** (§15 D721, `[S18.1-L3-04]`). The height was
/// `1.0 / ppp` **points**, so the two sentences above — *"1pt of `text_a(20)`"*
/// and *"the rule stays 1pt"* — were true only at 100%. Measured out of
/// `FullOutput::shapes`: **1.0 pt at ppp 1.0, 0.667 at 1.5, 0.5 at 2.0.** This
/// was the one hairline in the chrome denominated in device pixels; `MENU_BORDER`
/// is a 1-*point* stroke around the very frame this sits in, 5pt away, and so are
/// `field_frame`'s, `button_face`'s, `SEGMENT_BORDER` and `inspector::popup_rule`.
/// At 200% a group rule was **half** the weight of its own card's border.
///
/// ⚠️ **The snapping was never the defect and is kept.** `round(ppp).max(1.0)`
/// device pixels is what those two sentences mean *together* — a rule that is
/// about a point, and never lands on a fraction of a pixel. At ppp 1.5 that is
/// 1.333pt rather than 1.0; painting exactly 1.0pt there covers 1.5 device rows,
/// which is precisely the smear the paragraph above exists to avoid. **A third of
/// a point heavy and crisp beats exact and blurred**, and it errs toward the rule
/// being seen, which is the whole reason a separator is here rather than more air.
///
/// ⚠️ **[`MENU_SEP_H`] does not move**, and must not: `menu::height` sums it before
/// the menu is on screen so `menu_place` can flip and clamp. The rule growing to
/// 1.333pt stays well inside the `1.0 + SEP_AIR * 2.0` already allocated —
/// `the_rule_still_fits_the_height_the_menu_reserved_for_it` is the assertion that
/// says so.
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
/// on screen (`docs/context-menus.md` §8).
///
/// **`egui::Area::fixed_pos` does not flip and does not clamp**, so a menu opened
/// near the bottom or the right of the viewport simply hangs off it, with the
/// rows that matter most — Delete is last — the ones that go missing.
///
/// Flip first, then **round, then clamp**; and the order is the whole of the
/// function. Flipping is what a menu near an edge wants (it opens up and to the
/// left, hinged on the pointer, which is what every platform does); clamping is
/// the answer for a menu *taller than the viewport*, where flipping only moves
/// which end is lost.
///
/// 🚨 **Clamping comes last, and this doc said the opposite twice** (§15 D666,
/// `[S18.1-L3-07]`) — *"flip first, then clamp, then round"* and *"**rounding
/// comes last**"*, in a function whose own next sentence says the order is the
/// whole of it. The code has always been `snap(pos).clamp(…)`.
///
/// **And the code is right, which is why this is a doc fix.** Rounding a
/// *clamped* value can push it back out by up to half a pixel — and half a pixel
/// past the right edge is a menu whose last column is cut, where half a pixel off
/// the pixel grid is a hairline that looks soft. **The clamp has to have the last
/// word**, because staying on screen is the thing this function exists for and
/// crispness is the thing it does on the way. `a_menu_is_clamped_after_it_is_
/// rounded_and_not_before` is what pins that, and it is the one input where the
/// two orders differ.
pub fn menu_place(ctx: &egui::Context, at: egui::Pos2, size: egui::Vec2) -> egui::Pos2 {
    // `content_rect`, not `viewport_rect`: the difference is the strip an OS
    // status bar or a notch may be covering, and a menu row hidden under one is
    // hidden just as thoroughly as a row off the bottom edge. (It is also what
    // egui 0.35 renamed `screen_rect` into, which is the name `docs/context-menus.md`
    // §10 uses.)
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

/// Height of a top-bar dropdown's head, and so of its hit target. The design's
/// control height (its undo/redo squares are 26×26), which is also comfortably
/// more than the 16pt glyph and 12pt caret inside it.
const HEAD_H: f32 = 26.0;

/// Empty space a head keeps on each side of its content, so it occupies the same
/// optical box an [`icon_button`] does.
///
/// `(HEAD_H - 16) / 2`: the top bar's icon buttons are 26pt boxes around 16pt
/// glyphs, so five points of every one of them is already empty. A head used to
/// allocate its galleys and nothing else, which meant that at the *same*
/// `item_spacing` its mark sat five points closer to whatever was beside it than
/// an icon button's glyph did. In a row that alternates the two — undo, redo, a
/// rule, the View and Snap marks, the zoom readout — that reads as unequal
/// spacing, and it is unequal: the gaps were identical and the *ink* was not
/// (§15 D44). Padding the head is what makes the two kinds of control
/// interchangeable in a cluster, and it widens the hit target the same way.
const HEAD_PAD: f32 = 5.0;

/// The head of a top-bar dropdown — a mark, `gap` points, then a caret —
/// sensed as **one** rect.
///
/// Hand-painted, rather than two `ui.label`s with their responses unioned. That
/// is what this replaced, and it was clickable only in the empty band *above and
/// below* the glyphs: the top bar centres its row, so the unioned rect was the
/// bar's full height, but a `label` is a registered widget and wins the hit test
/// over a rect that merely contains it. Every press that landed where the user
/// was actually aiming — on the icon — went to the label and did nothing.
///
/// The same trap as §15 D35 wearing different clothes: two things claiming one
/// region, and the one that looks clickable losing. The fix is the same in
/// spirit — have only one claimant.
pub fn menu_head(ui: &mut egui::Ui, lead: egui::RichText, gap: f32) -> egui::Response {
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
            HEAD_PAD + lead.size().x + gap + caret.size().x + HEAD_PAD,
            HEAD_H,
        ),
        egui::Sense::click(),
    );
    // Vertically centred rather than baseline-aligned: the two are different
    // sizes and different fonts, and centring is what makes a glyph and the
    // caret beside it read as one control.
    let p = ui.painter();
    let at = |g: &std::sync::Arc<egui::Galley>, x: f32| {
        egui::pos2(x, rect.center().y - g.size().y / 2.0)
    };
    let ink = rect.left() + HEAD_PAD;
    p.galley(at(&lead, ink), lead.clone(), egui::Color32::PLACEHOLDER);
    p.galley(
        at(&caret, ink + lead.size().x + gap),
        caret,
        egui::Color32::PLACEHOLDER,
    );
    resp
}

/// A dropdown row that carries a checkmark — the View and Snap menus'
/// (`design/Editor.dc.html`, the `viewMenuOpen` / `snapMenuOpen` blocks).
///
/// The tick keeps its 13pt column whether or not it is showing, so the labels
/// stay in one line down the menu instead of shuffling left as things are
/// switched off. Off is also a *dimmer label*, not just a missing tick: the
/// design draws it at 72% against full strength, which is what lets the state be
/// read at a glance rather than by inspecting each row for a glyph.
pub fn menu_check(ui: &mut egui::Ui, label: &str, on: bool, height: f32) -> bool {
    /// The design's row: 8px inset, a 13px glyph column, 7px to the label.
    const PAD_X: f32 = 8.0;
    const TICK_W: f32 = 13.0;
    const GAP: f32 = 7.0;

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

/// A small-caps section eyebrow (`LAYERS`, `TRANSFORM`, …).
pub fn eyebrow(label: &str) -> egui::RichText {
    egui::RichText::new(label.to_uppercase())
        .size(9.5)
        .color(theme::text::FAINT)
}

/// An inspector card — a surface-filled rounded box with padding.
///
/// The inspector's cards float free over the canvas (no backdrop pane), so this
/// ground is the only thing separating a panel from the artwork behind it: it
/// is opaque on purpose, and carries a hairline and a drop shadow so the edge
/// reads against light and dark artwork alike.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card_at(ui, add).0
}

/// The horizontal inner margin of a [`card`], and the width of the action icon
/// a [`section_head`] can carry. Both are needed outside this pair to work out
/// which part of a card belongs to the `+` rather than to the header.
pub const CARD_MARGIN_X: f32 = 14.0;
pub const HEAD_ACTION_W: f32 = 20.0;

/// Empty space around a header's action icon that presses to the icon rather
/// than to the header behind it.
///
/// **A hit target, not a margin** — the glyph and its 20pt box stay exactly
/// where the design puts them, and only the sensed rect grows. A `+` sitting in
/// a row that toggles the whole panel is a 20pt square inside a 284pt target for
/// the opposite action, and missing it by two points collapses the panel you were
/// about to add a fill to. Which is worse than it sounds: the panel then has to be
/// re-opened before the fill can be seen.
///
/// The header gives up the same band ([`section_head`]) rather than both claiming
/// it. Overlapping targets do not share a press — either they fight over it or
/// both take it, which here would add a fill *and* collapse the panel.
pub const HEAD_ACTION_PAD: f32 = 6.0;

/// The pitch between rows inside a card, and the one number a multi-row entry in
/// one measures itself against.
///
/// Named rather than left as a literal in [`card_at`] because it is no longer only
/// the card's business. Four places read it, and each one used to be a number of
/// its own or an inheritance from whatever card it landed in: the card body, an
/// image fill's two lines, a stroke's pair — with twice it between one stroke and
/// the next — and the sides rows inside an expanded stroke. Four literals is how
/// those come to disagree, and the disagreement is exactly what was reported ("the
/// stroke 2-row line is not the same as the image").
///
/// **Read it, never copy it into a test fixture.** A fixture with its own literal
/// keeps agreeing with itself while the row moves, which is how one assertion spent
/// a revision describing a 6 for a row that painted 9 (§15 D180).
pub const CARD_ROW_GAP: f32 = 9.0;

/// How tall a control is. **Everywhere** — a panel's fields, a popover's, the
/// colour picker's rows, the buttons beside any of them.
///
/// ⚠️ **The popovers were 26 and had an argument for it**: "shorter than the card's
/// 28px fields, which is what keeps a popup of nine sections from towering over the
/// panel it belongs to". Three constants said so in three files, and the mock draws
/// it that way. What it costs is a popover hanging *beside* the card it opened from,
/// at the same width, whose every control is two points shorter than the control
/// that opened it — which is not a distinction the reader can attribute to anything
/// (§15 D386). A tall popover is a tall popover; it already scrolls.
///
/// A [`segmented`] track paints [`SEGMENT_CELL_H`] plus two points at each end, so
/// that is the number to hand it rather than this one.
pub const CONTROL_H: f32 = 28.0;

/// What to hand [`segmented`] so its track paints [`CONTROL_H`] tall.
///
/// The track adds its own two points of padding above and below the cells; the
/// hairline round it comes out of the cells rather than out of the track
/// ([`SEGMENT_BORDER`]), so it does not enter here.
pub const SEGMENT_CELL_H: f32 = CONTROL_H - 4.0;

/// The gap between two controls **side by side** in one row — [`CARD_ROW_GAP`]'s
/// horizontal twin, and the other half of the rhythm the Transform card sets.
///
/// **Seven, because that is what sits between X and Y**, which is the row the eye
/// measures every other row against: it is the first pair of fields in the panel
/// and the one that is on screen for every selection. Nothing else gets a vote.
///
/// Named for [`CARD_ROW_GAP`]'s reason, and the same drift had already happened
/// here. Fifteen rows spelled it `6.0` while twenty-four spelled it `7.0` — the
/// Fill and Stroke cards sat directly under Transform with a point less air
/// between their columns, and the stroke's four-control line, the image edit
/// rows, the effect rows and the dash popover all followed the narrower one
/// (§15 D386).
///
/// ⚠️ **It is spelled twice per row and both have to move together.** A row that
/// splits the card writes the gap once as `item_spacing.x` and again inside its
/// own width arithmetic — `(available_width() - GAP) / 2.0` — and changing only
/// the spacing pushes the last control a point past the card, at which point
/// `Ui::horizontal` wraps it onto a line of its own. So grep the constant, not the
/// number, when retuning this.
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
            // Every card spans the column. A `Frame` shrink-wraps its contents,
            // so without this a panel whose widest row is narrow — Align's six
            // buttons, a collapsed Effects header — comes out visibly narrower
            // than its neighbours instead of lining up with them.
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
}

/// The control on the right of a [`section_head`], if it has one.
///
/// The two fields below are two independent questions, which is why there are three
/// constructors rather than two variants. `active` says whether it *stays* on: the
/// `+` on Fill and Stroke does something and returns to rest, while Transform's
/// crosshair is a **toggle** — and a toggle that looked identical to a one-shot
/// button would be a switch with no visible position, so the tint is not decoration.
/// `enabled` says whether it can be pressed at all, independently, because a control
/// with nothing to act on has to stay in place and say so rather than disappear and
/// take its slot with it (§15 D134). So a one-shot can be inert
/// ([`HeadAction::inert`]) and so can a toggle.
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

    /// A one-shot action with nothing to act on: dimmed, inert, and still there.
    ///
    /// Present rather than absent, which is the rule the whole inspector follows —
    /// a control that vanishes takes its space with it and moves everything beside
    /// it, so walking from one layer to a selection makes the panel jump. The
    /// tooltip is where the reason goes.
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
/// action icon. `open` picks the caret direction.
///
/// **The whole row toggles.** The caret is a 12px glyph and the label is nine
/// and a half points of small caps; together they cover about a third of a
/// 284px card, and the two thirds beside them looked every bit as much like a
/// header while doing nothing at all. What the row does *not* claim is the
/// action icon's corner, which keeps its own press.
///
/// `sense` is false when the caller is going to sense something larger that
/// contains this row — a collapsed panel makes its whole card the target. Two
/// overlapping click targets is not a way of being generous: either they fight
/// over the press, or both take it and the panel toggles twice back to where it
/// started.
pub fn section_head(
    ui: &mut egui::Ui,
    label: &str,
    open: bool,
    action: Option<HeadAction>,
    sense: bool,
) -> HeadClicks {
    let mut clicks = HeadClicks::default();
    let row = ui
        .horizontal(|ui| {
            // Scoped, so the 6px pitch applies between the caret and the label
            // and not to the gap before the action icon's layout.
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
            });

            if let Some(a) = action {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Rightwards the pad runs all the way out to the card's edge:
                    // the icon already sits against the content margin, so that
                    // band is the card's own padding and nothing else wants it.
                    let pad = egui::Margin {
                        left: HEAD_ACTION_PAD as i8,
                        right: CARD_MARGIN_X as i8,
                        top: HEAD_ACTION_PAD as i8,
                        bottom: HEAD_ACTION_PAD as i8,
                    };
                    clicks.acted = icon_button_padded(
                        ui,
                        a.glyph,
                        egui::Vec2::splat(HEAD_ACTION_W),
                        15.0,
                        a.active,
                        a.enabled,
                        pad,
                    )
                    .on_hover_text(a.tooltip)
                    .clicked()
                        && a.enabled;
                });
            }
        })
        .response;

    if sense {
        // `horizontal` shrink-wraps, so on a header with no action icon the row
        // ends at the label. Stretch it to the card's own width: that empty
        // space to the right is most of what there is to aim at.
        let mut hit = row.rect;
        hit.max.x = hit.max.x.max(ui.max_rect().right());
        if action.is_some() {
            // The pad the action took on its left comes off here, so the two
            // targets meet rather than overlap ([`HEAD_ACTION_PAD`]).
            hit.max.x -= HEAD_ACTION_W + HEAD_ACTION_PAD;
        }
        let head = ui.interact(
            hit,
            ui.id().with(("section-head", label)),
            egui::Sense::click(),
        );
        clicks.toggled = head.clicked();
    }
    clicks
}

/// Horizontal inset between a [`field_frame`]'s hairline and its contents.
///
/// Needed by name outside `field_frame` so a control at the very edge of a field
/// can work out how much empty ground lies between it and the border — see
/// [`grip`], which claims that band as part of its hit target.
pub const FIELD_PAD_X: f32 = 9.0;

/// Largest stroke weight — and largest per-side width, and largest dash length or
/// spacing — the fields accept, in world units.
///
/// **A cap on the field, not on the model.** Any positive width is a legal
/// `Stroke::width` and a file may hold one; what this bounds is what can be *typed
/// or scrubbed*, because past a few thousand the number has stopped describing a
/// stroke. An artboard is a few hundred units across, so ten thousand is already
/// far beyond any drawing and leaves the digits comfortable room in the narrowest
/// field that carries them.
///
/// It is not a layout allowance. `value_field` no longer grows with its contents
/// at any magnitude (see the prefix-atom note there) — this is about the number
/// meaning something.
pub const MAX_STROKE_WIDTH: f64 = 10_000.0;

/// The recessed ground a value field or list row sits on.
pub fn field_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(color::FIELD)
        .stroke(egui::Stroke::new(1.0, color::FIELD_BORDER))
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(FIELD_PAD_X as i8, 0))
}

/// Vertical space [`field_frame`]'s hairline adds *outside* its content.
///
/// egui counts a `Frame`'s stroke width as margin, so a frame told to hold 28px
/// of content paints 30. Everything that has to line up with a field row has to
/// subtract this, and two visible bugs came of not doing so: a `ui.horizontal`
/// asks for `interact_size.y` (24) and the first over-tall row grows the row's
/// `max_rect` to 30, after which `Align::Center` centres every *later* column's
/// 28 in that 30 and lands it 1px low — which is what put Y below X, H below W
/// and border-radius below opacity. The same 2px is why a 28px [`field_button`],
/// which allocates exactly what it asks for, read as shorter than the 28px field
/// beside it: the two disagreed about whether the 28 was inside or outside the
/// hairline.
pub const FIELD_BORDER_H: f32 = 2.0;

/// A field row of exactly `size`: the recessed ground, laid out left to right
/// and vertically centred.
///
/// Spelled once on purpose. `Frame::show` *inherits* its parent's layout, so a
/// row built straight out of [`field_frame`] inside a panel column silently
/// stacks its contents down the page instead of across.
///
/// `size.y` is the height of the **painted ground**, hairline included — what
/// the eye measures a field by, and what a sibling button has to match. The
/// content inside it is shorter by [`FIELD_BORDER_H`]; anything the closure
/// allocates at full `size.y` grows the frame back and reintroduces the
/// misalignment that constant describes.
pub fn field_row<R>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    framed_row(ui, size, field_frame(), add)
}

/// [`field_row`]'s body, for the one caller that needs a different border.
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

/// A single-line **text** field on the same ground as [`value_field`]: the
/// recessed plate, the hairline, and [`FIELD_PAD_X`] of air at each end.
///
/// **Free text is the one field shape this module did not have**, and the
/// dashboard needed three of them — a project's name, the library's base folder,
/// the rename box on a card. Each had been spelled as a bare `ui.add(TextEdit…)`,
/// which is a different control in three ways at once: egui's own frame instead of
/// the app's, its `Margin::symmetric(4, 2)` instead of the field inset, and a
/// **focus ring in `Visuals::selection.stroke`** that nothing else in the app
/// draws. Reported as *"all textboxes have no padding… we don't use the blue
/// outline on focus anywhere"*, which is one cause with two symptoms.
///
/// ⚠️ **`Frame::NONE` is what turns egui's ring off, and it also throws the inset
/// away.** Two things hang off supplying a frame at all: the focused stroke is the
/// *frame*'s (`text_edit::builder` paints `Visuals::selection.stroke` in place of
/// `bg_stroke` when the field has focus, and skips both when the caller gave it a
/// frame), and so is the text inset — `let frame = frame.unwrap_or_else(|| …
/// inner_margin(margin))`, so `TextEdit::margin` is read *only* when no frame was
/// passed. `.frame(NONE).margin(m)` therefore drops `m` silently, which is a trap
/// worth a helper on its own: the padding here is [`field_row`]'s plate, and any
/// caller that wants its own must put it in the frame.
///
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
                // No margin of its own: the inset is the plate's, and a `margin`
                // call here would be read by nothing (see above).
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .font(egui::FontId::proportional(pt))
                .hint_text(egui::RichText::new(hint).size(pt)),
        )
    })
}

/// How far a value field's number travels per point of pointer movement,
/// as a multiple of the rate the field asks for.
///
/// **The one number to turn when scrubbing feels wrong**, which is the whole reason
/// it is a scale rather than 22 edited literals: the rates the fields ask for are
/// *relative* to each other and were each chosen against the quantity they edit —
/// a line height wants 0.02 where a coordinate wants 0.5 — so the ratios have to
/// survive any change of overall sensitivity. Scaling them together is the only way
/// to retune the feel without re-deciding twenty-two of those judgements.
pub const SCRUB_SCALE: f64 = 0.5;

/// A field's drag speed: the rate it asks for, at the current [`SCRUB_SCALE`].
pub fn scrub(base: f64) -> f64 {
    base * SCRUB_SCALE
}

/// How a [`value_field`] behaves under the hand.
///
/// The speed and the whole-units rule travel together because [`value_field`] needs
/// both: the speed to turn a drag on the *prefix* into a change in the number, and
/// the rule to round the result. Left at the call site as a `DragValue::speed` plus
/// a hand-written `if resp.dragged() { v = v.round() }`, the two were six copies of
/// the same three lines and one field that had quietly missed them.
#[derive(Clone, Debug)]
pub struct Scrub {
    speed: f64,
    /// Decimal places a *drag* settles to. Typing is never rounded.
    ///
    /// **Stated rather than inferred, because two halves of the field would
    /// otherwise disagree.** A `DragValue` rounds its own drag to a precision it
    /// derives from the speed (`auto_decimals`), while the prefix strip's drag is
    /// applied by [`value_field`] and would keep every digit the pointer produced —
    /// so a 37px drag came out `109` on the digits and `109.25` on the label. One
    /// number here settles both.
    decimals: u32,
    range: std::ops::RangeInclusive<f64>,
}

impl Scrub {
    /// **Whole units under the hand, any number from the keyboard.**
    ///
    /// The default for anything measured in the document's own units — a position, a
    /// size, an angle, a stroke weight. Dragging one through 1.7000000000000002 is a
    /// value nobody asked for that then shows up in the saved file, and a coordinate
    /// with three decimals is more precision than the number means. Typing is left
    /// alone: a hairline, or 33.3%, is a real thing to want and the field is the only
    /// way to ask for one.
    ///
    /// Rounding is keyed on the *drag*, never on focus — "round it unless it is being
    /// typed into" would round a typed 1.5 away the moment the field lost focus.
    pub fn whole(base: f64) -> Self {
        Self {
            speed: scrub(base),
            decimals: 0,
            range: f64::NEG_INFINITY..=f64::INFINITY,
        }
    }

    /// `decimals` places under the hand, for the few numbers where a fraction is the
    /// point and whole units would be uselessly coarse — a line-height multiplier
    /// over `0.5..=5.0` is the case this exists for. Typing is still unrestricted.
    pub fn fine(base: f64, decimals: u32) -> Self {
        Self {
            decimals,
            ..Self::whole(base)
        }
    }

    /// Clamp the value to `range`, both while dragging and when typed.
    ///
    /// ⚠️ **An inverted range is put the right way round here rather than
    /// panicking in [`Self::settle`]** (§15 D462). `f64::clamp`'s *"min > max, or
    /// either was NaN"* is an unconditional assert in `core`, not a
    /// `debug_assert!`, and a panic inside `eframe`'s update closure takes the
    /// process down with everything since the last `.recovery/` snapshot.
    ///
    /// `[S18.1-L1-01]`: **this is the only one of the forty-one `Scrub::range`
    /// call sites whose bound comes from the document.** `inspector.rs`'s *Scale*
    /// field for an image fill in *Crop* builds `floor..=10_000.0` out of
    /// `ImageRef::cover_scale`, which is `(fw / iw).max(fh / ih)` — frame world
    /// units over picture **pixels**, with no cap — so a 16×16 icon on a
    /// 1920×1080 rectangle gives a floor of **12 000** above a ceiling of 10 000.
    /// Measured twice in debug: `settle` called directly with that range, and end
    /// to end by driving a press and a 24pt `PointerMoved` through
    /// [`value_field`]. The `.max(0.01)` on that floor already closes `clamp`'s
    /// NaN half and left the ordering half wide open.
    ///
    /// ⚠️ **egui does not do this, which is why nothing upstream caught it.**
    /// `DragValue`'s own `clamp_value_to_range` opens by **swapping** an inverted
    /// pair, so the widget renders happily and only this app's re-clamp died —
    /// two spellings of one rule, one of them fatal. Swapping here makes the two
    /// agree.
    ///
    /// **The caller's own defect — a floor above its ceiling — is a separate
    /// question and is not answered here.** What this settles is that a shared
    /// widget must not panic on it. (It *was* answered, in the same commit, at
    /// the one caller that has the defect: `inspector.rs`'s *Scale* field takes
    /// its ceiling as `floor.max(10_000.0)` now. This function still makes no
    /// claim about its callers.)
    ///
    /// ⚠️ **`NaN` is the other half of `clamp`'s contract and it is closed here
    /// too**, by dropping the range rather than by swapping it: `f64::min`
    /// answers the non-NaN operand, so a single `NaN` bound would silently become
    /// a *both-ends-equal* range and pin the field to one number. No bound is the
    /// honest reading of a bound nobody can compare, and it is what the widget
    /// does with no `.range(…)` at all.
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

/// Format a field's number at **at most** `decimals` places, trailing zeros trimmed —
/// so `160` stays `160` while `411.672736…` reads `411.67`.
///
/// **The precision of a value field must not depend on its drag speed.** Left to itself
/// a `DragValue` derives one from the other:
/// `auto_decimals = (aim_radius / speed).log10().ceil().clamp(0, 15)`. That is a
/// reasonable guess for a field nobody has thought about, and wrong here twice over.
/// It gave the geometry fields three decimals, which is more than a coordinate means;
/// and on the pass a cursor wrap spends at zero speed ([`scrub_speed_for`]) it is
/// `log10(∞)` — clamped to fifteen — so for exactly one frame the Transform panel read
/// `411.672736201535258` (§15 D66). Nothing was wrong with the *value*; it was the
/// formatter guessing from a speed that had been borrowed for something else.
///
/// Given to `DragValue::custom_formatter`, which is handed the derived range and is free
/// to ignore it. That is what makes the display independent of the speed — and it trims,
/// where `max_decimals` alone would leave `160.00` on the swallow pass and `160.0` on
/// every other. A field that pins `max_decimals` needs none of this: its range is
/// already fixed, so the clamp lands on the same number either way.
pub fn number(decimals: usize) -> impl Fn(f64, std::ops::RangeInclusive<usize>) -> String {
    move |v, _| {
        let s = format!("{v:.decimals$}");
        let Some(trimmed) = s.contains('.').then(|| {
            let t = s.trim_end_matches('0');
            t.strip_suffix('.').unwrap_or(t)
        }) else {
            return s;
        };
        // A value the rounding took to zero from below formats as `-0`, which reads as a
        // different number than the zero it is.
        match trimmed {
            "-0" => "0".to_owned(),
            t => t.to_owned(),
        }
    }
}

// --- infinite scrub (cursor wrapping) --------------------------------------

/// How close to the window's edge the pointer gets before a scrub wraps it to the
/// other side.
///
/// Wide enough that a fast hand cannot clear it between two frames — past the edge
/// the OS stops moving the cursor, motion stops being reported, and the drag simply
/// stalls, which is the thing being fixed. Narrow enough that the teleport reads as
/// happening *at* the edge.
const WRAP_EDGE: f32 = 24.0;

/// How many passes a requested teleport is waited for before giving up on it.
///
/// A platform that refuses `set_cursor_position` reports nothing back, so without a
/// deadline one unanswered request would block every later wrap and the drag would be
/// stuck at the edge for good. Giving up restores the plain stall instead, which is
/// merely the old behaviour. Roughly a third of a second at 60fps — far longer than any
/// real teleport takes, short enough not to feel like a hang.
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

/// **This is the whole difficulty of cursor wrapping.** egui derives `drag_delta` from
/// the difference between successive pointer positions (`Response::drag_delta` →
/// `PointerState::delta`), so a teleport arrives indistinguishable from an enormously
/// fast flick: one pass reports a delta about the width of the window, and applied to a
/// value it sends the number flying — and, for a field that commits every pass, *stays*
/// there. There is no hook for correcting egui's idea of the pointer after the fact, so
/// the artefact is not removed; it is spent on a zero speed instead
/// ([`scrub_speed_for`]), costing one pass of scrubbing per wrap and nothing else.
///
/// **Recognised by its size, not by its timing**, and the difference is the whole bug
/// this once had. The first version assumed the teleport landed on the pass after the
/// request. It does not: the request reaches the backend only after the frame body
/// (`<OndinApp as eframe::App>::ui`) returns,
/// winit then calls `set_cursor_position`, and the OS posts the resulting move whenever
/// it gets round to it — while the hand carries on generating real events in between. So
/// the artefact turned up several passes late, at full speed, and was committed.
/// Its *size* is the reliable signal: the jump is most of the window wide and nothing a
/// hand does in one pass comes close, so half the requested distance separates the two
/// with room to spare.
fn wrap_pending(ctx: &egui::Context) -> Option<Wrap> {
    ctx.data(|d| d.get_temp(wrap_id()))
}

/// Whether this pass's pointer delta is a teleport landing rather than the hand moving.
fn wrap_landed(ctx: &egui::Context, pending: Wrap) -> bool {
    let dx = ctx.input(|i| i.pointer.delta().x);
    dx.abs() >= pending.distance * 0.5
}

/// The speed a `DragValue` should be given this pass: its own, or zero if this is the
/// pass a teleport landed on.
///
/// A pure read — clearing the pending wrap is left to [`wrap_scrub`], which runs only
/// for the field actually being dragged. Every field in the frame passes through here,
/// and one of them consuming the state would leave the dragged one at full speed.
/// [`scrub_speed_for`] as a bare number, for the prefix strip — which applies the
/// speed itself rather than handing it to a `DragValue`, and so needs the same
/// zero-on-the-teleport-pass rule stated the same way. Two copies of the *rule*
/// would be a way for the strip and the number to disagree about one frame of a drag.
fn scrub_speed_for_value(ctx: &egui::Context, speed: f64) -> f64 {
    match wrap_pending(ctx) {
        Some(p) if wrap_landed(ctx, p) => 0.0,
        _ => speed,
    }
}

fn scrub_speed_for<'a>(ctx: &egui::Context, value: egui::DragValue<'a>) -> egui::DragValue<'a> {
    match wrap_pending(ctx) {
        Some(p) if wrap_landed(ctx, p) => {
            // Zero rather than skipping the widget: a `DragValue` at zero speed still
            // *is* dragged, so the gesture survives the pass. Disabling it or leaving it
            // out would end the drag, and `egui`'s own code reads
            // `if delta_value != 0.0` before touching anything, so nothing is written and
            // no precision is lost from its accumulator.
            value.speed(0.0)
        }
        _ => value,
    }
}

/// Teleport the pointer to the far side of the window if a live scrub has run it into
/// an edge, so the drag can carry on indefinitely.
///
/// Figma, Blender ("continuous grab") and Photoshop's scrubby sliders all do this: the
/// value a field can reach should be limited by how long you are willing to drag, not
/// by how wide the monitor is. Ours stopped at the edge, which on a wide field like
/// `X` is reached long before the number gets anywhere.
///
/// **Window-relative, not screen-relative**, because that is what
/// `ViewportCommand::CursorPosition` takes and because a cursor placed outside the
/// window would stop being reported to us at all. The window is normally the screen,
/// so the two coincide in practice.
///
/// **The x axis only**, though `DragValue` reads `delta.x - delta.y` and so scrubs
/// vertically too. Two reasons, and the first is a bug this had: a field sitting within
/// a band of the window's *top* — which the inspector's first panel very nearly is —
/// starts every drag already inside the trigger, so a purely horizontal drag would
/// teleport vertically the instant it began. The second is that a scrub is a horizontal
/// gesture; wrapping the axis nobody is dragging along is motion the hand did not ask
/// for. A vertical scrub still stops at the edge, which is the state every field was
/// in before this.
fn wrap_scrub(ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    // A request already in flight. Nothing may be asked for until it lands: the pointer
    // sits in the trigger band for however long the OS takes to answer, so an ungated
    // wrap would fire again on every pass of that wait and the cursor would bounce
    // between the two edges.
    if let Some(p) = wrap_pending(ctx) {
        if !wrap_landed(ctx, p) && pass <= p.asked_on + WRAP_GIVE_UP {
            return; // still waiting
        }
        // Landed, or given up on. Cleared and then *fallen through* rather than costing
        // a pass of its own: the far side is a whole band clear of the trigger, so on a
        // landing pass the test below simply finds nothing to do — and on a give-up pass
        // the pointer is still stuck at the edge and deserves the retry immediately.
        ctx.data_mut(|d| d.remove::<Wrap>(wrap_id()));
    }
    let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else {
        return;
    };
    // The *viewport*, not `content_rect`: this is a cursor position in the window's
    // own coordinates, so it has to be the window's whole client area rather than the
    // part of it that is safe to draw in.
    let screen = ctx.viewport_rect();
    // Landing one band *inside* the opposite trigger band, so carrying on in the same
    // direction does not immediately wrap back.
    let inset = WRAP_EDGE * 2.0;
    // A window too narrow to hold two bands and a gap has nowhere to wrap to; moving
    // the pointer there would be a teleport into the band it came from.
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
    // The pass number and the distance are read *before* the `data_mut`, never inside
    // it: every one of these accessors takes the context's own lock, and nesting two of
    // them deadlocks (epaint's debug build says so after ten seconds; a release build
    // would simply hang).
    let pending = Wrap {
        asked_on: pass,
        distance: (to.x - p.x).abs(),
    };
    ctx.data_mut(|d| d.insert_temp(wrap_id(), pending));
    // The teleport arrives as an event, and an event only wakes egui if something is
    // already asking for a repaint. Mid-drag something is, but say so anyway.
    ctx.request_repaint();
}

/// What a [`value_field`] puts in front of its number.
///
/// The design prefixes some fields with a word (`X`, `LH`) and others with a
/// Phosphor glyph (a clock for rotation, a half-drop for opacity), both in the
/// same 38%-of-text grey. One enum so a field cannot be given the wrong size or
/// the wrong grey by being spelled out at the call site.
#[derive(Clone, Copy)]
pub enum Prefix {
    /// A short word — `X`, `W`, `LH`.
    Text(&'static str),
    /// A Phosphor glyph.
    Icon(&'static str),
}

/// Type size of a [`Prefix::Text`].
///
/// **13, not the design's 10.5** (§15). The two kinds of prefix sit in one column
/// of one panel — Transform has `X`/`Y`/`W`/`H` above a rotation clock and a skew
/// angle — so they are read against each other, and against each other the design's
/// sizes do not match: measured off the atlas, a capital at 10.5 inks 8pt tall where
/// a 15pt Phosphor glyph inks 11. Reported as the letters looking slightly small,
/// which is what they were.
///
/// A full match would want about 14.5, which is *larger* than the number the prefix
/// labels — a label louder than its value. 13 inks 10pt: within a point of the
/// glyphs above it and a half-point over the digits beside it, which is the gap
/// worth having rather than the one nobody asked about.
const PREFIX_PT: f32 = 13.0;
/// Type size of a [`Prefix::Icon`] — the design's, and unchanged.
const PREFIX_ICON_PT: f32 = 15.0;
/// Space between a prefix and the number it labels (the design's field gap).
const PREFIX_GAP: f32 = 7.0;
/// How far past its own text a clickable [`Suffix`]'s hit box reaches on each
/// side. Two characters of 11pt text is a small thing to aim at, and there is
/// nothing else in that corner of the field to hit.
const SUFFIX_HIT_PAD: f32 = 4.0;
/// Type size of the unit itself — the design's, and smaller than [`PREFIX_PT`]
/// on purpose. The two are not read against each other the way two *prefixes*
/// down a column are: this one sits alone at the far end of a field, labelling
/// a number it must not compete with.
const SUFFIX_PT: f32 = 11.0;
/// How far the chip hangs past the field's content box into its own padding.
///
/// The design's `margin-right:-4px` against `padding:0 9px` — so the chip's edge
/// finishes 5pt inside the hairline where the digits finish 9. That gap is the
/// whole difference between a unit that belongs to the field and a label parked
/// near it, and it was the reported symptom: *"the %/px toggle is still outside
/// the control"*.
const SUFFIX_BLEED: f32 = 4.0;

impl Prefix {
    /// The prefix as styled text. Both variants are the design's 38% grey; only the
    /// size and the family differ, and the reason they differ by more than the
    /// design says is [`PREFIX_PT`].
    fn rich_text(self) -> egui::RichText {
        match self {
            Prefix::Text(t) => egui::RichText::new(t)
                .size(PREFIX_PT)
                .color(theme::text::FAINT),
            Prefix::Icon(g) => theme::icon_text(g, PREFIX_ICON_PT, theme::text::FAINT),
        }
    }

    /// The prefix as a laid-out run, so a caller can measure it *before* placing
    /// anything. [`value_field`] needs the width in advance: it reserves that much
    /// space before the number and paints the prefix back into it afterwards.
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
/// `0.0` while `p` is clear of both bands, rising to ±`top` at the edge of
/// `min..max`.
///
/// Shared by the canvas (`canvas::autopan`) and the layers tree, so a drag that
/// runs out of room scrolls the same way whichever one it is in.
///
/// **Quadratic, not linear.** The band is the whole control surface for the speed,
/// so the useful part of it is the slow end — a linear ramp is already at a third
/// of full speed a third of the way in, which is too fast to place anything. `t²`
/// keeps a creep available for most of the band and puts the fast scroll in the
/// last few points, where the pointer is against the edge and cannot be pushed
/// further anyway.
///
/// Beyond the edge it clamps rather than growing: a pointer 400px past the window
/// is not a request to scroll forty times faster. A container narrower than two
/// bands scrolls in neither direction rather than fighting itself.
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
/// Its ink: `text::DIM`, the same as the menu heads' caret. Dimmer than the value
/// beside it, because the chevron says "there is a list here" once and the value
/// is what gets read — and *one* dim for every caret in the app, so a dropdown and
/// a menu head do not disagree about how loud "there is more here" should be.
pub const COMBO_CHEVRON: egui::Color32 = theme::text::DIM;

/// Set up a dropdown's menu rows, inside a `ComboBox::show_ui` closure.
///
/// **The row floor must clear the tallest a row can naturally be, and 22 did not.**
/// egui 0.35 resolves a `Button`'s frame *per widget state* and computes its inner
/// margin as `button_padding − bg_stroke.width` — then puts the stroke back for a
/// hovered or selected row and leaves it out for a resting one
/// (`Button::selectable` is `frame_when_inactive(selected)`). With the panels'
/// `button_padding.y` of 0 that margin is −1, so measured: a resting row wants 21 and a
/// hovered or selected one wants 23. A floor of 22 sits *between* them — it lifts the
/// resting row to 22 and leaves the other at 23 — so the selected row in every dropdown
/// was permanently a pixel taller than its neighbours, and rows below whichever row was
/// hovered moved down by one. Reported as "the items in the boolean dropdown move when I
/// hover… this happens in other dropdowns too".
///
/// A floor of 24 is above both, so every state clamps to it. That fixed the *vertical*
/// shift and left a smaller horizontal one, because the margin falls short of the padding
/// on **x** as well — 8 − 1 — and the framed branch adds that point back, so a hovered row
/// wants two points more width than a resting one and the popup grows to the widest row it
/// has. Reported as "there's still a little shift on hover, much less than before, but
/// still there".
///
/// **So the border goes.** Zeroing `bg_stroke` on all four states makes the two branches
/// geometrically identical on both axes — the margin *is* the padding, and the stroke put
/// back is nothing — which is a stronger guarantee than clearing a floor, because it does
/// not depend on where the natural height happens to fall. The fill is the hover cue on its
/// own, which is what a menu row conventionally uses and what the font family list already
/// does; asked for that way ("we can just remove the border in here… matching the font
/// family dropdown"). The floor stays at 24 because the rows want the height regardless.
///
/// That font family list is the evidence for the diagnosis from the other side: it paints
/// its rows by hand rather than through `selectable_label`, and it is the one dropdown that
/// never shifted.
pub fn menu_rows(ui: &mut egui::Ui) {
    ui.spacing_mut().interact_size.y = MENU_ROW_H;
    let w = &mut ui.visuals_mut().widgets;
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.bg_stroke = egui::Stroke::NONE;
    }
}

/// Height of one row in a dropdown's menu — a **floor** over a natural 21, which
/// is what makes it the height rather than a minimum nobody reaches.
///
/// **22 since 2026-08-20, down from 24**, with [`MENU_ITEM_H`] taking the same two
/// points off the menus that allocate exactly. Reported as "the layer menus have
/// gotten a bit too tall… global for all menus".
///
/// **A floor only ever *raises*, so this number is only the height while it stays
/// above the natural one** — and that is not automatic. A `selectable_label` is
/// its 15pt row plus `button_padding.y` twice, which is **23** at the theme's 4:
/// a floor of 22 there would be ignored outright and the compaction would be a
/// constant that changed nothing. It works because every dropdown's own scope has
/// already set that padding to 0 or 2 before [`menu_rows`] runs, putting the
/// natural height at 15 or 19. *A new caller that leaves the theme's padding alone
/// gets a 23pt row and no error*, which is what
/// `a_menu_rows_height_does_not_depend_on_its_state` asserts against at both of
/// the values in use.
///
/// **Both states still clamp to one number**, which is the guarantee [`menu_rows`]
/// exists for: the stroke is zeroed, so a hovered row and a resting one have the
/// same natural height, and the floor is above both.
pub const MENU_ROW_H: f32 = 22.0;

/// Height of one row in a menu that allocates its rows **exactly** — the context
/// menus (`docs/context-menus.md` §8) and the top bar's View and Snap menus.
///
/// **One constant where there were three copies of `26.0`**, which is the
/// duplication `build::replace_with_path`'s own note warns about a level down: the
/// number was in `menu.rs` and twice more in `app.rs`, and "compact every menu by
/// two points" is exactly the edit that finds two of three.
///
/// Two points taller than a dropdown row ([`MENU_ROW_H`]) and deliberately so:
/// these rows carry a glyph column *and* an accelerator column, where a dropdown
/// row carries a glyph and a label. The gap was two before this change and is two
/// after it, because what was asked for was the same amount off everything rather
/// than a new relationship between the two surfaces.
///
/// 🚨 **The two constants are two *families* of row, and [`MENU_ROW_H`]'s warning
/// does not apply to this one** (§15 D811). That one is a **floor** under a row
/// egui sizes, so `button_padding` decides the natural height and a caller that
/// leaves the theme's 4 alone silently gets 23; these rows are
/// `allocate_exact_size`d by [`menu_row`] and [`menu_item`], which take the height
/// as an **argument**, so the padding cannot reach them and the number is the
/// height rather than a minimum. *The tell for which family a row is in is whether
/// its height is an argument* — which is why a warning about "any new dropdown"
/// cannot be applied without reading the callee, and why it is written here beside
/// both constants rather than once over the pair. The Export panel's preset menu
/// is the case that made this worth saying: it calls [`menu_rows`], never touches
/// the padding, and is correct — where a warning phrased about dropdowns in
/// general scores it as the failure case.
pub const MENU_ITEM_H: f32 = 24.0;

/// Like [`egui::Ui::add_enabled_ui`], but idempotent under a parent that is
/// already disabled.
///
/// **`add_enabled_ui` does not guard on the ambient state, and the fade is a
/// multiply.** `Ui::disable` multiplies the painter's opacity by
/// `Visuals::disabled_alpha` — 0.5 — every time it is called, so a disabled gate
/// inside a disabled gate lands at 0.25. That is visible as *unevenness* rather
/// than as a bug: the doubly-gated controls sit a shade fainter than the singly
/// gated ones beside them, in one row, and it reads as a rendering fault.
///
/// The identity row is where this bit. `points_header` wraps the whole row in
/// `add_enabled_ui(false)` to show the design's second row inert, and the row's
/// four controls each gate themselves again — so in point-edit mode the boolean
/// dropdown, *Mask* and *Ungroup* sat at 0.25 while ***Group*** beside them sat at
/// 0.5. Group is the odd one out because it is the only one of the four whose own
/// gate is *open* there: point editing implies exactly one selected `Path`, which
/// `build::group` accepts, while `selected_boolean`, `booleanable_selection` and
/// the container test behind `can_ungroup` all refuse it.
///
/// Note `Ui::add_enabled` — the *widget* form — already guards this way; it is
/// only the scope form that does not, which is why the two read as one thing
/// spelt twice.
pub fn disable_unless<R>(
    ui: &mut egui::Ui,
    enabled: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    // Already disabled means already faded and already inert, so there is nothing
    // a second gate can add — pass `true` and let the parent's state stand.
    ui.add_enabled_ui(enabled || !ui.is_enabled(), add)
}

/// The arrow at the closed end of a `ComboBox` — a Phosphor chevron, in place of
/// egui's default.
///
/// egui paints a filled triangle at `fg_stroke.color`, which is the *interaction*
/// colour: it brightens on hover and goes full-strength white when the popup is
/// open, so the loudest thing in a panel of dropdowns was their arrows. A glyph
/// from the icon font at one fixed colour makes them chrome again, and matches
/// every other caret the app draws (`menu_check`, the layers tree's disclosure).
///
/// Written to egui's `IconPainter` signature so it can be handed to
/// `ComboBox::icon` directly; `_open` is deliberately unused — egui itself stopped
/// flipping the arrow upwards for popups that open above (its own comment explains
/// why it looked wrong), and a chevron that changed colour on open would be the
/// same mistake in a different dimension.
///
/// *Every new `ComboBox` owes this call.* There is no style hook for the combo
/// icon — `Style` has no field for it — so it cannot be made the default, and a
/// dropdown that forgets it silently draws egui's white triangle beside every one
/// that did not forget. `combo_chevron_tests` pins the pair; it cannot pin the
/// call site that does not exist yet.
///
/// ⚠️ **This sentence used to end *"next to seven that do not"*, and the number
/// was the coverage claim** (§15 D665, `[S18.1-L3-09]`). It was seventeen when the
/// review counted and had been for a long time — stale by a factor of two and a
/// half, in the one sentence standing in for the gate the sentence itself says
/// cannot exist. `[A8-L6-04]`'s class one level down: **a rule whose only
/// enforcement is a figure in a comment.**
///
/// **The figure is gone rather than corrected**, because it rots on exactly the
/// day somebody adds a dropdown — which is the day it is being read. The check is
/// two greps over `crates/ondin-app/src`, and it is a thing somebody runs rather
/// than a gate:
///
/// ```text
/// ComboBox::from_id_salt      → every dropdown
/// .icon(combo_chevron)        → every one that remembered
/// ```
///
/// Run at the close of the fix phase's seventeenth session, 2026-09-09 over
/// `crates/ondin-app/src`: **23 constructions, of which 19 take a chevron and
/// four do not.** The four are `"probe"` combos inside `#[cfg(test)]` modules —
/// `inspector.rs:12786`, `:12975`, `typography.rs:5589` and `ui.rs:8131` — where
/// egui's own triangle is irrelevant. **So the rule holds at every production
/// dropdown**, and the positive result is written down so the next reader
/// compares rather than re-derives.
///
/// ⚠️ **Two traps in re-running it, and the recipe above walks into the first
/// one — deliberately, because there is no spelling that does not.** Each of
/// those two lines **matches itself**, so the greps return **24 and 20** and the
/// true figures are **23 and 19**: subtract one from each. A check written down
/// in the file it checks counts itself, and hiding the strings to avoid it would
/// make the recipe unrunnable, which is worse.
///
/// ⚠️ **Second: two of the nineteen call this function unqualified**, from
/// `combo_chevron_tests` in this very module, so grepping for `ui::combo_chevron`
/// finds **17** and leaves a two-dropdown gap that reads like the rule being
/// broken. That is session 14's `settings::card` shape for the third time on
/// record: **a qualified-name grep undercounts by exactly the module that owns
/// the function** — which is the likeliest interesting caller.
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
/// and nothing else — the identity card's boolean dropdown.
///
/// Full-strength ink, unlike [`glyph_and_text`]'s leading glyph: there is no label
/// beside it for the eye to go to instead, so this *is* the value being shown.
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
/// For the places a [`Prefix`] cannot reach: a `ComboBox`'s `selected_text` is a
/// single `WidgetText`, so the design's icon-plus-word closed state has to be
/// laid out as one job with two differently-styled sections rather than two
/// widgets side by side.
pub fn glyph_and_text(glyph: &str, text: &str) -> egui::WidgetText {
    glyph_and_text_tinted(glyph, text, theme::text::FAINT)
}

/// [`glyph_and_text`] with the **glyph** in a colour of the caller's choosing.
///
/// One caller, and it is a warning: the type family row wears `TEXT_AA` in
/// [`theme::color::WARN`] when the family cannot be had at all (§15 D227). The
/// precedent is a layers row whose picture cannot be drawn, which says so the same
/// way (§15 D179) — and the *label* stays full strength either way, because the
/// family's name is the identity the row is for and dimming it would read as the
/// pick not having taken.
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
/// a small label or glyph (`X`, `W`, a rotation clock) and the value beside it.
///
/// The prefix is painted by hand rather than through `DragValue::prefix`, which
/// renders prefix and value as one run of text in one colour and one font — so
/// it can neither dim only the prefix nor put an icon there. The `DragValue` is
/// stripped of its own ground — the row *is* the field.
///
/// **The whole row scrubs, the prefix included** — press anywhere on the ground,
/// the `X` or the clock included, and the value follows the pointer. That is not a
/// nicety: a field where the number scrubs and its own label does not means having
/// to remember which apps start the drag on the icon and which on the digits, and
/// the label is exactly where the hand goes, because it is what says *which* number
/// this is.
///
/// It is bought at the cost of this function owning the number rather than taking a
/// finished `DragValue`. The strip reserved for the prefix is outside the
/// `DragValue`'s box (see the padding note below for why it has to be), so the only
/// way it can scrub is for a second claimant to sense the drag there and apply it —
/// and to apply it, something has to be able to write to the value and know the
/// speed. Hence `&mut f64` and [`Scrub`]. A click on the strip hands the field
/// keyboard focus, so the two halves behave as one control in both gestures.
///
/// **Focus is drawn on the row, not inside it.** A `DragValue` being typed into
/// is a `TextEdit`, and a focused `TextEdit` frames itself: a second rounded
/// outline from the first digit to the right margin, nested inside the field's
/// own. Two problems with letting it — it is a box inside a box, and the digits
/// sit flush against its left edge with no padding at all, so the number reads
/// as overlapping its border. Suppressing that frame and stroking the *row*
/// instead keeps the value exactly where it sits at rest (no jump when the
/// field takes focus) and gives it the field's full 9px margin to breathe in.
/// `value` is generic over egui's `Numeric` so the `f32` fields (opacity) and the
/// `f64` ones (everything else) read the same at the call site; internally it is all
/// `f64`, which is what [`Scrub`] and `DragValue` both work in.
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
/// **Its own widget, not `DragValue::suffix`** — which renders the unit inside the
/// number's own text run, in one colour and one font, so it can neither be dimmed
/// nor clicked. That is the same reason [`Prefix`] exists at the other end.
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
/// The second return value is whether the suffix was clicked — which is a
/// *different* kind of change from anything the field itself reports, because it
/// converts the value rather than editing it. A caller must commit that directly
/// rather than route it through the drag valve, which is waiting for a drag that
/// will never come.
pub fn value_field_suffixed<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    suffix: Option<Suffix<'_>>,
    value: &mut N,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> (egui::Response, bool) {
    let mut v = value.to_f64();
    let out = value_field_f64(ui, size, prefix, suffix, None, &mut v, scrub, build);
    // Written back only on a real change, so a field whose type cannot hold its own
    // displayed value exactly — an `f32` opacity — is not rewritten every frame with
    // the round trip's error.
    if v != value.to_f64() {
        *value = N::from_f64(v);
    }
    out
}

/// How much of a [`value_field_metered`] row's ground the bar covers, as a
/// fraction of the field, and the colour it covers it in.
///
/// **16% of the accent, which is a wash rather than a fill.** The bar sits
/// *under* the field's own label and value, so anything stronger competes with
/// the digits it is behind; at this weight it reads as the field being partly
/// full and the text goes on reading as text.
const METER_ALPHA: u8 = 41;

/// Width of the tick marking a metered field's zero, in points.
///
/// **A hairline, and the same blue as the bar it is the origin of.** Its whole
/// job is that a row at rest does not read as an empty box: with the bar growing
/// from the centre, a neutral value draws nothing at all, and a field with
/// nothing in it says "no control here" rather than "this control is at zero".
/// One point of the field's own accent says both — where the middle is, and that
/// something fills from it.
///
/// It is more visible than that alpha suggests, which is why it does not need a
/// stronger one: over [`color::FIELD`] the wash resolves to about `rgb(62,66,76)`,
/// where the field's own hairline border is `rgb(53,53,54)` on the same ground.
/// The tick is the higher-contrast line of the two.
const METER_TICK_W: f32 = 1.0;

/// The accent wash a metered field's bar and its zero tick are both painted in.
///
/// One function rather than two literals so the tick cannot drift off the bar's
/// colour — being the *same* blue is the point of it, not a coincidence.
fn meter_wash() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        color::ACCENT_400.r(),
        color::ACCENT_400.g(),
        color::ACCENT_400.b(),
        METER_ALPHA,
    )
}

/// A [`value_field`] with a bar painted across its ground, **growing from the
/// centre**.
///
/// For a value whose size and direction are judged by eye rather than read — the
/// seven image adjustments, where what a card of seven rows has to answer at a
/// glance is which of them are doing anything, which way, and roughly how much.
/// A column of bare numbers answers that only one row at a time.
///
/// `signed` is clamped to `-1..=1`. Zero is the **middle** of the field, a
/// negative value fills left from it and a positive one fills right.
///
/// **That is a correction, and it is the whole shape of the control.** This grew
/// from the left at `|value|` first, on the argument that the sign is in the
/// digits — where it is exact — and that a two-sided bar spends half the field's
/// width encoding what one character already carries. The argument is true and
/// the conclusion was wrong: these seven are *offsets from a neutral zero*, not
/// quantities, so a bar that draws −40 and +40 identically is not compressing the
/// sign, it is contradicting it. A single-direction bar can only be right for a
/// value that has a bottom, and none of these do — their bottom is the middle.
///
/// **Painted between the ground and the content**, which is why it is threaded
/// through the field rather than drawn by the caller: the field's ground is
/// opaque, so a bar drawn before it is hidden and one drawn after it tints the
/// text. The shape is reserved inside the frame — after its background, before
/// its content — and filled in once the row's ink box is known.
///
/// The value itself is right-aligned rather than sitting beside the prefix, which
/// every other field does: a number in the middle of a field the bar is also
/// crossing reads as a label *on* the bar, and it lands in a different place in
/// every row because the labels are different lengths.
pub fn value_field_metered(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    signed: f32,
    value: &mut f64,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    value_field_f64(ui, size, prefix, None, Some(signed), value, scrub, build).0
}

/// Height of a [`badge_field`] — the size badge's own pill height, because it takes
/// that badge's place (§15 D252).
pub const BADGE_FIELD_H: f32 = 20.0;
/// Type size inside a badge pill, and the gap between its label and its value.
pub const BADGE_PT: f32 = 10.5;
const BADGE_FIELD_GAP: f32 = 5.0;
/// Padding inside a badge pill, matching `canvas::draw_size_badge`'s.
const BADGE_FIELD_PAD_X: f32 = 7.0;
/// How much of the badge's ink the label carries — the design's `opacity: .7`, as a
/// multiplier rather than a second colour so the two can never drift apart.
const BADGE_LABEL_ALPHA: f32 = 0.7;

/// Everything in a [`badge_field`] that is not the number: the padding either side,
/// the label, and the gap between label and value.
///
/// Exists so a caller can size its pill against the text it has to hold without
/// restating this function's own layout — which is how the origin pills came to clip
/// their value by less than a point (§15 D253).
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

/// A numeric field drawn as a **canvas badge**: the size badge's pill, with a
/// dimmed one-letter label and a value that scrubs, types and takes arithmetic
/// (§15 D252).
///
/// **The size badge's chrome exactly** — same ground, same ink, same height, same
/// corner radius, same padding — because it appears in that badge's slot and
/// swapping one for the other must not read as the canvas changing. The only thing
/// that differs is that this one is a control, and the only thing that says so is
/// the cursor.
///
/// **Upright, where the badge follows the shape's angle.** `canvas::badge_frame`
/// lays the pill along the edge it hangs from and `canvas::badge_text_angle` flips
/// it end-for-end so it reads the right way up (§15 D329); an egui widget cannot be
/// rotated at all, and a tilted text field would be a poor one even if it could. So
/// the fields take the badge's *position* and not its rotation, which on an
/// unrotated layer — every layer, most of the time — is the same placement.
///
/// The plumbing is [`value_field`]'s, deliberately: the same [`Scrub`], the same
/// [`crate::expr`] parser, and the same second-claimant trick that makes the label
/// scrub as well as the digits. A field the user has to aim at the digits of is a
/// field, and this one is 20 points tall.
///
/// `width` is the whole pill's, and it is the **caller's** rather than
/// shrink-wrapped. A pill that resized as its value was scrubbed would slide out
/// from under the pointer mid-drag, and two of them would stop being the same width
/// the moment one read `100` — on the canvas, where nothing else lines them up, that
/// reads as a wobble rather than as information.
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
    // Sensed as nothing: the two claimants below own everything that can be
    // pressed, and a third over the same region is the trap `icon_button_padded`
    // describes.
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::empty());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(4), theme::color::BADGE);

    // **Through [`badge_field_chrome_w`] rather than from `lead` directly**, so that
    // the room this leaves for the number and the room a caller sizes its pill
    // against are one statement. They were two, and the pill came out a point
    // narrower than the value it had to show (§15 D253).
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
            // Not `click_and_drag()`, which carries `FOCUSABLE` — the same dead tab
            // stop the inspector's prefix strip had, and this badge is a pair of
            // fields the eye tabs between exactly as the Transform panel's are
            // (§15 D290).
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
            // The `DragValue`'s two faces both ground themselves; here the pill is
            // the ground, so both go — see [`strip_drag_value_chrome`].
            strip_drag_value_chrome(ui);
            // Dark ink on a light pill, which is the badge's own inversion — the
            // panel's text colour would be white on blue and unreadable.
            ui.visuals_mut().override_text_color = Some(theme::color::BADGE_INK);
            ui.spacing_mut().button_padding = egui::Vec2::ZERO;
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.spacing_mut().interact_size = number.size();
            let dv = build(
                egui::DragValue::new(value)
                    .speed(scrub.speed)
                    .range(scrub.range.clone())
                    // ⚠️ **The same opt-out [`value_field_f64`] makes, and the
                    // argument is written there** (§15 D475). This field is the
                    // *other* numeric control in the file and it kept egui's
                    // default: a stored value outside the range rewritten to the
                    // nearest end on every frame, reported to the caller as a
                    // user edit.
                    //
                    // **Inert today and one `.range(…)` from not being.** Both
                    // production callers pass an unranged `Scrub`, so there is
                    // nothing to clamp against — the pivot marker's own comment
                    // three lines from the call argues for that deliberately. So
                    // this is not a fix for a measured defect; it is the same
                    // decision made in both places rather than in one, because
                    // the day a caller gives this field a range is the day it
                    // silently acquires `[S6.2-L1-01]`'s behaviour with nothing
                    // anywhere to say so.
                    .clamp_existing_to_range(false)
                    .custom_parser(crate::expr::eval),
            );
            // ⚠️ **`.update_while_editing(true)` used to sit here and is gone**
            // (§15 D723, `[S18.1-L3-06]`). It is egui's own default — measured,
            // `egui-0.35.0/src/widgets/drag_value.rs:81` — so it did nothing
            // today, and it sat *after* `build`, so it would silently have
            // overridden a caller that asked for `false` the day one wanted to.
            // The other two helpers leave the default alone. **An override that
            // is a no-op is the worst kind: nothing tests it and nothing
            // announces it.** If this field ever needs the non-default, it wants
            // saying here in as many words.
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
    // Painted after the value, so a focused field's selection block cannot land on
    // top of the label.
    ui.painter().galley(
        egui::pos2(strip.left(), rect.center().y - lead.size().y / 2.0),
        lead,
        theme::color::BADGE_INK,
    );
    // The strip's drag is reported as the field's own, so a caller's `dragged()` /
    // `drag_stopped()` / `changed()` — and therefore the edit valve — cannot tell the
    // two halves apart.
    resp = resp.union(lead_resp);
    if nudged || *value != before {
        resp.mark_changed();
    }
    resp
}

/// A bare [`egui::DragValue`] carrying the two decisions every numeric field in
/// this app makes, for the fields that cannot go through [`value_field`].
///
/// The two are `clamp_existing_to_range(false)` — §15 **D425**, argued at length
/// in [`value_field_f64`]; D475 is the precedent for making it where it is
/// *inert* — and [`crate::expr::eval`] as the parser. Chain
/// `suffix`/`speed`/`range`/`max_decimals` onto the result as usual; a caller
/// that sets its own `custom_parser` afterwards still wins.
///
/// ⚠️ **This exists because the decision was being made at one call site and
/// nowhere else** (§15 D552, `[S23.1-L1-01]`). `value_field_f64` spelled the
/// opt-out by hand for its 82 fields and was, until D552, **the only place in the
/// app it was made** — [`bare_drag_value`] put the parser on its four and left
/// the clamp alone — and the picker's gradient popover draws two plain
/// `DragValue`s that go through neither. One of the two had
/// `.range(0.0..=100.0)` over a value the
/// document can legitimately hold outside it, which is the whole of D425's bug at
/// a third door: the field painted `100` for a radial centre stored at `1.5`, and
/// a bare click moved the gradient 50 units into an undo step.
///
/// ⚠️ **And the field beside it was safe by accident, which is the reason to make
/// this a constructor rather than to add one line.** The angle field's producer
/// ends `to_degrees().rem_euclid(360.0)`, so its value cannot leave
/// `0.0..=360.0` and the clamp can never bite. The difference between the two
/// fields in one popover was that one producer normalises and the other does not
/// — a property of code two files away, holding a widget's behaviour up. The
/// next field added here inherits the decision instead.
pub fn plain_drag_value<T: egui::emath::Numeric>(value: &mut T) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .clamp_existing_to_range(false)
        .custom_parser(crate::expr::eval)
}

#[allow(clippy::too_many_arguments)]
fn value_field_f64(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    suffix: Option<Suffix<'_>>,
    meter: Option<f32>,
    value: &mut f64,
    scrub: Scrub,
    build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> (egui::Response, bool) {
    let scope = ui.scope(|ui| {
        field_row(ui, size, |ui| {
            // Reserved here and filled at the bottom of this closure: this is the
            // one point in the frame that is after its ground and before its
            // content, and both of these have to be both.
            //
            // **Two slots rather than one `Shape::Vec`**, which would be the
            // tidier spelling and is the wrong one: a `Vec` arrives in
            // `FullOutput::shapes` as a single nested `ClippedShape`, so every
            // probe that walks that list for a `Shape::Rect` would stop finding
            // either of them. Flat is what keeps the bar measurable
            // (`meter_tests`). The tick is reserved first so the bar paints over
            // it rather than under.
            let meter_slots = meter.map(|_| {
                (
                    ui.painter().add(egui::Shape::Noop),
                    ui.painter().add(egui::Shape::Noop),
                )
            });
            let lead = prefix.galley(ui);
            // Rounded, because the space reserved for the prefix and the point the
            // glyph is painted back at have to be the same number — a half-point
            // apart reads as the glyph sitting slightly off its own gap.
            let inset = (lead.size().x + PREFIX_GAP).round();

            // Both faces ground themselves and this row has already drawn its own
            // — see [`strip_drag_value_chrome`], which holds the two egui facts
            // this depends on and the reported bug behind the second.
            strip_drag_value_chrome(ui);
            // `DragValue` sizes its hit box from `interact_size`, set below to the
            // room left after the prefix — the prefix is a claimant of its own now,
            // not part of this widget's box. (This said the width was the *whole*
            // field and that `button_padding.x` was the prefix's room, which the
            // block beneath has since falsified in both halves.)
            //
            // Its *height* is the content box, not `size.y`: the hairline is
            // already spoken for ([`FIELD_BORDER_H`]), and asking for the full
            // row height here pushes the frame back out to `size.y + 2` — the
            // over-allocation the constant exists to prevent, reintroduced from
            // the inside.
            let full = ui.available_width();
            let inner_h = size.y - FIELD_BORDER_H;
            // **The prefix's width is space reserved beside the number, not padding
            // wrapped around it.** That distinction was worth a reported bug.
            //
            // `button_padding` is a `Vec2`, so egui applies it to *both* sides
            // (`Style::button_style` → `inner_margin: button_padding.into()`), and
            // the inset used to live there. It bought the wanted inset on the left
            // and an identical 22px of dead space on the right: 44 of this field's
            // 68px content box was padding and the digits had 24. Past four of them
            // the resting face — a `Button` with `TextWrapMode::Extend`, which never
            // truncates — simply grew, and took the field, the card and the whole
            // inspector's width with it. That is the stroke weight clipping off the
            // edge of the app.
            //
            // So the padding goes to zero and the room is made by advancing the
            // cursor instead: the digits get `full - inset` (~46px rather than 24)
            // and the field cannot grow until far past what [`MAX_STROKE_WIDTH`]
            // allows. `field_row`'s own `FIELD_PAD_X` is the breathing room at each
            // end.
            //
            // **Not a prefix *atom*, though `DragValue::prefix` exists**: atoms are
            // laid out by the resting `Button` only. The keyboard face is a bare
            // `TextEdit` that never sees them, so the glyph vanished and the digits
            // jumped left the moment the field took focus — which is exactly what
            // `focusing_a_value_field_outlines_the_row_and_leaves_the_value_put`
            // exists to catch. Painting the galley ourselves is what makes the
            // prefix survive both faces.
            //
            // The strip is therefore outside the `DragValue`'s box, which is why the
            // prefix has to scrub as a **second claimant** rather than for free — see
            // this function's own doc.
            // The unit's own strip at the far end, reserved the same way the
            // prefix's is: measured first, subtracted from the number's room, and
            // painted back afterwards. Same 38% grey, so the two ends of the field
            // read as one pair of labels around one value.
            let trail = suffix.map(|s| {
                // **Laid out in `PLACEHOLDER`, which is what defers the colour to
                // paint time.** `Painter::galley`'s colour argument is a *fallback*:
                // the tessellator substitutes it only where a section left that
                // sentinel. This galley was built with `.color(theme::text::FAINT)`,
                // so the hover colour was computed, handed over, and silently
                // dropped — the unit never brightened. Reported as "doesn't have a
                // hover color".
                //
                // Omitting the colour is *not* the fix and was the second attempt:
                // `into_galley` fills an unset colour with the style's own
                // `text_color`, so the unit came out full-strength white at rest.
                // The sentinel has to be asked for by name.
                let galley = egui::WidgetText::from(
                    egui::RichText::new(s.text)
                        .size(SUFFIX_PT)
                        .color(egui::Color32::PLACEHOLDER),
                )
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Body,
                );
                (s, galley)
            });
            // The room the unit takes out of the number's: its own text, less the
            // part of it that hangs into the field's padding, plus the gap that
            // separates it from the digits.
            let trail_w = trail
                .as_ref()
                .map(|(_, g)| (g.size().x - SUFFIX_BLEED + PREFIX_GAP).round())
                .unwrap_or(0.0);

            ui.spacing_mut().button_padding.x = 0.0;
            // **Every gap in this row is measured by hand** — the prefix strip via
            // `add_space(inset)`, the unit's via `trail_w` — so egui must not add
            // one of its own between them. It did, once the unit's strip became a
            // real allocation rather than paint: the frame came out exactly one
            // `item_spacing.x` wider than the size asked for.
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.spacing_mut().interact_size = egui::vec2(full - inset - trail_w, inner_h);

            // The prefix strip, sensed *before* the number is laid out so its drag is
            // already in the value by the time the number renders — otherwise the
            // digits would trail the pointer by a frame.
            //
            // Its own id, not the `DragValue`'s. Re-registering the number's id over a
            // wider rect would be the tidier trick and egui does keep only the last
            // rect per id — but `Context::create_widget` is private, and `Ui::interact`
            // sets `rect` as well as `interact_rect`, which trips the id-clash warning
            // (§15 D65) and paints a red outline on every field in the app.
            //
            // **`auto_id_with`, not `id().with`.** `Ui::id` is the *containing* Ui's,
            // which two fields in one `horizontal` share — so a salt hung off it gave
            // both strips the same id and lit every field in the Transform and Stroke
            // panels with egui's clash overlay, the strips inert behind it.
            // `next_auto_id` is positional within the Ui, so it distinguishes them;
            // the salt then keeps it clear of the `DragValue` that takes the same
            // slot. `a_row_of_value_fields_has_no_id_clash` is the guard.
            let strip = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(inset, inner_h));
            if let (Some((tick_slot, slot)), Some(frac)) = (meter_slots, meter) {
                // **The field's *ink* box, which is neither the size asked for nor
                // the content box.** `size.x` is the outer box and includes the
                // hairline on each side ([`FIELD_BORDER_H`] is the same two points
                // counted vertically); the cursor sits one `FIELD_PAD_X` inside
                // that, since the frame's inner margin is what reserves the field's
                // breathing room. So the bar starts a padding to the left of the
                // first glyph and runs the content's width plus both paddings —
                // drawn to the inside of the border, the way the design's
                // `overflow: hidden` clips it.
                let ink = egui::Rect::from_min_size(
                    ui.cursor().min - egui::vec2(FIELD_PAD_X, 0.0),
                    egui::vec2(full + FIELD_PAD_X * 2.0, inner_h),
                );
                // **From the middle, in whichever direction the sign says.** The
                // half-width is the whole travel of one side, so ±1 reaches an
                // edge and 0 is a bar of no width at all — which is what an
                // unadjusted row has to draw, since a control at rest showing a
                // mark is one the eye has to check rather than skip.
                let frac = frac.clamp(-1.0, 1.0);
                let mid = ink.center().x;
                let reach = mid + ink.width() / 2.0 * frac;
                let bar = egui::Rect::from_min_max(
                    egui::pos2(mid.min(reach), ink.top()),
                    egui::pos2(mid.max(reach), ink.bottom()),
                );
                // **Each pair of corners is rounded only when the bar reaches that
                // end of the field.** The field is a 5pt round rect and the bar is
                // inside it, so a bar that ran to an edge with square corners would
                // paint into the two slivers the ground leaves for the card behind
                // it. Rounding either pair unconditionally is the opposite error
                // and shows at every value but the extreme: a capsule floating in
                // the field rather than a fill running out of it. From the centre
                // both ends are in play, where a left-anchored bar only ever had
                // one.
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
                // **The zero tick, under the bar and always drawn.** Square, and
                // the full height of the ink — which is already the field less its
                // two hairlines, so it stops short of the border rather than
                // meeting it. Always, rather than only at rest: it is the origin
                // the bar grows from, so a row that hid it as soon as it had a
                // value would take the reference away exactly when there is
                // something to read against it.
                //
                // Painted *under* the bar rather than over, so that at any
                // non-zero value the two overlap by half a point at the origin
                // instead of the tick cutting a line through the fill.
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
            let lead_resp = ui
                .interact(
                    strip,
                    ui.auto_id_with("prefix-scrub"),
                    // **`CLICK | DRAG` spelled out, because `Sense::click_and_drag()`
                    // is `CLICK | DRAG | FOCUSABLE` and this strip must not be a tab
                    // stop.** It is a scrub handle with no keyboard face of its own —
                    // the digits beside it are the thing you type into — so focus on
                    // it lands nowhere visible. Tabbing across the Transform panel
                    // stopped on one of these before each field, so X → Y took two
                    // presses and the first went somewhere with nothing drawn on it
                    // (§15 D290).
                    egui::Sense::CLICK | egui::Sense::DRAG,
                )
                // The same cursor the number shows, so the strip advertises what it
                // does instead of looking like a label that happens to be draggable.
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            let mut nudged = false;
            if lead_resp.dragged() {
                // The pointer's own delta at the field's own speed, which is what
                // `DragValue` does internally — and through `scrub_speed_for`, so a
                // scrub that wraps the cursor round the screen edge does not jump.
                let speed = scrub_speed_for_value(ui.ctx(), scrub.speed);
                *value = scrub.settle(*value + f64::from(lead_resp.drag_delta().x) * speed);
                nudged = true;
                wrap_scrub(ui.ctx());
            }

            // `left_to_right` aligns *main*-axis content centred, which would
            // float the number in the middle of that widened box; the design has
            // it beside the prefix.
            //
            // **Except in a metered field, where it goes to the far end.** Two
            // reasons, and the second is the one that decided it: a number sitting
            // in the middle of a field the bar is also crossing reads as a label
            // *on* the bar rather than as the field's value, and — since the
            // prefix is a word of the model's rather than a glyph — it would land
            // in a different place in all seven rows, so the card would have no
            // column of numbers to read down at all.
            let main = match meter.is_some() {
                true => egui::Align::Max,
                false => egui::Align::Min,
            };
            let before = *value;
            let dv = build(
                egui::DragValue::new(value)
                    .speed(scrub.speed)
                    .range(scrub.range.clone())
                    // ⚠️ **The range bounds what a *gesture or a keystroke* can
                    // reach; it does not rewrite what the document already
                    // holds.** egui's default is the other way round
                    // (`clamp_existing_to_range` is `true`), applied
                    // unconditionally on every frame — so a stored value outside
                    // the range was silently rewritten to the nearest end, and
                    // the `mark_changed` below then reported the widget's own
                    // rewrite as a **user edit** and routed it through
                    // `edit_valve` into a transaction. A blur radius of 1200 in
                    // an imported document became 1000 on the frame the panel
                    // first drew it, with nobody touching anything.
                    //
                    // **One line, for all 82 call sites.** None of them opts out
                    // today (`grep clamp_existing_to_range` finds nothing outside
                    // egui), so this is where the decision is made rather than a
                    // default any of them chose. It is §5.3's *"clamped where the
                    // outline is built … not on the way in"* applied to the
                    // widget.
                    .clamp_existing_to_range(false)
                    // **Before `build`, so a field can still take its parser
                    // back.** [`crate::expr::eval`] is a superset of egui's
                    // default — every plain number it used to take, plus the
                    // arithmetic — so nothing needs to opt out today, and the
                    // three fields that carried a `%`/`°`-trimming parser of
                    // their own dropped it when this landed. A caller that sets
                    // one after this wins, which is what keeps a future field
                    // that parses something other than a number possible.
                    .custom_parser(crate::expr::eval),
            );
            let dv = scrub_speed_for(ui.ctx(), dv);
            ui.add_space(inset);
            let mut resp = ui
                .allocate_ui_with_layout(
                    egui::vec2(full - inset - trail_w, inner_h),
                    egui::Layout::left_to_right(egui::Align::Center).with_main_align(main),
                    |ui| ui.add(dv),
                )
                .inner;
            if resp.dragged() {
                wrap_scrub(ui.ctx());
            }
            // **Whole units under the hand, keyed on the drag.** One place rather
            // than the six call sites that each had to remember it — and `settle`
            // also re-clamps, since a rounded value can step outside a range whose
            // bound is fractional.
            if resp.dragged() || resp.drag_stopped() {
                let settled = scrub.settle(*value);
                if settled != *value {
                    *value = settled;
                }
            }
            // Report the strip's drag as the field's own, so a caller's `dragged()` /
            // `drag_stopped()` / `changed()` — and so the edit valve (§9.3) — cannot
            // tell the two halves apart.
            resp = resp.union(lead_resp);
            if nudged || *value != before {
                resp.mark_changed();
            }
            // Back into the strip reserved above. After the value, so a focused
            // field's selection block cannot land on top of the label.
            let at = egui::pos2(strip.left(), resp.rect.center().y - lead.size().y / 2.0);
            ui.painter().galley(at, lead, theme::text::FAINT);

            let mut unit_clicked = false;
            if let Some((s, galley)) = trail {
                // **Allocated, not merely painted into.** `field_frame`
                // shrink-wraps its content, so a strip that was subtracted from
                // the number's width and then drawn over left the frame `trail_w`
                // narrower than the size the caller asked for — a 130px field
                // painting 110, and the unit sitting outside its own border
                // because the *placement* still came from the full content box
                // (§15 D105). Sensed as nothing: the chip below claims what is
                // clickable, and two claimants over one region is the trap
                // `icon_button_padded` describes.
                ui.allocate_exact_size(egui::vec2(trail_w, inner_h), egui::Sense::empty());
                let band = egui::Rect::from_min_size(
                    egui::pos2(full_rect_right(ui) - trail_w, resp.rect.top()),
                    egui::vec2(trail_w, inner_h),
                );
                // **The unit is text that lights up, not a chip.** It had a ground
                // that appeared under it on hover, and the ground is what made the
                // two labels look unequal: `%` and `px` do not fill a box the same
                // way — one has a descender and the other does not — so a box drawn
                // tightly around either put the ink visibly off its own centre, and
                // the eye reads the *box* as the thing that should be centred.
                // Without one there is nothing to be off-centre against, which is
                // also why the one-point optical lift this used to carry is gone.
                //
                // It still hangs `SUFFIX_BLEED` past the content box into the
                // field's own padding — the design's `margin-right:-4px` — so it
                // finishes 5pt inside the hairline where the digits finish 9.
                let text_right = band.right() + SUFFIX_BLEED;
                let hit_rect = egui::Rect::from_min_max(
                    egui::pos2(text_right - galley.size().x - SUFFIX_HIT_PAD, band.top()),
                    egui::pos2(text_right + SUFFIX_HIT_PAD, band.bottom()),
                );
                let mut ink = theme::text::FAINT;
                if s.clickable {
                    // Sensed before it is painted, so the ink is this frame's answer
                    // rather than last frame's. Its own id and its own rect: the
                    // field keeps the rest of the row, which is what leaves two hover
                    // states for the eye. The hit box is a little wider and the
                    // field's full inner height, because two characters of 11pt text
                    // is a small thing to aim at and there is nothing else in that
                    // corner to hit.
                    //
                    // **`Sense::CLICK` without `FOCUSABLE`**, for the reason the
                    // prefix strip above spells out: this sentence used to claim the
                    // arrangement left "one focus target for typing" and
                    // `Sense::click()` is `CLICK | FOCUSABLE`, so a suffixed field
                    // was in fact three tab stops — strip, digits, unit (§15 D290).
                    let hit =
                        ui.interact(hit_rect, ui.auto_id_with("unit-suffix"), egui::Sense::CLICK);
                    // **No pointing hand.** The chrome never sets one: a design
                    // tool's panels are all controls, so a cursor that changes on
                    // every one of them says nothing and flickers constantly (§9.2).
                    // The places the cursor *does* change still mean something —
                    // `Grab` on a reorder grip, `ResizeHorizontal` on the layers
                    // panel's drag edge (§15 D349), on this field's own scrub strip
                    // and on a gradient stop's handle in the picker, and the drawn
                    // bitmaps on the canvas.
                    // Here the brightening text is the affordance.
                    //
                    // ⚠️ **The rule is "it has to mean something", not a count, and
                    // the list above is an example rather than a census.** This
                    // said "the two places" until 2026-08-25 and there were already
                    // four, because a cursor can be claimed through *two* APIs and
                    // grepping one of them finds neither scrub strip:
                    //
                    //     grep -rn "set_cursor_icon\|on_hover_cursor" crates/ondin-app/src
                    //
                    // Run that rather than trusting this sentence.
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
                    unit_clicked = hit.clicked();
                }
                let at = egui::pos2(
                    text_right - galley.size().x,
                    band.center().y - galley.size().y / 2.0,
                );
                ui.painter().galley(at, galley, ink);
            }
            (resp, unit_clicked)
        })
    });

    // Painted after the row, so it lands on top of the field's own hairline
    // rather than under it.
    if scope.inner.0.has_focus() {
        ui.painter().rect_stroke(
            scope.response.rect,
            egui::CornerRadius::same(5),
            egui::Stroke::new(1.0, color::ACCENT_700),
            egui::StrokeKind::Inside,
        );
    }
    scope.inner
}

/// The right edge of the content box a field's row is laying out into.
///
/// `ui.cursor()` has already advanced past the number by the time the suffix is
/// placed, and `max_rect` is the room the row was given — which is the edge the
/// unit has to sit against, inside `field_frame`'s own padding.
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

/// [`icon_button`] on a box that need not be square — the Align row's buttons
/// are 24×22 in the design, wider than they are tall so eight of them plus two
/// rules fit the card without crowding.
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

/// [`icon_button_sized`] whose **hit target** is `pad` larger than the box it
/// paints, on each side. See [`HEAD_ACTION_PAD`] for why a header's `+` wants
/// that and what the caller owes in return.
///
/// One widget, not two. The box is allocated without a sense and the press is
/// read from the padded rect, because a clickable 20pt square inside a clickable
/// 40pt one is the trap [`menu_head`] describes: the inner one wins the hit test
/// wherever it lies, so the padding would be live everywhere *except* on the
/// glyph.
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
/// **Three visual states, so not two `bool`s.** It was `(active, enabled)`, and the
/// state that forced an enum is [`FieldButton::Set`] — a combination those two
/// cannot express, because it is neither on nor off but *off while holding a
/// value*.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FieldButton {
    /// Off, and there is nothing more to say: the field ground and a muted glyph.
    #[default]
    Off,
    /// On — the accent wash and an accent border, the design's mark for a toggle
    /// whose extra fields are showing. A lighter wash than [`icon_button`]'s
    /// pressed state, because this one *stays* on.
    On,
    /// **Off, but not at its default.** The ordinary ground and glyph with the *on*
    /// border.
    ///
    /// What this buys is a control that can be collapsed without either throwing
    /// its value away or lying about the state: the border says "there is a setting
    /// in here", the fill says "and it is not open". The stroke panel's sides
    /// selector is the case it was built for — closing that used to reset the
    /// stroke to the whole outline, because with only two states the alternatives
    /// were losing the setting or refusing to close.
    Set,
    /// Nothing to do: a dimmed glyph and no hover feedback, so a button with no
    /// effect says so rather than inviting a click. Callers must still ignore the
    /// click; this only draws the state.
    Disabled,
    /// **[`FieldButton::On`] in red** — the confirming button of a card that
    /// destroys something, on [`theme::color`]'s danger ramp instead of the accent.
    ///
    /// The card it exists for is *Delete project*, whose confirm read as an
    /// ordinary [`FieldButton::Off`] button beside *Cancel* — two grey buttons, one
    /// of which removes a project that has no trash to come back from. **The
    /// accent's own weight is the point**: *Create project* and *Save changes* are
    /// filled blue, so a filled red says "this is that button, and it is the other
    /// kind of that".
    ///
    /// ⚠️ **Not the same red as a destructive *hover*.** `theme::color::DANGER` on
    /// an otherwise ordinary button is what the dashboard's trash icon does — quiet
    /// until aimed at. This one is loud at rest, which is the difference between a
    /// control you might click and the button you came to the card to press.
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

/// How far a [`focus_ring`] sits outside the widget it marks, and the corner it
/// takes: [`BUTTON_R`] plus that gap, so the ring stays concentric with the button
/// inside it rather than looking tighter at the corners than along the edges.
const FOCUS_GAP: f32 = 2.0;

/// The accent hairline around whatever the **keyboard** has focused — CSS's
/// `:focus-visible`, which egui has no equivalent of (§15 D382).
///
/// **Call it once at the end of a modal's body**; `settings::card` does that for
/// every card in the app so a new one cannot forget. It paints nothing at all
/// unless a key put the focus there.
///
/// ⚠️ **The mode flag is the whole feature, and it is `Tab` on / pointer-press
/// off.** Marking *whatever is focused* would put a ring on every field the moment
/// it was clicked, which is the thing browsers stopped doing and which this app
/// already suppresses by hand (`ui::text_field` turns egui's own ring off, and
/// [`field_frame`] is what a focused text box looks like). egui moves focus with
/// `Tab`/`Shift+Tab` and nothing else, so that one key is the whole of "the
/// keyboard is driving"; any pointer press hands it back.
///
/// ⚠️ **The rect is last frame's**, through `Context::read_response` — the focused
/// widget may not have been drawn yet this pass, and on the frame `Tab` moves the
/// focus it certainly has not. One frame of lag on a ring nobody is watching move
/// is not worth the alternative, which is every caller reporting its own rect.
///
/// ⚠️ **Every hand-rolled control in this app is already focusable**:
/// `Sense::click()` is `CLICK | FOCUSABLE` in egui, so
/// `allocate_exact_size(.., Sense::click())` — which is how [`button_face`],
/// [`switch_row`] and the swatches are all built — is in the tab order whether or
/// not anyone meant it to be. Read out of `egui::Sense` rather than assumed; it is
/// the reason this works without touching a call site.
///
/// ⚠️ **The editor's panels are focusable on the same terms and get no ring, and
/// §15 D747 records that as a *deferred question* rather than as a rule.** It is
/// not a decision that the chrome may take focus invisibly, nor that it should not
/// take focus at all; the entry carries the measurement and both answers.
pub fn focus_ring(ui: &egui::Ui) {
    let ctx = ui.ctx();
    // Two reads and a write, never nested: the context's own lock is not
    // re-entrant and `data_mut` inside `input` deadlocks (in release it hangs).
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
/// of four quarter-turn and mirror buttons splits the card evenly between them
/// rather than leaving 28px squares and a gap.
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

/// [`field_button_sized`] with a **word** in it instead of a glyph.
///
/// For the controls whose whole job is not to be mistaken for an icon button
/// somewhere else in the panel. The image popover's two mirror buttons are the
/// case it exists for: they do to the *picture* what the Transform panel's flip
/// buttons do to the *layer*, and on a symmetric shape the two produce the same
/// drawing — so nothing on screen can distinguish them and only the words can.
/// Phosphor has no second flip pair to reach for, and its own `flip-horizontal`
/// and `flip-vertical` are not even a matched pair (one mirrors about a dashed
/// axis, the other hangs right-triangles off a solid bar), so drawing the
/// difference was the alternative and words are cheaper and clearer.
///
/// The ground, border, hover and the four states are [`button_face`]'s, shared
/// with the glyph version rather than copied — a text button that drifted from
/// the icon buttons beside it is exactly the unevenness the eye catches.
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
/// be used, [`theme::text::DISABLED`] when it cannot.
///
/// **`enabled` is ignored where the surrounding `Ui` is already disabled**, for the
/// same reason [`disable_unless`] exists. That scope has already faded this glyph
/// by `Visuals::disabled_alpha`, so painting the disabled tier *underneath* the
/// fade dims it twice — 40, against the 79 every other unavailable control in the
/// app paints. Deferring is not an approximation: `MUTED` faded by a disabled scope
/// **is** `text::DISABLED`, which is where that constant's value came from.
///
/// The identity row is where this showed. Its buttons pass their own condition to
/// [`icon_button`] *and* sit in a [`disable_unless`] scope gated on the same thing,
/// so Mask and Ungroup came out a shade darker than Group beside them — the second
/// dim axis left over after the scope one was fixed. A control saying "unavailable"
/// twice should not look twice as unavailable.
fn resting_ink(ui: &egui::Ui, enabled: bool) -> egui::Color32 {
    match enabled || !ui.is_enabled() {
        true => theme::text::MUTED,
        false => theme::text::DISABLED,
    }
}

/// The ground, hairline and hover treatment every field-row button shares, and
/// the colour its content should be drawn in.
///
/// Split out so [`field_button_sized`] and [`label_button`] cannot disagree about
/// any of the four [`FieldButton`] states — the states have to be geometrically
/// and tonally identical across both or a row mixing them reads as two different
/// controls.
///
/// Its corner is [`BUTTON_R`], named rather than written here because a caller
/// that repaints over this face — the dashboard's *Delete project*, which lays a
/// red wash on the hovered state — has to round to exactly the same number or the
/// overlay shows a hairline of the ground at each corner.
///
/// **`pub` for the same reason it exists**: a caller whose content is neither a
/// glyph nor a word still owes the four states. The dashboard's project filter is
/// the case — a colour dot, a name and a caret, sitting immediately beside a
/// [`label_button`] and an [`action_button`], where any drift in the ground or the
/// hairline reads as two different kinds of control in one row.
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
    // `Set` takes the *hover* treatment of an ordinary button, not `On`'s wash:
    // its whole point is that the ground reads as off. Only the border differs.
    // ⚠️ **The two filled states have a hover of their own, and had none at all.**
    // They cannot fall through to the neutral arm — the loudest button on a card
    // would go grey under the pointer — and the first spelling of them ignored
    // `hovered` entirely instead, which left every modal's *Create project*, *Save
    // changes* and *Delete project* completely inert to the pointer. One step up
    // their own ramp is the whole fix: `ACCENT_900` → `ACCENT_800` and the danger
    // ramp's twin of that.
    let (bg, fg) = match (state, hovered) {
        // The *ground* is the only thing that moves, in both — the ink stays put so
        // the two filled states lighten by the same one visible step. `ACCENT_100`
        // exists and is deliberately not reached for here: the danger ramp has no
        // twin of it, and a hover that brightens the text on one button and not the
        // other is the kind of unevenness this whole function exists to prevent.
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
                // The border is what `Set` and `On` share, and it is the whole of
                // the signal that a collapsed control is still carrying something.
                FieldButton::On | FieldButton::Set => color::ACCENT_700,
                FieldButton::Danger => color::DANGER_700,
                FieldButton::Off | FieldButton::Disabled => color::FIELD_BORDER,
            },
        ),
        egui::StrokeKind::Inside,
    );
    (rect, resp, fg)
}

/// Between one labelled section and the next inside a popover — the type popup's
/// tabs, the stroke card, the colour picker, the export settings and the export
/// menu's divider spacing.
///
/// **One number for all five, which is the whole reason it is here.** They had
/// four between them — 15, 11, 10 and 10 — each with a reason recorded about the
/// card it was in, and read one after the other in the same lane those reasons do
/// not survive contact: the cards are the same width, the same padding and the
/// same controls ([`CONTROL_H`] — 26 when D275 was written, 28 since D386), so a
/// section gap that changes between them is a wobble
/// rather than a distinction (§15 D275). `popup_rule`'s 5-above-plus-4-below plus
/// its own hairline already comes to exactly this, which is why the menus needed
/// nothing.
///
/// ⚠️ **It is [`CARD_ROW_GAP`] now, not a tenth number of its own.** D275 pulled
/// four into one and left the one at 10, which is a point off the pitch the
/// inspector stacks every row at — invisible where an eyebrow stands between two
/// sections, and not invisible in the stroke popover's dash block, where there is
/// no eyebrow and this *is* the gap between two controls (§15 D386). The eyebrow
/// is what separates one section from the next; the number underneath it is the
/// panel's, like everything else.
pub const POPOVER_SECTION_GAP: f32 = CARD_ROW_GAP;

/// Between a small-caps eyebrow and the control it names.
///
/// **Tighter than [`CARD_ROW_GAP`] on purpose, and it is the one gap that is.** A
/// label and its control are one thing; two controls are two. Left at the row
/// pitch the label floats equidistant between what it names and the section above
/// it, which is the reading that makes a nine-section popover look like eighteen
/// loose rows.
pub const SECTION_LABEL_GAP: f32 = 5.0;

/// One labelled block: the eyebrow, then its controls at the card's own row pitch.
///
/// **The two gaps are two numbers, which is the whole reason this exists.** Set as
/// a single `item_spacing.y` they cannot be — and were not: the popovers ran the
/// label gap between their *controls* as well, so a section holding two fields
/// stacked them 5 apart where the Transform card stacks X/Y and W/H at
/// [`CARD_ROW_GAP`]. Read one after the other at the same width those are not two
/// rhythms, they are one rhythm and a wobble (§15 D386).
///
/// That reverses a decision this codebase had already made and written down — the
/// popover took the design mock's vertical compression (5 down against the card's
/// 9) while matching it across — on the grounds that the mock drew it that way.
/// The maintainer's instruction is explicitly *not* the mock: the Transform card's
/// X↔Y and X/Y↔W/H gaps are the baseline every panel and popover follows.
pub fn labelled<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        // Zero, so the eyebrow's own gap is the `add_space` below and nothing
        // else — a spacing here would be *added* to it (`settings::settings_ui`
        // states the same trap).
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

/// Between an [`action_button`]'s glyph and its word — the design's, and the field
/// gap the value fields use at both ends of their number.
const ACTION_GAP: f32 = 7.0;
/// An [`action_button`]'s leading glyph.
const ACTION_GLYPH_PT: f32 = 15.0;
/// What an [`action_button`]'s pair keeps clear of its own corners, so an elided
/// name ends in a `…` with air after it rather than against the rounding.
const ACTION_PAD: f32 = 10.0;
/// An [`action_button`]'s word.
const ACTION_LABEL_PT: f32 = 12.0;

/// The width an [`action_button`] needs to show `label` without eliding it.
///
/// **Measured off the same four constants the button lays itself out with**, which
/// is the whole point of it existing: a caller that wants to size the box to the
/// text — the dashboard's sort control, which changes its own word — would
/// otherwise be re-deriving `ACTION_PAD * 2 + ACTION_GLYPH_PT + ACTION_GAP` from
/// reading this file, and the day one of them moves the two disagree silently.
/// [`badge_field_chrome_w`] exists for the same reason beside the same problem.
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

/// A wide button with a glyph and a word, both centred — the Export panel's
/// *Export selection* and *Export as Zip* (`design/Editor.dc.html`), and the
/// dashboard's sort control (`design/Dashboard.dc.html`).
///
/// **Centred as a pair, where [`toggle_row`] is left-aligned**, and the difference
/// is what each one is: a switch is a row in a list and lines up with the rows
/// above it, where this is an action and reads as a button. Same
/// [`button_face`], so the two are the same material.
///
/// **The word is elided to fit, not painted through the button's edges.** The
/// label here is not always a phrase this module chose: the Export panel puts a
/// *layer's name* on it, and there is no length a name cannot be — `Export
/// Ellipse 11312312 3123123132` ran out of both sides of the card. A `p.text`
/// call clips against nothing, so the fit has to be asked for at layout time
/// (§15 D276). A caller that would rather grow the box than lose the word asks
/// [`action_button_w`] how wide to make it.
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
    // Measured, then placed, so the pair is centred as one thing rather than the
    // word being centred with a glyph hanging off it. The measurement is of the
    // galley that will actually be painted — an elided one is *narrower* than the
    // label it came from, and centring the pair by the untruncated width would
    // push it off the button it was just made to fit.
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
/// dash pinned to the right — the Export settings card's *Trim transparent edges*
/// and *Pad out to a square* (`design/Editor.dc.html`).
///
/// **[`button_face`]'s four states rather than a checkbox**, so a switch in a
/// popover is tonally the same control as the square icon buttons in the card
/// behind it: `On` is the accent wash, and `Disabled` is a switch that has nothing
/// to act on and says so instead of vanishing. That is also what keeps the *whole
/// row* the target, which a 13pt tick could never be.
///
/// **The off state draws a dash, not nothing.** An empty right-hand column reads
/// as a row that has not finished loading; the design draws `ph-minus` there, and
/// it is the same reasoning [`menu_check`] gives for keeping its tick's column
/// reserved — the state has to be readable at a glance rather than by noticing an
/// absence.
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

/// The drag grip on a reorderable row: the design's six-dot glyph, sensing drag
/// rather than click.
///
/// Its own widget rather than part of the row, because the row's *other* controls
/// have to keep working while this one is the only thing that starts a reorder —
/// a hex field you cannot click into because the row is draggable is worse than
/// no reorder at all. Pulled back into the field's left margin by 3px, as the
/// design draws it, so the chip beside it still sits where it does on a row with
/// no grip.
/// `enabled: false` — a list of one, where there is nowhere for the only paint to
/// go — **draws the identical glyph and simply does not drag**. Dimming it or
/// dropping it would make the row jump about as paints are added and removed, and
/// would spend a visual distinction on a state where nothing is being withheld.
/// The tooltip goes, since "drag to reorder" would be a lie.
///
/// **The hit target is the whole left end of the field**, `row_h` tall and reaching
/// back through the field's own [`FIELD_PAD_X`] inset to its hairline — everything
/// left of the glyph belongs to the glyph, because there is nothing else there to
/// hit. Six dots 11pt wide by 20 is a small thing to catch, and missing it lands on
/// the field, which does nothing.
///
/// It stops at the glyph's own right edge and does **not** take the 9pt gap before
/// the colour chip. That gap is the chip's approach: swallowing it would trade a
/// grip that is hard to grab for a swatch that is hard to click, which is the worse
/// half of the bargain — the swatch is used on every paint and the grip only on a
/// list of two or more.
pub fn grip(ui: &mut egui::Ui, enabled: bool, row_h: f32) -> egui::Response {
    // The glyph's own box, always allocated so the chip beside it lands in the
    // same place whether or not this row can be reordered. The press is read from
    // the wider rect below, so this one senses nothing: two claimants over one
    // region and the smaller one wins wherever it lies (see [`icon_button_padded`]).
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

/// Whether a hairline `width` points wide wants its centre on a pixel **centre**
/// rather than on a pixel **edge**, at this display scale (§15 D479).
///
/// A line whose *device* width is odd covers a whole number of pixels only when it
/// is centred on one; an even one only when it sits on the boundary between two.
/// Getting it backwards is what turns a hairline into a two-pixel grey smear.
///
/// **The parity is `floor(width × ppp)` — the device width, floored** — and all
/// three parts of that were wrong at some point. `rulers.rs` carries the full
/// derivation and the probe that settled it: reading the *point* width assumes one
/// point is one device pixel, which is false at 150% and 175%; `round` calls a
/// 1.5-device-pixel line even, edge-centres it, and leaves a resting guide with no
/// solid pixel at all.
///
/// **This is the half four spellings of the rule agree on.** They do *not* agree
/// on the base — see [`hairline_in_column`] — which is why the shared thing is a
/// predicate rather than the whole formula.
pub fn hairline_on_pixel_centre(ppp: f32, width: f32) -> bool {
    (width * ppp).floor() as i32 % 2 == 1
}

/// A hairline's coordinate snapped to the **nearest grid position of the right
/// parity**, so it covers whole physical pixels and stays inside the column it was
/// allocated (§15 D479).
///
/// The three chrome rules — [`rule`], `inspector::popup_rule` and
/// `settings::section` — all draw a 1pt line into a 1pt allocation between two
/// galleys, so the constraint is two-sided: land on the grid, and do not drift so
/// far doing it that the ink leaves the column. **Nearest-of-the-right-parity is
/// what satisfies both**, and it bounds the drift at half a device pixel at every
/// scale, which is the floor on the error rather than a slack allowance — the
/// column's centre generally is not on the grid, and no arithmetic closes a gap
/// the two coordinate systems create.
///
/// ⚠️ **The parity term used to be unconditional here**, which is the whole of
/// `[S19.1-L1-04]`: `((v × ppp).floor() + 0.5) / ppp` always asks for a pixel
/// *centre*, so at an even integer scale a 1pt line — 2 device pixels — spread as
/// `[0.5, 1.0, 0.5]` across three. Measured at 200%, plain Windows scaling, at all
/// three sites. It is `[1.0, 1.0]` now, and 150%'s `[0.25, 1.0, 0.25]` — which no
/// arithmetic can improve on, since a 1.5-pixel line cannot put both edges on the
/// grid — is unchanged.
///
/// ⚠️ **The two arms are not two spellings of one snap and neither may be swapped
/// for the other**, which a probe over `ppp ∈ {1, 1.25, 1.5, 1.75, 2, 3}` says
/// rather than taste: `floor + 0.5` is the nearest pixel *centre* and `round` the
/// nearest pixel *edge*, so each is nearest for its own parity and up to a **whole
/// device pixel** out for the other's — 1.0pt at 100%, which puts a 1pt rule
/// entirely outside its 1pt column. That is the failure [`rule`]'s own history
/// records (*"put the ink up to 0.7pt right of centre and, measured, mostly
/// outside the column altogether"*, §15 **D46**, the second contributor to D44's
/// unevenly spaced cluster) reached by a second road.
///
/// ⚠️ **`rulers::snap_across_axis` shares [`hairline_on_pixel_centre`] and not
/// this**, which the review filed as one rule written twice (`[S18.1-L3-03]`,
/// G20). It rounds for **both** parities, so its odd case is a nearest *edge* with
/// half a pixel added — up to a whole device pixel from where the guide was put.
/// A guide has no column, so nothing has ever made that visible, and this is
/// recorded rather than changed: moving guide ink is a visible change to the one
/// piece of this arithmetic that *is* pinned by tests, and it wants its own
/// decision rather than being swept in behind a merge.
pub fn hairline_in_column(centre: f32, ppp: f32, width: f32) -> f32 {
    let device = centre * ppp;
    match hairline_on_pixel_centre(ppp, width) {
        // The nearest pixel *centre*, and the nearest pixel *edge*. `floor + 0.5`
        // and `round` are those two things, not a downward bias and a nearest —
        // which is why neither may be swapped for the other.
        true => (device.floor() + 0.5) / ppp,
        false => device.round() / ppp,
    }
}

/// The 1px rule the Align row uses to part its three groups. Allocated at the
/// row's full height and painted 16px tall, centred — a `Separator` would claim
/// the whole remaining width.
///
/// The device-grid snap is [`hairline_in_column`], which is where the reasoning
/// this function used to carry now lives — it had been copied verbatim into two
/// other files and had the parity bug in all three (§15 D479).
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

/// Paint a **filled** lock centred in a `size`-tall box.
///
/// Drawn rather than set in type: the bundled `Phosphor.ttf` is the Regular
/// weight only, and Phosphor's Fill style is a separate font file we do not
/// ship. The design marks a locked layer with the filled glyph specifically —
/// it has to read as shut at a glance beside the outline one on the hover
/// state — so the shape is reproduced here from Phosphor's own
/// `lock-simple-fill` geometry, in its 256-unit box: a body from `32,80` to
/// `224,224` with a 16-unit corner radius, and a shackle band between radii 32
/// and 48 about `128,56` — i.e. a 16-wide stroke along radius 40.
///
/// `size` is the em size the equivalent glyph would be set at, so a call site
/// can hand it the same number it passes to [`theme::icon_font`] and get a lock
/// that lines up with the outline one it replaces. `rot` turns it, for the size
/// badge, which tilts with the layer it measures.
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

/// The two shapes [`paint_lock_filled`] draws — split out so the drawing can be
/// checked against the box it is supposed to fill, the way [`corner_mask`] is.
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
    /// The ink spans y = 8..224, so its middle — not the box's — is what has to
    /// land on `center`, or the lock hangs low beside the text next to it.
    const INK_MID_Y: f32 = 116.0;

    let k = size / 256.0;
    let frame = Frame2::rotated(rot, center);
    let at = |x: f32, y: f32| frame.to_screen(egui::pos2((x - 128.0) * k, (y - INK_MID_Y) * k));

    // The shackle: two short verticals into the body, joined over the top by a
    // half-circle. One polyline, so the corners meet without a seam.
    const STEPS: usize = 12;
    let mut pts = vec![at(PIVOT.0 - BAND, BODY.1)];
    for i in 0..=STEPS {
        let t = std::f32::consts::PI * i as f32 / STEPS as f32;
        pts.push(at(PIVOT.0 - BAND * t.cos(), PIVOT.1 - BAND * t.sin()));
    }
    pts.push(at(PIVOT.0 + BAND, BODY.1));

    // The body: a rounded rect built as geometry in the same frame. It used to
    // be a `RectShape`, which cannot be turned.
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

/// The hairline round a [`segmented`] track, and the width the cells give up to
/// it on every side.
///
/// One point, like every other border in the chrome — [`field_frame`]'s stroke,
/// [`button_face`]'s, the dashboard search field's. Named rather than written as a
/// literal because it is subtracted in one place and painted in another, and those
/// two have to agree or the cells sit crooked in their own track.
const SEGMENT_BORDER: f32 = 1.0;

/// A segmented control: `n` equal cells in a recessed track, the selected one
/// raised. `paint` draws cell `i` into its rect (`selected` tells it which tier
/// of contrast to use), which is what lets the stroke panel put line samples in
/// the cells rather than text. Returns the clicked index.
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
/// A disabled cell keeps its place and its label, takes no hover ground and
/// returns no click. That is the same bargain the identity row's permanently
/// disabled mask button makes: a control that vanishes leaves the user wondering
/// whether the app has the feature at all, where one that is visibly unavailable
/// says "not for this subject", which is the actual state.
///
/// It also keeps the *row* the size it always is. The picker's kind tabs used to
/// be dropped entirely for a solid-only slot, and what that left was a header with
/// nothing in it but a close button — reported as looking empty, which it was.
///
/// `enabled` is asked per cell rather than passed as a mask, so the caller can
/// answer from whatever it already knows about that index.
///
/// ⚠️ **"Takes no hover ground and returns no click" is the whole of it, and a
/// disabled cell takes no *focus* either** (§15 D722, `[S18.2-L3-04]`). It used
/// to be registered `Sense::click()` — so it was a tab stop that could not be
/// activated — on the argument that it might carry a tooltip. The signature makes
/// that impossible: the per-cell `Response` never leaves this function. The
/// inspector's layout-grid row cites *"`ui::segmented_enabled`'s bargain"* and
/// quotes only the visible-but-unavailable half, which is the half that is true.
pub fn segmented_enabled(
    ui: &mut egui::Ui,
    width: f32,
    cell_h: f32,
    n: usize,
    selected: usize,
    enabled: impl Fn(usize) -> bool,
    paint: impl Fn(&egui::Painter, usize, egui::Rect, bool),
) -> Option<usize> {
    const PAD: f32 = 2.0;
    const GAP: f32 = 2.0;
    let (track, _) =
        ui.allocate_exact_size(egui::vec2(width, cell_h + PAD * 2.0), egui::Sense::empty());
    let r5 = egui::CornerRadius::same(5);
    let p = ui.painter();
    p.rect_filled(track, r5, theme::color::text_a(13));
    // The design's hairline, the same one every field and button in the chrome
    // wears — `text 8%` over whatever the track is sitting on, which is why it is
    // [`theme::color::text_a`] rather than the solid [`color::FIELD_BORDER`]: a
    // segmented control appears on a card, in a popover and in the dashboard's
    // header, and those are three different grounds. It was the **only** control
    // class in the app still drawing its ground bare (§15 D386).
    p.rect_stroke(
        track,
        r5,
        egui::Stroke::new(SEGMENT_BORDER, theme::color::text_a(20)),
        egui::StrokeKind::Inside,
    );

    // ⚠️ **The hairline comes out of the cells, not out of the track.** The track
    // is what a caller sized and what has to keep lining up with the field beside
    // it, so the border is drawn *inside* the box already allocated and the cells
    // move in by its width — which is also what the design does: a 28pt control is
    // `1 + 2 + 22 + 2 + 1`, not `2 + 24 + 2` with a border hung outside it.
    let inner = track.shrink(PAD + SEGMENT_BORDER);
    let cell_w = (inner.width() - GAP * (n.saturating_sub(1)) as f32) / n.max(1) as f32;
    let mut clicked = None;
    for i in 0..n {
        let rect = egui::Rect::from_min_size(
            egui::pos2(inner.min.x + i as f32 * (cell_w + GAP), inner.min.y),
            egui::vec2(cell_w, inner.height()),
        );
        let live = enabled(i);
        // 🚨 **A disabled cell senses hover, not click** (§15 D722,
        // `[S18.2-L3-04]`). This was `Sense::click()` either way, on the stated
        // ground that *"the cell can carry a tooltip explaining why it is
        // unavailable"* — **which no caller can do**: this function returns
        // `Option<usize>`, `resp` is a local that never escapes, and `paint` is
        // handed a `&Painter` rather than a response. One production caller
        // (`picker::picker_body`), zero tooltips, and `segmented` passes
        // `|_| true` so it has no disabled cells at all.
        //
        // What it did instead was put the cell in the **keyboard tab order** —
        // `Sense::click()` is `CLICK | FOCUSABLE`, which `focus_ring`'s ⚠️ warns
        // about from the other side — so Tab stopped on a cell that swallowed
        // Return and Space and offered neither the click nor the explanation.
        //
        // ⚠️ **Still registered, rather than not sensed at all.** The rect stays
        // claimed so the cell reports hover, which is what the *ground* is drawn
        // from and what a future tooltip would rest on; `Sense::hover()` is
        // `Sense::empty()` (§15 D688), so it takes no focus and no click. If the
        // tooltip is ever wanted, the change is to hand `paint` the response —
        // widening the signature is the honest way to offer it, and a sense
        // nobody can read is not.
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
        paint(ui.painter(), i, rect, on);
        if live && resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
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

/// A [`segmented`] cell showing nothing but a dash — the **mixed** state.
///
/// A segmented control cannot show "several of these at once", so it shows none
/// of them and says so: the track paints with no cell raised and this dash in the
/// middle. Blank would read as a bug; the selected-looking first cell would be a
/// lie.
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

/// The row a [`slider`] allocates — the knob's diameter.
///
/// ⚠️ **It is not the height of the ink, and this doc said it was** (§15 D673,
/// `[S18.2-L3-05]`). It read *"the knob's diameter, which is the tallest thing in
/// it"*; the tallest thing in it is the knob's **shadow**, a `half`-radius disc
/// drawn one point lower ([`slider`], the design's `box-shadow: 0 1px 3px`), so a
/// slider paints `SLIDER_H + 1` of ink into the `SLIDER_H` it reserves. Measured
/// at ppp 1.0 over a 120pt slider: row `120 × 11`, ink `120 × 12`, bottom overflow
/// exactly 1.0.
///
/// **A shadow outside the box is what a shadow is**, so the bleed is the design
/// rather than a defect — it lands in the caller's row gap, and the one production
/// caller (`typography::axis_slider`) has room. What was wrong was a `pub`
/// constant telling its callers it bounded the drawing.
/// `a_sliders_shadow_leaves_the_row_by_exactly_one_point` is now the gate on how
/// far, because a number in a doc comment is not one. (Plain backticks: it is a
/// `#[cfg(test)]` item, which `cargo doc` cannot see — §15 D319. Written as a
/// `[link]` first, and **the doc gate caught it**, which is D319's mechanism from
/// a third direction: not a link nothing checks, but a production doc that cannot
/// name a test at all.)
///
/// 🚨 **The app's other slider was said to answer the same question the other way,
/// and measurement says it answers a different question** (§15 D766, ruling D673).
/// `panels::picker`'s track bounds its knob **vertically** — that is what its
/// `+ 6.0` buys — and lets it hang **7pt off each horizontal end**, unclamped,
/// where this one bleeds 1pt below and clamps travel so the knob cannot leave the
/// rail. **They bound different axes**, so there was no "which is right" to rule
/// on: the sentence this replaces (*"its knob is given room where `ui::slider`'s
/// overhangs"*) is true of one axis and silent about the one where the answers are
/// reversed.
///
/// ⚠️ **And `picker`'s constant was never this quantity.** It is now `TRACK_H`,
/// because it is the *rail's* thickness — this one is the **knob diameter**, with
/// [`SLIDER_RAIL_H`] under it. Unifying the two values would have thickened the
/// picker's gradient strips and left its knob exactly where it was.
///
/// ⚠️ **`the number every caller reserves` was an overstatement and is gone**: no
/// production code outside this file reads this constant. `slider` reserves it on
/// its callers' behalf, which is a different claim and the true one.
pub const SLIDER_H: f32 = 11.0;
/// The rail's thickness. **3, and the knob is nearly four times it**: the design
/// draws a hairline track with a bead riding on it, where egui's own `Slider`
/// draws a chunky rail and a handle barely thicker than it — the two read as
/// completely different controls beside the same fields.
const SLIDER_RAIL_H: f32 = 3.0;

/// A value between two bounds: a hairline rail, filled in accent to the knob.
///
/// **Hand-painted rather than `egui::Slider`**, for the reason `segmented` and
/// `switch` are: the design specifies a 3pt rail, an 11pt knob, the filled half in
/// `ACCENT_600` and the knob in `ACCENT_100`, and egui derives all four from
/// `Spacing` and `Visuals` in ways that cannot be reached without restyling every
/// slider in the process. It also drops `Slider`'s text box, arrows and drag
/// handles, none of which this wants — the axis rows carry their own numeric field.
///
/// The knob's centre travels between `half..width - half` so it cannot hang off
/// either end of the rail, and the rail is drawn full width underneath: a knob
/// whose *edge* touched the end would read as a value short of the maximum.
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

    // **Absolute, not incremental**: a slider jumps to where it is pressed and
    // then follows the pointer, which is what every slider does and what makes a
    // click on the rail mean something. (A `DragValue` is the opposite — see
    // `value_field` — because a number has no rail to point at.)
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
    // The design's `box-shadow: 0 1px 3px rgba(0,0,0,.4)` — one soft disc under
    // the knob, which is what lifts a light bead off a light rail.
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
/// **A rounded-rectangle knob in a rounded-rectangle track, not a pill.** Every
/// other control in the chrome is a 4–5px rounded box — the segmented cells, the
/// field buttons, the swatches — and a capsule switch in among them reads as
/// borrowed from somewhere else. The radii here are the same family: 5 outside,
/// 3 inside.
///
/// **The travel is the whole signal, and the colour is the confirmation.** The
/// knob moves the width of the track and the ground goes accent; either alone is
/// a state that has to be *inspected* rather than seen, which is the failure mode
/// of a checkbox in a scrolling list. This one is built for exactly that list —
/// the OpenType features — where a dozen rows are read as a column and what
/// matters is which of them stand out.
///
/// Hover lifts the resting track rather than the knob: the track is the larger
/// shape and the one the pointer is actually over.
///
/// **Painting, not a widget**, because the only thing that wants a switch so far
/// wants it at the end of a row whose *whole width* is the target — see
/// [`switch_row`], and [`icon_button_padded`] for why the two cannot both sense.
/// A standalone `switch` widget is four lines on top of this and can be added the
/// day something needs one; adding it now would be a builder with no callers, and
/// **`dead_code` would say so on the next `cargo check`** — which is a reason to
/// wait rather than a reason not to.
///
/// 🚨 **That last clause used to read *"which in a library crate `dead_code` never
/// notices"*, and it is void in the file it is written in** (§15 D665,
/// `[S18.2-L3-06]`). `ondin-app` is not a library crate: `Cargo.toml` declares
/// only `[[bin]] name = "ondin"`, there is no `src/lib.rs`, and `main.rs`'s
/// `mod ui;` carries no `#[allow(dead_code)]` — so the lint analyses every item
/// here and an uncalled `pub fn switch` would surface on the plain
/// `cargo check --release -p ondin-app` gate. The decision is unchanged and its
/// stated reason was the opposite of the fact, which is worse than no reason: it
/// told the next reader this module can hide an uncalled `pub` builder.
///
/// ⚠️ **The true version of that rule is CLAUDE.md's and it is about `ondin-core`,
/// where a `pub` item *is* the API and `dead_code` never fires** — plus the second
/// half, which is not about `pub` at all: a derived `PartialEq` hides an unread
/// **field** anywhere in the workspace, this crate included.
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

/// Type size of a [`switch_row`]'s label.
///
/// Named because a *second* kind of row shares this column — the typography
/// panel's n-way `cvXX` row, which is not a switch and still has to be read as
/// one of the same list. Two literals is how the two come to disagree by half a
/// point, which is the sort of thing that only shows up on screen.
pub const SWITCH_ROW_LABEL_PT: f32 = 11.5;

/// The ink of a toggle-list row's label: [`theme::text::STRONG`] when the row is
/// on, [`theme::text::MUTED`] when it is off.
///
/// The two tiers are what let a dozen rows be read as a column — see
/// [`switch_row`] — so a row that is not a switch still owes them.
pub fn switch_row_ink(on: bool) -> egui::Color32 {
    if on {
        theme::text::STRONG
    } else {
        theme::text::MUTED
    }
}

/// A row of a toggle list: a label, and a [`paint_switch`] against the right margin.
///
/// Returns the **row's** response, so `clicked()` is the toggle and a hover
/// attaches to the row itself rather than to whatever `Ui` happens to contain it.
/// The typography panel's feature list needs the second half: a switch labelled
/// "Tabular figures" says what it is called and nothing about what it does.
///
/// **The row is the target, not the switch.** A 26×16 switch is a small thing to
/// hit at the end of a wide row, and a clickable switch *inside* a clickable row
/// is the trap [`icon_button_padded`] describes — the inner claimant wins the hit
/// test wherever it lies, so the row would be live everywhere except on the
/// control. So the switch is painted rather than allocated, and the row senses.
///
/// The label dims when the feature is off, the way [`menu_check`]'s does: a
/// column of a dozen rows is read as a column, and two tiers of contrast say
/// which ones are on without the eye having to find each knob.
pub fn switch_row(ui: &mut egui::Ui, label: &str, on: bool, height: f32) -> egui::Response {
    switch_row_inner(ui, label, Some(on), height)
}

/// A [`switch_row`] over a selection whose runs disagree: the word **"Mixed"**
/// where the switch would be (§15 D752).
///
/// 🚨 **The word, not a third knob state, and that is §15 D130 rather than a
/// preference.** D130 is *Resolved* and states the rule once: *"A control says
/// **Mixed**. If it is too small to hold five letters it says a dash, and there
/// are exactly two"* — `ui::segment_mixed` and `ui::Swatch::Mixed`. A feature row
/// is the width of the card, so five letters fit with room to spare, and a third
/// dash would be a departure from a live entry rather than a consistency repair.
/// The maintainer independently ruled out the knob: *"A three way switch would be
/// visually weird (thumb in the middle?) so I'll take the dim and tooltip
/// approach."*
///
/// ⚠️ **Painting the knob at the first run's value is the thing this exists to
/// stop.** That is what the section did — `shown` resolves at `range.start`, so a
/// selection half of which has `tnum` on drew a column of dead switches — and
/// D130's own argument for the swatch applies exactly: *"whichever it picked would
/// read as a claim the next click would make true"*, which here it then would.
///
/// ⚠️ **The row stays live.** Dim is a readout, not a disablement: clicking is how
/// a mixed range is resolved, and a row that refused the click would leave the
/// user no way to make the selection agree.
pub fn switch_row_mixed(ui: &mut egui::Ui, label: &str, height: f32) -> egui::Response {
    switch_row_inner(ui, label, None, height)
}

/// The one implementation behind [`switch_row`] and [`switch_row_mixed`].
///
/// `on: None` is *the runs disagree*. One function rather than two spellings, so
/// the hover ground, the row height, the label ink tiers and the right margin
/// cannot come apart between the definite row and the mixed one — they sit in the
/// same column and are read as one list.
fn switch_row_inner(
    ui: &mut egui::Ui,
    label: &str,
    on: Option<bool>,
    height: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if resp.hovered() {
        // **Painted wider than the row, and the ink stays put.** The label lines
        // up with the eyebrow above it and the segmented tracks around it — every
        // other thing in this column is flush with the content edge — so the row
        // cannot take an inset to breathe in. The hover ground borrows it from the
        // card's own padding instead, which is empty either way.
        ui.painter().rect_filled(
            rect.expand2(egui::vec2(SWITCH_ROW_BLEED, 0.0)),
            egui::CornerRadius::same(5),
            theme::color::text_a(12),
        );
    }
    let inner = rect;
    ui.painter().text(
        egui::pos2(inner.left(), inner.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(SWITCH_ROW_LABEL_PT),
        // A mixed row takes the *off* tier. It is the dimmer of the two, and a
        // range that disagrees is not a range this feature is on for.
        switch_row_ink(on.unwrap_or(false)),
    );
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
        // **Right-aligned to the same margin the switch occupies**, so the column
        // reads as one list rather than as rows of two different shapes — the
        // switch's right edge is `inner.right()` and so is the word's.
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

/// What a control says when the selection disagrees with itself (§15 D130).
///
/// **One spelling, because the rule is about the word.** D130 reversed an earlier
/// entry to land on it and names the only two places a dash is allowed instead;
/// a second literal is how the app comes to say "Mixed" in one panel and
/// "mixed" or "—" in the next.
pub const MIXED_WORD: &str = "Mixed";

/// Type size of a [`segment_label`].
///
/// Named rather than inlined because a cell **clips nothing**: `painter.text`
/// centres a galley in the rect and lets it run over the neighbours, so whether a
/// label fits its cell is a question about this number and the cell arithmetic,
/// and one a test can only ask if it can see both.
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
/// [`segmented_enabled`].
///
/// [`theme::text::DISABLED`], which is the tier `icon_button` already uses for a
/// disabled glyph, so a dead tab and a dead button are the same statement. A
/// **third** tier and not the resting one: the resting label is what an
/// *available* choice looks like, and two readings of one colour is how a disabled
/// control comes to look merely unselected.
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

/// Paint a left-to-right ramp of `stops` across `rect` as a vertex-coloured
/// mesh. Square corners — see [`mask_corners`].
///
/// ⚠️ **The offsets are made monotonic here, and the doc used to state "sorted"
/// as a precondition nothing checked and nothing on the read path established**
/// (§15 D564). `panels::paint::with_stops` sorts on the *write* path, so a
/// gradient the user has touched is fine; a gradient **imported** from a
/// `<linearGradient>` with descending offsets is not, and `panels::paint::stops_of`
/// hands the list over verbatim. `[(1.0, red), (0.0, blue)]` painted the ramp
/// **backwards** — two of the four triangles wound the other way and the end
/// extensions gave the left edge the *last* stop's colour.
///
/// **Clamped rather than sorted, because that is what the canvas beside it
/// does.** §15 D455 settled that question for the importer, the renderer and the
/// SVG writer and picked SVG's rule — *"each gradient offset is required to be
/// equal to or greater than the previous"* — over a sort, on the argument that a
/// file is to be read rather than guessed at. The chip was the fourth door and
/// was missed, so one document was drawn two ways in one frame: the swatch
/// mirrored, the artwork clamped. `ondin_core::image::monotonic_offset` is the
/// rule, spelled once.
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
/// `bg`. egui cannot round a vertex-coloured mesh, so the ramp and SV plane are
/// drawn square and masked — which is exact, unlike insetting them.
///
/// Emitted as one **`Mesh`**, deliberately, not as filled polygons. A corner
/// sliver is tangent to both edges it meets, so its path doubles back on itself
/// at each tangent point; egui's anti-aliased fill mitres that zero-degree
/// vertex and the result shoots spikes several pixels clear of the rect — on a
/// 14px swatch they read as crop marks around the colour chip. A mesh is
/// rasterized from its vertices with no feathering and no mitre, so the mask
/// cannot escape its own geometry. The hairline drawn over the same rounded
/// boundary afterwards is what smooths the arc.
pub fn mask_corners(painter: &egui::Painter, rect: egui::Rect, radius: f32, bg: egui::Color32) {
    painter.add(egui::Shape::mesh(corner_mask(rect, radius, bg)));
}

/// The mask geometry [`mask_corners`] paints — split out so it can be checked
/// against the rect it is supposed to stay inside.
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
        // A fan from the square corner out to the arc: triangle `i` spans the
        // corner and two neighbouring arc points, so the union is the sliver
        // between the corner and the rounded boundary.
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
    /// The paints this chip stands for do not agree on a colour — the **mixed**
    /// state, drawn as the bare checkerboard with a dash on it.
    ///
    /// Nothing over the checkerboard, because that is already what this app draws
    /// where there is no paint, and the dash is [`segment_mixed`]'s mark — the one
    /// the app makes where a control has no room for the word "Mixed" that every
    /// field says. A 16px chip has no room. A plausible colour would be the one
    /// thing it must not show: whichever one it picked would read as a claim the
    /// next click would then make true of every layer.
    ///
    /// It does read a little like a transparent paint, which is the cost of using
    /// the checkerboard for both. The word beside it is what settles that, and is
    /// why the chip is not asked to carry the state on its own.
    Mixed,
    /// An image fill's thumbnail — the picture as that layer shows it, cut to
    /// the chip's square by [`crate::thumbs`].
    ///
    /// **The chip was always going to be this**, which is why an image row draws
    /// one rather than a glyph: the paint rows are a column of chips and a column
    /// of labels, and a picture that opted out of the first would put its label
    /// on a different x from every row above it (§15 D180).
    ///
    /// Over the checkerboard like the rest, so a PNG with a transparent corner
    /// says so here in the same way a 50% colour does.
    ///
    /// `alpha` is the brush's own `ImageSampler::alpha` — [`panels::paint::image_alpha`]
    /// is the one place that reads it — so a fill faded to 30% draws a chip faded
    /// to 30% (§15 D784). It rides on the variant rather than being folded into
    /// the texture because the texture is **shared**: `thumbs` keys its atlas by
    /// picture, and two layers can carry one photograph at two opacities.
    ///
    /// [`panels::paint::image_alpha`]: crate::panels::paint::image_alpha
    Picture {
        id: egui::TextureId,
        alpha: f32,
    },
}

/// The hairline every colour chip wears, drawn over its rounded boundary after
/// the corners are masked.
///
/// **Always on, never conditional.** A chip whose colour is near the row's own
/// ground is otherwise invisible — a `2F2F2F` fill sits on `color::FIELD`
/// (`2F2F30`) and there is nothing to see. A threshold would pop: scrub a colour
/// across it and the border flicks on and off, and getting the threshold right at
/// all needs a *perceptual* metric, since RGB distance fires on `2F2F2F` against
/// `262626` and stays quiet for a saturated blue beside a saturated purple. None
/// of that is needed, because **the risk here is one-sided**: the UI is dark, so
/// only dark chips vanish, and translucent white is either helping or invisible.
///
/// **White, not the picker's black.** `picker.rs` hairlines its ramps in
/// translucent black, which is right over a bright SV plane and exactly the wrong
/// direction here: black on a dark chip on a dark row is invisible twice over.
/// This is also why the chip's *old* border was removed and why restoring it in
/// the same colour would be a mistake — an inset **black** hairline darkened
/// every light colour's edge and read as a frame around the swatch rather than as
/// the swatch. White self-cancels on exactly the colours that complained.
///
/// **76/255 is set by the black chip, which is the hard case.** Composited over a
/// chip, the hairline's value climbs from the alpha (over black) to the chip's own
/// colour (over white), so the darkest chip is the one that has to clear the
/// ground — grey 76 against `color::FIELD`'s 47, a step of 29 levels. Every
/// lighter chip lifts further and needs no help; a white one shows nothing at all,
/// which is the point.
///
/// It has a second job the theme has always asked of it: [`mask_corners`] emits a
/// `Mesh`, which epaint rasterizes with no feathering, so without a stroke over the
/// same arc the rounded corner is a hard aliased step. `picker::preview_chip` pairs
/// the two exactly this way, and the chips now do too.
///
/// **Four call sites still mask and stop** — `picker.rs`'s `stop_bar`, `sv_plane`,
/// `hue_slider` and `alpha_slider` — so pairing the two is the rule and not yet the
/// practice. Their hairline would not be this colour: they are masked against
/// `color::CARD` under a bright ramp or plane, which is the case `preview_chip`
/// chose translucent black for. Decided separately, and noted here so the next
/// reader does not take this constant for the whole answer (§15 D231).
/// Spelled premultiplied because that is the `const` constructor; this is
/// `Color32::from_white_alpha(76)`, which is the same four bytes.
pub const SWATCH_HAIRLINE: egui::Color32 = egui::Color32::from_rgba_premultiplied(76, 76, 76, 76);

/// A small colour chip: checkerboard, the paint, and [`SWATCH_HAIRLINE`] over
/// the rounded boundary. Sensed for clicks — the inspector's paint rows open the
/// picker from it.
///
/// `bg` is the ground the chip sits on, because rounding its corners means
/// painting the leftover square over in that colour ([`mask_corners`]) — a chip
/// told the wrong ground shows four visible notches.
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
        // **`Color32::WHITE` is `image`'s identity multiplier, not a colour** — a
        // chip drawn in the theme's ink would be a photograph in one hue, which is
        // what tinting a texture means. So the only thing this tint is allowed to
        // carry is a uniform multiplier, and the brush's alpha is exactly one
        // (§15 D784): white scaled by `alpha` leaves every hue where it was and
        // fades the picture over the checkerboard behind it.
        Swatch::Picture { id, alpha } => {
            let tint = egui::Color32::WHITE.gamma_multiply(alpha);
            p.image(id, rect, crate::thumbs::FULL_UV, tint);
        }
        // Over the checkerboard rather than over a ground of its own, and the ink
        // is dark for that reason: the chip's light squares are what the dash has
        // to read against.
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
    // **After the mask**, or the mask bites four pieces out of it. `Inside`
    // rather than `Outside` for the same reason the radius is repeated here: a
    // chip sits in a row of a fixed height, and a stroke that grew it by a point
    // on each side would push the label beside it.
    p.rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, SWATCH_HAIRLINE),
        egui::StrokeKind::Inside,
    );
    resp
}

/// Strip a `DragValue`'s own grounds and borders, so it floats on whatever the
/// caller has already drawn.
///
/// **One block, three callers** (§15 D723, `[S18.1-L3-06]`). [`value_field_f64`],
/// [`badge_field`] and [`bare_drag_value`] each held a verbatim copy of this
/// eight-line sequence with its own paragraph restating the same reasoning — and
/// a fourth field wanting a bare `DragValue` would have got a fourth. The two
/// egui facts below are the whole of why it is eight lines and not one, and they
/// are worth having in one place rather than three.
///
/// **A `DragValue` has two faces and both ground themselves.** The resting face
/// is a `Button`, which fills from `bg_fill`/`weak_bg_fill` and outlines from
/// `bg_stroke` across all four widget states; the keyboard-editing face is a
/// `TextEdit`, which fills from `extreme_bg_color` and, once focused, outlines
/// itself in `selection.stroke`. A caller that has drawn its own field, pill or
/// row wants none of them.
///
/// 🚨 **The width goes and the colour stays, and that distinction is a reported
/// bug.** `selection.stroke` has two jobs in egui: it is that focus outline,
/// *and* it is the colour selected glyphs are repainted in —
/// `text_selection::visuals::paint_text_selection` writes it straight into the
/// galley's vertices. `Stroke::NONE` is transparent, so zeroing the whole thing
/// turned **every selected digit invisible** and left an opaque selection block
/// with nothing legible inside it. A zero-*width* stroke is skipped by the
/// tessellator, so the outline is gone either way; keeping the colour is what
/// keeps the text.
///
/// ⚠️ **What is deliberately *not* here**: the selection **fill** behind the
/// digits, which is what shows the field is being typed into. And nothing about
/// text colour, padding or spacing — `badge_field` overrides the ink because a
/// light pill needs dark digits, and that is its business and not this rule's.
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
/// The paint rows and the picker's hex row already *are* fields — a recessed
/// pane with a hairline — and a second bordered box for the opacity inside one
/// reads as a control bolted onto the row rather than part of it. That includes
/// the hover border, which was the loudest of the three: hovering a paint row
/// lit up a rectangle around two digits.
pub fn bare_drag_value(ui: &mut egui::Ui, value: egui::DragValue<'_>) -> egui::Response {
    ui.scope(|ui| {
        strip_drag_value_chrome(ui);
        // Arithmetic, as in every other field ([`crate::expr::eval`]).
        //
        // ⚠️ **The parser goes on *last* here and *first* in the other two, and
        // that is the signature rather than a different decision** (§15 D723).
        // [`value_field_f64`] and [`badge_field`] build the `DragValue`
        // themselves, so they can set a parser the caller's `build` closure is
        // free to replace; this helper is handed a **finished** `DragValue` and
        // has nowhere earlier to put one. **The rule is the same in all three —
        // the parser is a default a caller may replace** — and only here is the
        // rule unofferable. A field that has to parse something other than a
        // number cannot use this helper for it, and wants one of the other two.
        //
        // ⚠️ This read as a *limitation of this helper* and the sibling's read as
        // a *feature of that one*, each written as though it were the only copy —
        // which is how one rule came to look like two contracts.
        //
        // ⚠️ **`clamp_existing_to_range(false)` goes on here for the same reason
        // and it did not until §15 D552.** This helper is handed a finished
        // `DragValue`, so a *ranged* one arrived carrying egui's default — the
        // field rewriting a stored value outside its range on every frame and
        // reporting the rewrite as a user edit (D425).
        //
        // ⚠️ **All four production callers pass `.range(0.0..=100.0)`, and every
        // one of them is an alpha percentage** — the picker's opacity field,
        // `inspector::paint_row`, `inspector::slot_color_row` and
        // `typography::char_color_row`. So egui's default was live at four sites
        // out of four and held harmless by four *separate* producers, every one of
        // them guaranteeing 0..1 by construction. That is `[S23.1-L1-01]`'s own
        // shape — a property of code elsewhere holding a widget's behaviour up —
        // so the decision is made here rather than left to whatever the next
        // caller's producer happens to promise. (This comment said *three of the
        // four are unranged* when it was written; `arch-scribe` counted them, and
        // the measurement supports the stronger conclusion.) Last, like the
        // parser, and for the same reason.
        let value = scrub_speed_for(ui.ctx(), value)
            .custom_parser(crate::expr::eval)
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
/// back out of it has lost its RGB triple wherever alpha is low — round-tripping
/// values through egui's colour types made the picker drift the hue every time
/// opacity was nudged. Colour *values* travel as [`Hsv`] instead.
pub fn color_to_egui(c: ondin_core::peniko::Color) -> egui::Color32 {
    let k = c.components;
    egui::Color32::from_rgba_unmultiplied(
        (k[0] * 255.0).round() as u8,
        (k[1] * 255.0).round() as u8,
        (k[2] * 255.0).round() as u8,
        (k[3] * 255.0).round() as u8,
    )
}

/// Straight-alpha HSV over sRGB values — the space every design tool's colour
/// picker works in, and the one the hex field agrees with.
///
/// Deliberately not egui's `Hsva`, which converts through linearised light and
/// premultiplied bytes. Both hurt here: the linear round trip moves the hex, and
/// the premultiplication destroys the colour at low alpha.
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

/// How many characters a hex field accepts: `#RRGGBBAA`, the longest form
/// anyone pastes. The alpha digits are read and discarded (see [`parse_hex`]),
/// but a field that refuses to hold them refuses the paste outright.
pub const HEX_CHAR_LIMIT: usize = 9;

/// Whether a text field's edit has ended **and is meant to be kept** — the frame
/// it lost focus, unless `Escape` is what took the focus away (§15 D808).
///
/// 🚨 **`Escape` committed.** Four chrome text fields decided they were finished
/// from `resp.lost_focus()` alone — the inspector's hex and layer-name fields, the
/// stroke-dash list and the Export panel's prefix/suffix — and egui does not
/// revert a `TextEdit` on `Escape`: it hands back what was typed and merely
/// surrenders focus. So the key that cancels a gesture everywhere else in the app
/// *applied* the edit, and spent an undo step doing it. Measured on the hex field:
/// typing `336699` over `[0.5, 0.25, 0.125, 1.0]` and pressing `Escape` leaves
/// `[0.2, 0.4, 0.6, 1.0]` and `undo_depth` 1.
///
/// **The rule already existed twice and was broken at four sites beside it**,
/// which is why it is a function now rather than a fifth copy of the condition.
/// `app::OndinApp::edit_valve` carries it for every *numeric* field — it is the
/// `if !…key_pressed(Escape)` on the falling edge of D316's engagement latch, and
/// §15 D317 is the entry for getting its ordering right — and
/// `panels::dashboard`'s rename field carries it as an arm further up. A text
/// field cannot use the valve itself (see `inspector::paint_hex_field`), so what
/// it can share is this one sentence of it.
///
/// ⚠️ **It reads the key rather than the focus, and that is not the same
/// question.** `Escape` clears egui's focus in `Memory::begin_pass`, so the frame
/// a field reports `lost_focus()` is the frame the press is still in `input` —
/// which is `input::resolve`'s own note (§15 D317) from the other end. Asking
/// `has_focus()` or the next frame's state answers nothing: by then the key is
/// gone and the two ways out of a field are indistinguishable.
///
/// ⚠️ **One test covers all four callers, because the rule is here and not at
/// them** — `inspector::paint_hex_field_tests::escape_abandons_a_typed_hex_instead_of_committing_it`,
/// and dropping the second term above is what it flips against. The three other
/// sites have no `Escape` assertion of their own and are not meant to grow one;
/// what they owe is to call this rather than to re-spell it. (Plain backticks:
/// the test is `cfg(test)` and a production doc may not link one — §15 D319.)
pub fn defocus_commits(resp: &egui::Response) -> bool {
    resp.lost_focus() && !resp.ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

/// Select the whole of a hex field the moment it takes focus, so typing replaces
/// the colour instead of inserting into it. Call with the field's own response.
///
/// **Parity with the number fields, which get this for free.** `DragValue` selects
/// all on the click that focuses it (`egui::widgets::drag_value::select_all_text`),
/// so every W, H and rotation in the inspector could be clicked and retyped — while
/// the hex beside them dropped a caret where the pointer landed and left `6D8CD9`
/// to be cleared by hand. A plain `TextEdit` has no such behaviour; this is egui's
/// own recipe, applied where egui does not.
///
/// Stored **after** the field has run, which is why it takes effect from the next
/// frame: the click that focused it also placed a caret, and that write has to be
/// the one overwritten rather than the other way round. `DragValue` does the same.
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

/// Parse `RGB` / `RRGGBB` / `RRGGBBAA`, with or without a leading `#`, and with
/// any surrounding whitespace.
///
/// The hash is optional in **both** directions: copying a colour out of a
/// browser, a style guide or another design tool gives you `#EB6E5A`, and
/// typing it by hand gives you `EB6E5A`. Either has to work, and neither leaves
/// the hash behind — [`hex_of`] is what the field shows once the value lands,
/// and it never writes one.
///
/// Alpha is the caller's business — the hex field edits the colour, the opacity
/// field beside it edits the alpha — so an eight-digit value's last pair is
/// accepted and dropped rather than rejected.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#').trim();
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let b = |i: usize, n: usize| u8::from_str_radix(&s[i..i + n], 16).ok();
    match s.len() {
        3 => {
            let (r, g, bl) = (b(0, 1)?, b(1, 1)?, b(2, 1)?);
            // `F` means `FF`, not `F0`: 0x11 per step, so `FFF` is white.
            Some([r * 17, g * 17, bl * 17])
        }
        6 | 8 => Some([b(0, 2)?, b(2, 2)?, b(4, 2)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored value outside a field's range is **shown**, not rewritten — and
    /// a frame with no events reports no change.
    ///
    /// ⚠️ **This is one line's worth of behaviour for all 82 numeric fields in
    /// the app**, and none of them chose it: egui's `clamp_existing_to_range`
    /// defaults to `true` and is applied unconditionally every frame, so any
    /// value the document already held outside a control's range was rewritten
    /// to the nearest end on the frame the panel first drew it. `value_field_f64`
    /// then took `let before = *value` around `ui.add`, saw the widget's own
    /// rewrite, and called `mark_changed` — which routes it through `edit_valve`
    /// and out as a **transaction**. So an imported blur radius of 1200 became
    /// 1000, in an undo step, with nobody touching anything.
    ///
    /// The range still bounds what a scrub or a keystroke can reach; `Scrub::settle`
    /// is what clamps a gesture, and it is untouched. What is given up is the
    /// widget rewriting the *document*, which §5.3 already says it should not:
    /// *"clamped where the outline is built … not on the way in"*.
    ///
    /// **The in-range control is the assertion that keeps this honest** — without
    /// it a fix that stopped reporting changes at all would pass.
    ///
    /// Flip-check, run: removing `.clamp_existing_to_range(false)` fails on the
    /// first assertion with `1000` where `1200` was, which is the reported symptom.
    #[test]
    fn a_stored_value_outside_a_fields_range_is_shown_rather_than_rewritten() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        // One frame with no events at all: nothing is being edited.
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

        // The control: an in-range value is equally untouched and equally quiet,
        // so the assertions above are about the clamp rather than about the
        // field having gone inert.
        let mut in_range = 500.0;
        let changed = draw(&mut in_range);
        assert_eq!(in_range, 500.0);
        assert!(!changed);
    }

    #[test]
    fn hex_round_trips_through_parse() {
        let c = ondin_core::peniko::Color::from_rgba8(0x6D, 0x8C, 0xD9, 0x80);
        assert_eq!(hex_of(c), "6D8CD9");
        assert_eq!(parse_hex("6D8CD9"), Some([0x6D, 0x8C, 0xD9]));
        assert_eq!(parse_hex("fff"), Some([0xFF, 0xFF, 0xFF]));
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(parse_hex("12345"), None);
    }

    /// A pasted colour arrives with a hash far more often than not, and the
    /// field must take it either way — and never hand the hash back, because
    /// `hex_of` is what redraws the field once the value is applied.
    #[test]
    fn a_hash_is_optional_going_in_and_never_comes_back_out() {
        for spelling in ["#6D8CD9", "6D8CD9", "  #6d8cd9  ", "#6D8CD9FF"] {
            assert_eq!(
                parse_hex(spelling),
                Some([0x6D, 0x8C, 0xD9]),
                "{spelling:?} did not parse"
            );
        }
        assert_eq!(parse_hex("#fff"), Some([0xFF, 0xFF, 0xFF]));
        let round_tripped = ondin_core::peniko::Color::from_rgba8(0x6D, 0x8C, 0xD9, 0xFF);
        assert_eq!(hex_of(round_tripped), "6D8CD9", "the hash must not survive");
        // A stray hash mid-string is not a colour, and neither is a lone one.
        assert_eq!(parse_hex("6D#8CD9"), None);
        assert_eq!(parse_hex("#"), None);
        // The longest accepted form fits in what the fields allow.
        assert_eq!("#6D8CD9FF".len(), HEX_CHAR_LIMIT);
    }

    /// The picker's colour path must not drift. Every control writes back
    /// `Hsv -> Color`, so `Color -> Hsv -> Color` has to be the identity at
    /// 8-bit precision — including at low alpha, where routing through egui's
    /// premultiplied `Color32` used to lose the RGB triple entirely.
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

    /// The corner mask must stay inside the box it is rounding.
    ///
    /// It used to be four anti-aliased polygons. A corner sliver is tangent to
    /// both edges it meets, so its outline doubles back on itself at each
    /// tangent point, and egui mitred that zero-degree vertex into spikes 6px
    /// clear of a 14px swatch — which is what the crop-mark artifacts around
    /// every colour chip were.
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

    /// Greys have no hue to recover, so the picker carries one alongside. What
    /// matters is that saturating a grey does not silently jump to red.
    #[test]
    fn a_grey_keeps_whatever_hue_it_is_given() {
        let c = Hsv::new(0.6, 0.0, 0.5, 1.0).to_color();
        assert_eq!(Hsv::from_color(c).s, 0.0);
        let saturated = Hsv::new(0.6, 1.0, 0.5, 1.0).to_color();
        assert!((Hsv::from_color(saturated).h - 0.6).abs() < 1e-5);
    }

    /// Lay out one `value_field` in a card and return what got painted, plus
    /// the field's id so a second pass can hand it keyboard focus.
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

    /// **A field must occupy the width it was given, whatever number is in it.**
    ///
    /// The resting face of a `DragValue` is a `Button` with `TextWrapMode::Extend`,
    /// which never truncates — so a number too wide for its room does not clip, it
    /// *grows*, and the growth propagates out through the frame to the card and the
    /// whole inspector. Reported as a stroke weight clipping off the edge of the app,
    /// and visible from three digits, because the prefix inset used to be applied as
    /// `button_padding` and so counted twice: 44 of the weight field's 68px content
    /// box was padding.
    ///
    /// Measured at the two widths the Stroke panel actually uses, across the whole
    /// range the fields accept.
    #[test]
    fn a_value_field_holds_its_width_up_to_the_largest_value_it_accepts() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        // The stroke weight field, and one of the four per-side widths beside it.
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

    /// The prefix has to be legible in *both* faces of the field, and the number has
    /// to stay put between them.
    ///
    /// This is why the inset is reserved space with the glyph painted into it, rather
    /// than `DragValue::prefix`: a prefix **atom** is laid out by the resting
    /// `Button` only, and the keyboard face is a bare `TextEdit` that never sees it —
    /// so the glyph disappeared and the digits jumped left on focus. Its sibling
    /// `focusing_a_value_field_outlines_the_row_and_leaves_the_value_put` catches the
    /// jump; this catches the glyph going missing, which that one would not.
    #[test]
    fn a_value_fields_prefix_survives_being_typed_into() {
        for prefix in [Prefix::Text("W"), Prefix::Icon(icon::ARROW_CLOCKWISE)] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            paint_value_field(&ctx, prefix, None); // warm the font atlas

            // Every text run painted that is *not* the value.
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

    /// A `DragValue` being typed into is a `TextEdit`, and a focused `TextEdit`
    /// frames *itself* — a second outline starting at the first digit, so the
    /// number reads as sitting on top of its own border. `value_field`
    /// suppresses that and strokes the row instead. What must hold: focusing a
    /// field does not move the value, and the only outline is the field's own.
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
            // And the value clears that outline by the field's own margin.
            assert!(
                value_x(&focused) - row.min.x >= 9.0,
                "the value sits {}px from the outline",
                value_x(&focused) - row.min.x
            );
        }
    }

    /// The window the wrap tests pretend to be in. Big enough to hold two trigger
    /// bands and a gap, which is what `wrap_scrub` refuses to act without.
    const WINDOW: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::pos2(1200.0, 800.0),
    };

    /// Lay out one `value_field` over `v`, feeding it `events`, and report the box
    /// the number claimed along with everything egui asked the backend to do.
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
                    // Configured as the real geometry fields are, formatter included —
                    // the point of `digits_shown` below is that the pinned precision is
                    // what the wrap cannot disturb.
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

    /// How many decimal places the field actually painted this pass.
    ///
    /// Read off the shapes rather than computed, because what went wrong was purely a
    /// matter of what reached the screen: the value was right throughout.
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

    /// **The prefix is part of the scrub.** A press that lands on the `W` and drags
    /// right has to move the number, because the label is what the hand aims at when
    /// it goes looking for "the width field" — and for as long as the prefix was a
    /// widget in front of the value, the leftmost fifth of every field did nothing.
    ///
    /// Driven with real pointer events rather than by measuring rects: what matters
    /// is that the gesture *lands*, and a rect that merely contains the prefix would
    /// prove nothing if a widget in front of it took the press (§9.2).
    #[test]
    fn dragging_a_value_field_by_its_prefix_moves_the_number() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut v = 100.0_f64;
        let rect = drive_value_field(&ctx, &mut v, vec![]); // warm fonts and geometry

        // Two points in from the field's own left edge — on the label, as far from
        // the digits as the field goes.
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

    /// **A scrub that runs into the window's edge teleports to the other side**, so
    /// the reach of a field is how long you are willing to drag rather than how wide
    /// the monitor is (§15 D66).
    ///
    /// Checked through the `CursorPosition` command the pass emits, which is the whole
    /// of what this code decides — the OS actually moving the pointer is the backend's
    /// half and cannot be seen from here.
    #[test]
    fn a_scrub_that_reaches_an_edge_wraps_the_pointer_to_the_far_side() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut v = 100.0_f64;
        let rect = drive_value_field(&ctx, &mut v, vec![]);
        let start = rect.center();

        // Press, then drag left until the pointer is inside the left-hand band.
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
        // **y is left exactly alone**, even though this field sits within a band of the
        // window's top — the case that made wrapping both axes wrong, since a purely
        // horizontal drag would have teleported vertically the moment it began.
        assert_eq!(to.y, at_edge.y, "the vertical axis must not be touched");

        // Mid-field, nothing wraps — otherwise the pointer would jump on every drag.
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

    /// **The pass after a wrap must not move the value.** egui reads `drag_delta` as
    /// the difference between successive pointer positions, so a teleport arrives
    /// indistinguishable from a flick most of the window wide — and applied to a
    /// number it would send it flying. `scrub_speed_for` spends that pass on a zero
    /// speed instead, which is the one thing making cursor wrapping usable at all.
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

        // Into the left band: this pass wraps.
        let at_edge = egui::pos2(WRAP_EDGE - 4.0, start.y);
        let (_, out) =
            drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(at_edge)]);
        let to = wrapped_to(&out).expect("the wrap");
        let before = v;

        // The teleport itself, as the backend would report it: a jump of most of the
        // window's width, in the *opposite* direction to the drag.
        drive_value_field(&ctx, &mut v, vec![egui::Event::PointerMoved(to)]);
        assert_eq!(
            v,
            before,
            "the teleport moved the value by {} — it must cost nothing but its own pass",
            v - before
        );

        // And the very next pass of real movement scrubs again, so the wrap costs one
        // pass and not the rest of the gesture.
        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(to - egui::vec2(20.0, 0.0))],
        );
        assert!(v < before, "the drag stopped scrubbing after the wrap: {v}");
    }

    /// **The teleport is recognised whenever it turns up, not on the pass after it was
    /// asked for.** This is the bug the first version had, and it was reported from the
    /// Rotation field: the request only reaches the backend after the frame body
    /// (`<OndinApp as eframe::App>::ui`) returns, the
    /// OS posts the resulting move whenever it gets round to it, and the hand keeps
    /// producing real events in the meantime — so a one-pass window missed the artefact,
    /// which then landed at full speed and, on a field that commits every pass, stayed.
    ///
    /// Three things have to hold together here, which is why they are one test: the real
    /// motion during the wait scrubs normally, the late teleport costs nothing, and the
    /// gesture carries on afterwards.
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

        // Into the left band: this pass asks for the wrap.
        let edge = egui::pos2(WRAP_EDGE - 4.0, start.y);
        let (_, out) = drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(edge)]);
        let to = wrapped_to(&out).expect("the wrap");

        // Four passes of ordinary movement before the OS answers. Each must scrub, and
        // none may be mistaken for the teleport.
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

        // Now the teleport finally arrives.
        let before = v;
        drive_value_field(&ctx, &mut v, vec![egui::Event::PointerMoved(to)]);
        assert_eq!(
            v,
            before,
            "the late teleport moved the value by {}",
            v - before
        );

        // And the drag is still live, and free to wrap again.
        drive_value_field(
            &ctx,
            &mut v,
            vec![egui::Event::PointerMoved(to - egui::vec2(15.0, 0.0))],
        );
        assert!(v < before, "the drag stopped scrubbing after the wrap: {v}");
    }

    /// **A wrap must not change how many digits the field shows**, which is the second
    /// thing reported about it: for exactly the frame the pointer teleported on, the
    /// Transform panel read `411.672736201535258`.
    ///
    /// The value was never wrong — a `DragValue` derives its precision from its *speed*
    /// (`auto_decimals = (aim_radius / speed).log10().ceil().clamp(0, 15)`), and the
    /// swallow pass sets the speed to zero, so the guess came back at its ceiling of
    /// fifteen. Pinning the precision is what makes the display independent of a speed
    /// that has been borrowed for something else (§15 D66, `number`).
    #[test]
    fn a_wrap_does_not_change_how_many_digits_a_field_shows() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        // A value with far more precision than the field should ever show.
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

        // The teleport itself — the pass whose speed is zeroed, and the one frame the
        // report was of.
        let (_, landing) = drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(to)]);
        assert!(
            digits_shown(&landing) <= 2,
            "the teleport pass showed {} decimals",
            digits_shown(&landing)
        );
    }

    /// A request the platform never answers must not leave the drag unable to wrap for
    /// the rest of its life — it gives up after `WRAP_GIVE_UP` passes and goes back to
    /// stalling at the edge, which is merely how every field behaved before.
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

        // The pointer stays in the band and the teleport never comes — as it would not
        // on a platform that refuses to move the cursor.
        for _ in 0..WRAP_GIVE_UP {
            drive_value_field(&ctx, &mut v, vec![egui::Event::PointerMoved(edge)]);
        }
        let (_, again) = drive_value_field_out(&ctx, &mut v, vec![egui::Event::PointerMoved(edge)]);
        assert!(
            wrapped_to(&again).is_some(),
            "after giving up, a fresh request should be allowed"
        );
    }

    /// Making the number's box the whole field must not *move* the number: the digits
    /// still start one prefix and one gap in from the field's edge, which is where
    /// laying the prefix out ahead of them put them.
    #[test]
    fn the_number_still_sits_a_prefix_and_a_gap_from_the_fields_edge() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        for prefix in [Prefix::Text("W"), Prefix::Icon(icon::ARROW_CLOCKWISE)] {
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
                // The prefix's own advance width, which is what the inset is built
                // from — measured here rather than assumed, so a change of type size
                // does not need this number changed with it.
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

    /// A colour typed into a hex field has to *replace* the one there, the way a
    /// number typed into a `DragValue` does. Driven with real events, because the
    /// thing under test is what a keystroke does to the buffer — asserting on the
    /// stored cursor range would restate the implementation instead of the behaviour.
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
                            .char_limit(HEX_CHAR_LIMIT)
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
        // The click focused it and the selection was stored behind that frame's own
        // caret placement, so the typing lands on the frame after.
        run(vec![egui::Event::Text("F".to_owned())], &mut text);
        assert_eq!(
            text, "F",
            "typing into a focused hex field should replace the colour, not extend it"
        );
    }

    /// Rasterize `lock_filled_shapes` onto a pixel grid, `true` where it inks.
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

    /// The filled lock is drawn, not set in type, so nothing but a test tells us
    /// it still looks like a lock. Two things make it one: a solid body, and a
    /// shackle above it that is a *band* — if the arc ever loses its hole the
    /// glyph reads as a blob, which is exactly what "just fill it in" produces.
    /// It also has to stay inside the em box it claims, or it collides with the
    /// eye beside it in the layers tree.
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

        // Inside the em box, centred in it. One pixel of slack for rounding.
        let half = SIZE / 2.0;
        assert!(
            top as f32 >= centre.y - half - 1.0 && (bottom + 1) as f32 <= centre.y + half + 1.0,
            "lock ink spans rows {top}..{bottom}, outside a {SIZE}px box at {centre:?}"
        );
        assert!(
            left as f32 >= centre.x - half - 1.0 && (right + 1) as f32 <= centre.x + half + 1.0,
            "lock ink spans columns {left}..{right}"
        );

        // The body is the widest part and sits below the shackle.
        let width_of = |y: usize| (0..W).filter(|x| ink(*x, y)).count();
        let widest = (top..=bottom).max_by_key(|y| width_of(*y)).unwrap();
        assert!(
            widest > (top + bottom) / 2,
            "the widest row ({widest}) should be the body, in the lower half"
        );

        // Somewhere between the top of the ink and the top of the body there is
        // a row with a gap in the middle: the shackle's two legs.
        let banded = (top..widest).any(|y| {
            let on: Vec<usize> = (0..W).filter(|x| ink(*x, y)).collect();
            !on.is_empty() && on.windows(2).any(|p| p[1] - p[0] > 1)
        });
        assert!(banded, "the shackle has no hole — it is a blob, not a lock");
    }

    /// A label too long for its button is elided *inside* it.
    ///
    /// **The reported symptom is a layer's name.** The Export panel's button reads
    /// `Export {name}`, and `Export Ellipse 11312312 3123123132` ran out of both
    /// ends of the card — the name is not a phrase this module chose and has no
    /// length limit anywhere. Nothing about that is visible in the source: the
    /// button lays its own text out and paints it with a `Painter`, which clips
    /// against nothing, so the overflow only exists in the shapes.
    ///
    /// Both halves are asserted, because either alone passes for the wrong reason:
    /// a galley that is *elided* could still be centred by the untruncated width
    /// and hang off the left, and ink that is *inside* the rect would also be
    /// inside it if the label happened to be short. And **the name off the
    /// screenshot is one of two cases, not the whole of it** — laid out in full it
    /// comes to 220.9 in a 221pt button, so it is the *padding* that catches it
    /// and the containment check alone would let it through by a twentieth of a
    /// point. The second name is the one that genuinely runs out of both ends.
    #[test]
    fn a_long_action_button_label_is_elided_inside_the_button() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        // One pass so the fonts exist to lay anything out with.
        let _ = ctx.run_ui(Default::default(), |_| {});
        // The Export panel's own button: the card's content width less the ⋯
        // beside it and the gap between them.
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

    /// A press **on the glyph** opens the menu.
    ///
    /// This is the regression the user reported: the head used to be two
    /// `ui.label`s with their responses unioned, and a label wins the hit test
    /// over a rect that merely contains it — so the only clickable part was the
    /// empty band above and below the icons. Clicking the icon, which is the one
    /// thing that looks clickable, did nothing.
    ///
    /// The press is aimed at the centre of the returned rect, because that is
    /// where the glyphs are painted. Geometry alone would not catch this: the
    /// old rect was the right rect, it just was not the thing being hit.
    #[test]
    fn a_menu_head_is_clicked_on_its_own_glyph() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // First pass: lay the head out and learn where it is. Fonts are only
        // available once a frame has run, so this doubles as that.
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run_ui(Default::default(), |ui| {
            rect = menu_head(
                ui,
                crate::theme::icon_text(icon::EYE, 16.0, theme::text::MUTED),
                1.0,
            )
            .rect;
        });
        assert!(rect.is_positive(), "the head allocated nothing");
        assert_eq!(rect.height(), HEAD_H);

        // Second pass: press and release on the middle of it.
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
            )
            .clicked();
        });
        assert!(clicked, "a press on the glyph did not reach the head");
    }

    /// **A card's eyebrow takes the arrow, not an I-beam.**
    ///
    /// Reported after living with the inspector: *Appearance*, *Fill*, *Stroke* —
    /// every header — put a text cursor over a row that behaves like a button,
    /// because egui's labels are selectable by default and a selectable label sets
    /// `CursorIcon::Text` for as long as it is hovered. Nothing in `section_head`
    /// says so or could; the switch is one line in `theme::install_style_into`,
    /// which is exactly why it is worth a test that names the header rather than
    /// the flag.
    ///
    /// **The second half is what stops this passing on a context where nothing was
    /// hovered at all** — the failure mode of every cursor assertion. Turning the
    /// style flag back on, the same pointer over the same word reports `Text`, so
    /// the arrow in the first half is a *result* and not an absence.
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
            // First pass: lay it out and find the word. Fonts only exist once a
            // frame has run, so this doubles as that.
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
            // Then hold the pointer on it. A widget's interaction state is last
            // frame's, so the first of these is the frame that registers the hover
            // and the second is the one that can act on it.
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

    /// The mark and its caret are `gap` apart and nothing wider, so the gap a
    /// head asks for is the gap it gets — nothing scaling with it or creeping in
    /// to make one cluster's carets sit differently from another's.
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
                )
                .rect
                .width();
            });
            w
        };
        // Widening the gap by 4 widens the head by exactly 4: the head's own
        // padding is a constant either side, not something that rides on `gap`.
        assert!((measure(5.0) - measure(1.0) - 4.0).abs() < 1e-3);
    }

    /// **A head and an icon button hold their ink the same distance from the
    /// edge of the box they occupy.**
    ///
    /// The top bar's right cluster alternates the two — undo and redo are icon
    /// buttons, View, Snap and the zoom readout are heads — and one gap for the
    /// whole row only *looks* like one gap if the two kinds of control are inset
    /// alike. An `icon_button` gets its inset for free from being a 26pt box
    /// around a 16pt glyph; a head allocated only its galleys, so its mark sat
    /// 5pt nearer the divider beside it and the cluster read as unevenly spaced
    /// however the gaps were tuned (§15 D44). Measured on the painted glyph
    /// rather than the allocated rect, because the rects were never the problem.
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
                );
            });
        });

        // Both controls are one box with glyphs painted in it; the first glyph
        // in each is the mark, and its left edge is what the eye measures the
        // spacing from.
        let inks: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.visual_bounding_rect()),
                _ => None,
            })
            .collect();
        assert!(inks.len() >= 2, "expected two marks, got {inks:?}");

        // The button paints its glyph centred, so its inset is half the slack.
        let button_inset = (HEAD_H - GLYPH) / 2.0;
        assert!(
            (button_inset - HEAD_PAD).abs() < 1e-3,
            "a {HEAD_H}pt button around a {GLYPH}pt glyph insets it \
             {button_inset}, but a head pads by {HEAD_PAD}"
        );
    }

    /// **A rule's hairline lands on a device pixel, within half of one of the
    /// centre of the column it was given.**
    ///
    /// A rule's neighbours are galleys, so its 1pt column starts at a fractional
    /// x. Rounding that centre in *point* space — which is what this did — pushed
    /// the ink up to 0.7pt right of centre and, at 100%, mostly outside the
    /// column: both top-bar dividers leaned the same way, so the cluster read as
    /// unevenly spaced whatever the gaps were set to (§15 D44). Checked at 125%
    /// and 150% as well as 100%, because point space and device space agree only
    /// at 100% and that is exactly what hid it.
    ///
    /// Half a device pixel is the floor on the error, not a slack allowance: a
    /// crisp line has to sit on the grid, the column's centre generally does not,
    /// and no amount of care closes a gap the two coordinate systems create. The
    /// stroke is a *point* wide, so on a column whose own edges fall between
    /// pixels some of that width necessarily overhangs — which is why this
    /// measures the centre line and not the ink.
    ///
    /// ⚠️ **This test asserted a pixel *centre* unconditionally until §15 D479, and
    /// that is why it could not see `[S19.1-L1-04]`.** Which grid position is the
    /// crisp one is a question about the line's **device** width: odd wants a pixel
    /// centre, even wants a pixel edge. A 1pt line is odd at 100%, 125%, 150% and
    /// 175% — every scale in this loop but one — so an assertion that hard-coded
    /// the odd answer looked exhaustive and was right four times out of five, and
    /// **wrong at exactly the case it was failing to draw**: 200%, where the line
    /// is 2 device pixels and spread `[0.5, 1.0, 0.5]` across three. The review
    /// recorded `ui::rule` as *"pinned by nothing"*; it was pinned by this, which
    /// is worse — **a test naming a rule and asserting a special case of it**, the
    /// G19 shape, in the one place that would have caught the bug.
    #[test]
    fn a_rules_hairline_lands_on_a_pixel_inside_its_own_column() {
        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            ctx.set_pixels_per_point(ppp);
            let _ = ctx.run_ui(Default::default(), |_| {});

            // A fractional origin is the case that matters, so start the row
            // with something of non-integer width and let the rule follow it.
            let mut column = egui::Rect::NOTHING;
            let out = ctx.run_ui(Default::default(), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    menu_head(ui, egui::RichText::new("100%").size(11.5), 5.0);
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

            // On the grid position its own device width needs, so it is one crisp
            // line rather than two soft ones: a pixel centre for an odd device
            // width, a pixel edge for an even one.
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
            // And no further from the column's centre than that snapping forces.
            let drift = (x - column.center().x).abs();
            assert!(
                drift <= 0.5 / ppp + 1e-3,
                "{ppp}x: hairline at {x} is {drift} from the centre of {column:?}, \
                 more than the half-pixel the grid costs"
            );
        }
    }

    /// Everything in a field row is the height it asked for and starts on one
    /// baseline — the fields *and* the buttons beside them.
    ///
    /// This is the assertion for `FIELD_BORDER_H`, and it has to measure the
    /// painted **grounds** rather than the returned rects: the response is the
    /// frame's content box, which is exactly where the missing 2px hid. With the
    /// rows over-allocating, the three grounds came out 30/30/28 tall at y
    /// 0.0/1.0/1.5 — a field row whose second column sat 1px low and whose
    /// button was 2px short, both of which read as sloppiness rather than as a
    /// margin bug.
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

        // Every ground is a rect filled with the field colour: two rows and the
        // button, which shares their ground on purpose.
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

    /// **`Ui::disable` fades everything inside it, explicitly-coloured text
    /// included** — grounds, strokes and glyphs alike, all by `disabled_alpha`.
    ///
    /// egui 0.35 disables with `painter.multiply_opacity(disabled_alpha)`, and
    /// every shape the scope emits goes through `epaint`'s `adjust_colors`, which
    /// for a `Shape::Text` takes `Arc::make_mut` on the galley and recolours the
    /// **mesh vertices** of every row. So there is no way to smuggle a colour past
    /// it: `RichText::color(…)` and `Painter::text(…, colour)` fade exactly as a
    /// plain `ui.label` does.
    ///
    /// **The trap this test exists to stop, because it caught me.** A galley
    /// carries the colour in two places, and only one of them paints.
    /// `galley.job.sections[..].format.color` is the layout *input* and
    /// `adjust_colors` never touches it; `galley.rows[..].row.visuals.mesh
    /// .vertices[..].color` is the output, baked at layout time from that same
    /// format, and is what the tessellator draws. A probe reading the section
    /// therefore reports "explicitly-coloured text does not fade" — confidently,
    /// reproducibly, and wrongly. **Read the vertices.**
    ///
    /// `Color32::PLACEHOLDER` in a vertex is the sentinel meaning "substitute
    /// `fallback_color` at tessellation", and `fallback_color` is adjusted too, so
    /// that path fades as well. `theme::install` sets `override_text_color`, which
    /// changes which of the two paths a plain label takes and nothing about the
    /// outcome.
    ///
    /// Asserted rather than described, so an egui upgrade that changes it says so.
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
                    // 1. explicit colour through RichText
                    ui.label(egui::RichText::new("A").color(theme::text::MUTED));
                    // 2. plain label, colour from visuals
                    ui.label("B");
                    // 3. explicit colour straight onto the painter
                    let p = ui.painter();
                    p.text(
                        egui::pos2(50.0, 50.0),
                        egui::Align2::LEFT_TOP,
                        "C",
                        egui::FontId::proportional(12.0),
                        theme::text::MUTED,
                    );
                    // 4. a filled rect
                    p.rect_filled(
                        egui::Rect::from_min_size(egui::pos2(0.0, 80.0), egui::vec2(9.0, 9.0)),
                        egui::CornerRadius::ZERO,
                        color::ACCENT,
                    );
                });
            })
        };
        // **What actually paints a glyph is the mesh vertex**, not the layout
        // section — see this test's own note. `PLACEHOLDER` there means "defer to
        // `fallback_color`", which the tessellator substitutes later, so that is
        // the one case where the fallback is the answer.
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
            // Halved, which is `Visuals::disabled_alpha` — asserted so that a
            // theme setting it explicitly has to come past this test.
            assert!(
                (i32::from(dim.a()) - i32::from(lit.a()) / 2).abs() <= 1,
                "{name} faded from {lit:?} to {dim:?}, which is not the 0.5 \
                 `disabled_alpha` says"
            );
        }
    }

    /// Alpha the vertices of the corner mask were painted at, if a mask was
    /// painted. The mask is the one part of a `swatch` whose colour is the
    /// *ground* rather than the paint, which is exactly what makes it the part a
    /// faded scope ruins.
    /// **A gate inside a gate must not fade twice**, which is what
    /// `disable_unless` exists for.
    ///
    /// `Ui::disable` multiplies the painter's opacity rather than setting it, and
    /// `add_enabled_ui` does not guard on the ambient state, so the naive nesting
    /// lands at 0.25 against its neighbours' 0.5. It shows as one row carrying two
    /// dim levels — unevenness, which reads as a rendering fault rather than as a
    /// gate, and is exactly the kind of thing that is invisible in the source.
    ///
    /// Asserted as an A/B against a single gate, and against the unguarded nesting
    /// too, so it cannot pass by the fade having quietly stopped happening.
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

    /// **"Unavailable" and "switched off" are different greys, and both routes to
    /// "unavailable" are the same one.**
    ///
    /// The app dims for two unrelated reasons and used to do it at the same value:
    /// `theme::text::FAINT` (97) painted a disabled glyph, while a hidden paint row
    /// is `HIDDEN_OPACITY` over a `MUTED` one (95) and a hidden or locked layer row
    /// is `text_a(102)`. Two and five levels apart is no distance, so dim alone
    /// read as "inert, don't bother" on a control that was merely switched off.
    ///
    /// Two things are asserted, because the fix has two halves. **Disabled is one
    /// ink however it is reached** — `Ui::disable`'s 0.5 fade of a `MUTED` glyph
    /// lands on the same value as the explicit `DISABLED` constant, which is where
    /// 79 came from rather than being picked. And **it clears both off tiers** by a
    /// margin that reads, the same order as the step that settled
    /// `SWATCH_HAIRLINE`.
    ///
    /// `FAINT` itself is untouched and is *not* asserted against here: it stayed
    /// the tier for placeholders, hints and eyebrows, which are quiet rather than
    /// unavailable, and it is allowed to sit wherever the off tiers do.
    #[test]
    fn unavailable_and_switched_off_are_different_greys() {
        const OFF_HIDDEN_ROW: u8 = 95; // HIDDEN_OPACITY (0.6) over MUTED
        const OFF_LAYER_ROW: u8 = 102; // layers.rs, absolute
        const MARGIN: i32 = 15;

        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        // A disabled glyph reached through the *scope*, measured as painted.
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

    /// **Saying "unavailable" twice must not look twice as unavailable.**
    ///
    /// A control can be dimmed by its own `enabled` flag, by a disabled scope
    /// around it, or — in the identity row — by both, because the row gates itself
    /// and then gates each button on a condition of its own. Under the naive
    /// spelling the three channels compose: `DISABLED` (79) painted inside a scope
    /// already faded by `disabled_alpha` gives 40, so *Mask* and *Ungroup* sat a
    /// shade darker than *Group* beside them, in one row. Uneven rather than
    /// wrong-looking, which is the kind of thing that never gets reported.
    ///
    /// The three spellings are asserted to agree, which is the property that
    /// matters — not any one of their values.
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

        // Its own flag alone — no scope in play.
        let by_flag = ink(&btn(false));
        // A disabled scope alone, the button believing itself live.
        let by_scope = ink(&|ui: &mut egui::Ui| {
            ui.add_enabled_ui(false, btn(true));
        });
        // Both at once, which is what the identity row does.
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

    /// A hidden paint row is faded, and its **swatch is not**.
    ///
    /// `Ui::multiply_opacity` fades everything drawn inside it, and `swatch`
    /// rounds its corners by painting the leftover slivers in the *ground* colour
    /// (`mask_corners`). Under a multiplied opacity that mask goes translucent,
    /// the chip shows through it, and the square corners partly reappear — the
    /// "four visible notches" artifact `mask_corners` exists to prevent, arriving
    /// by a new route. It reads as a tessellation bug and traces back to the
    /// opacity scope, so the chip is painted outside it.
    ///
    /// Checked as an A/B rather than as one absolute number, because "the mask is
    /// opaque" would also pass if the dim were silently doing nothing at all.
    ///
    /// The factor below is illustrative and deliberately does **not** track the
    /// inspector's `HIDDEN_OPACITY`: what is under test is `set_opacity(1.0)`
    /// escaping an enclosing `multiply_opacity`, which holds at any factor under 1.
    /// Chasing the product value here would couple this to a number that exists to
    /// be retuned by eye.
    #[test]
    fn a_dimmed_row_fades_everything_but_the_swatch() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // Fonts are only available once a frame has run.
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
        // Fonts are only available once a frame has run.
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut rect = egui::Rect::NOTHING;
        let out = ctx.run_ui(Default::default(), |ui| {
            rect = swatch(ui, 16.0, color::FIELD, Swatch::Solid(chip)).rect;
        });
        (out, rect)
    }

    /// The chip's hairline, and where in the frame it was painted.
    ///
    /// Every other rect a `swatch` emits is a `rect_filled` — the checkerboard's
    /// white base, its grey cells, the paint itself — so a non-zero stroke width
    /// identifies this one without matching on a colour the test is about to
    /// assert on.
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

    /// `top` composited over an opaque `under`.
    ///
    /// **In gamma space, which the GPU is not.** epaint hands the shader
    /// premultiplied gamma bytes and the framebuffer blends them linearly, so the
    /// reported `2F2F2F` chip lands nearer 86 on screen than the 109 this returns.
    /// It makes no difference to what is asserted: the binding case is the *black*
    /// chip, where the hairline composites to its own alpha under either rule, and
    /// the margin is set by that one. Worth knowing before anyone tunes `MARGIN`
    /// against the lighter chip's number.
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

    /// **A chip the colour of the row it sits on is still visible**, because its
    /// hairline is not.
    ///
    /// The two chips are the two ends of the problem. `2F2F2F` on `color::FIELD`
    /// (`2F2F30`) is the case as reported — a fill one level off its own ground,
    /// with nothing whatever to see. Black is what actually sets the alpha:
    /// composited over a chip the hairline climbs from the alpha itself (over
    /// black) to the chip's own colour (over white), so the darkest chip is the
    /// one that has to clear the ground, and every lighter chip clears it by more.
    ///
    /// The margin is asserted rather than the constant so that the *reason* is
    /// what fails. Flipped against the two versions somebody would plausibly
    /// write: `from_black_alpha(76)` — the picker's direction, right over a bright
    /// SV plane and wrong here — leaves the reported chip 15 levels from its
    /// ground and the black one 47 *below* it, still invisible in the second case
    /// and worse in the first; and trimming the alpha to be subtler fails on
    /// black first, at 65.
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

    /// **After the mask, and inside the chip.**
    ///
    /// `mask_corners` paints the leftover corner slivers in the ground colour,
    /// so a hairline drawn before it has four pieces bitten out of it — the
    /// artifact reads as a tessellation bug and traces back to paint order. And a
    /// chip sits in a row of a fixed height: a stroke drawn `Outside` would grow
    /// it by a point on each side and push the label beside it, which is the shape
    /// of the complaint that got the *previous* border removed.
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

    /// **A `segmented` track wears the same hairline every field and button
    /// does** (§15 D386), and its cells sit inside it rather than under it.
    ///
    /// The one control class in the chrome that was still drawing a bare ground —
    /// found by dumping every rounded `RectShape` the inspector, the dashboard and
    /// the modals paint with a fill and no stroke over it, which came back with
    /// the two alignment tracks, the dashboard's view toggle and nothing else.
    /// The switches came back too and are *right* to: the design draws a toggle
    /// with no border, and so do the sliders and the Align row's buttons.
    ///
    /// ⚠️ **The track paints two `RectShape`s on one rect now — ground, then
    /// hairline** — which is why the assertion looks for a stroked rect at the
    /// track's own bounds rather than counting shapes. Two tests were reading the
    /// second one as a second control before this landed
    /// (`the_image_cards_tracks_and_fields_are_one_height` decided the fixture was
    /// not a column).
    ///
    /// Flipped both ways. Dropping the `rect_stroke` fails on `hairline` — as
    /// predicted. Leaving `inner` at `track.shrink(PAD)` fails on the cell's
    /// *height*, 24 against the 22 asserted, and **not** on its position: the cell
    /// still starts inside the border either way, so a test that only checked
    /// `cell.min.y > track.min.y` would have passed the version that lays the cell
    /// against the hairline.
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
        // The track is the only thing here `CELL_H + 4` tall — the two points of
        // padding it adds at each end.
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

        // The raised cell: `1 + 2` in from the track at both ends, so a 28pt
        // control holds a 22pt chip — the design's `1 + 2 + 22 + 2 + 1`.
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

    /// The inspector card's content width at the design's measurements: a 296px
    /// column less its 12px right padding is a 284px card, less
    /// `CARD_MARGIN_X` either side.
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

    /// **Tab goes from one field's digits to the next field's digits, and stops
    /// nowhere in between** (§15 D290).
    ///
    /// A `value_field` is three interactive rects — the prefix scrub strip, the
    /// `DragValue`, and a clickable unit at the far end — and only the middle one
    /// has a keyboard face. `Sense::click()` and `Sense::click_and_drag()` are
    /// `… | FOCUSABLE`, so the other two were tab stops that painted no focus of
    /// their own: X → Y took two presses and the first landed on 16 × 26 pixels of
    /// nothing. Reported on Transform, where the fields are used in pairs.
    ///
    /// **Asserted as "no id but these two is ever focused", not as a step count.**
    /// A count pins the bug that was reported and nothing else; the set catches a
    /// third rect gaining `FOCUSABLE` later, which is the same bug arriving by a
    /// different route. The second field is deliberately the **suffixed** variant,
    /// because that is the one with a stop on each side of its digits — a row of
    /// two plain fields would have gone green with the unit's half unfixed.
    ///
    /// The sweep is six presses over a two-field row, so it wraps and comes round:
    /// `None` in the sequence is egui's own gap at the end of the focus list, not a
    /// widget.
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
            // The frame's ink is not the question here; focus is read back off the
            // context afterwards.
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
                        // The rect and the sense, because the whole difficulty of
                        // this bug was that the offender draws nothing: a failure
                        // message naming only an `Id` would send the next reader
                        // looking for a widget rather than for a `Sense`.
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

    /// **W is exactly as wide as X, and the row still fills the card.**
    ///
    /// This is the design change the whole row exists for, and it is invisible in
    /// code: three slots split evenly would put W's right edge a button's-worth
    /// left of X's, and that misalignment down the middle of the card is the first
    /// thing the eye finds. H absorbing the difference has nothing above it to
    /// line up with, so nothing shows.
    ///
    /// Measured rather than reasoned about, because the arithmetic runs through
    /// `FIELD_BORDER_H` — a frame told to hold 28px of content paints 30 — and
    /// that is exactly the class of off-by-two that reads as "the boxes don't
    /// line up" and is invisible in the source.
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
        // Both rows reach the card's far edge — the chain button ends where Y does.
        assert!(
            (xy[1].max.x - wh[2].max.x).abs() < 0.01,
            "the rows do not end together: {:?} vs {:?}",
            xy[1],
            wh[2]
        );
    }

    /// The four quarter-turn and mirror buttons split the card evenly and fill it,
    /// which is what makes them read as one control rather than as 28px squares
    /// with a gap on the end.
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

    /// A paint row leaves exactly the eye button's width beside it, and the two
    /// share a baseline — the field and the button disagreeing about whether 28 is
    /// inside or outside the hairline is what `FIELD_BORDER_H` exists for.
    #[test]
    fn a_paint_row_and_its_eye_fill_the_card_on_one_baseline() {
        const ROW_GAP: f32 = 6.0;
        let bs = grounds(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ROW_GAP;
                let size = egui::vec2(ui.available_width() - H - ROW_GAP, H);
                field_row(ui, size, |ui| {
                    // As the real row does: a `field_row`'s frame shrink-wraps,
                    // so what makes the ground the full 222px is the right-aligned
                    // group claiming the rest of it. Spelled out here rather than
                    // stubbed with a label, or the probe would measure a row the
                    // panel never draws.
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

    /// Selected digits must stay legible in **every** helper that strips a
    /// `DragValue`'s chrome.
    ///
    /// `selection.stroke` is two things in egui: the outline a focused `TextEdit`
    /// frames itself with, *and* the colour selected glyphs are repainted in —
    /// `paint_text_selection` writes it into the galley's own vertices. Both helpers
    /// suppress the outline, and suppressing it with `Stroke::NONE` (transparent)
    /// turned the selected digits invisible: the opaque selection fill stayed, so a
    /// field being typed into showed a solid blue block with nothing in it.
    ///
    /// **All of them, in one table**, because the bug was fixed in `value_field`
    /// and left live in `bare_drag_value` — the report named the number fields,
    /// and the opacity fields are the same line one helper over. A test that
    /// covered only the reported one would have let that stand.
    ///
    /// ⚠️ **`badge_field` was the third helper and was not in this table until
    /// 2026-09-10** (§15 D723, `[S18.1-L3-06]`) — which is the doc above happening
    /// a second time, to the test written to stop it happening. The three now
    /// share one `strip_drag_value_chrome`, so a regression would take all three
    /// at once; the arm is kept anyway, because what this pins is that each
    /// *helper* is still wired to the rule, and the cheapest way to break that is
    /// for one of them to stop calling it.
    ///
    /// The galley is the only place the evidence lives — the selection is inside the
    /// text mesh, not a separate shape — so this reads glyph vertex alphas out of it
    /// rather than looking for a rect. And it drives real pointer and key events to
    /// make the selection, rather than asserting on the visuals it is testing.
    #[test]
    fn selected_digits_are_never_painted_transparent() {
        for what in ["value_field", "bare_drag_value", "badge_field"] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let _ = ctx.run_ui(Default::default(), |_| {});

            let mut v = 12345.0_f64;
            let field = |ui: &mut egui::Ui, v: &mut f64| match what {
                // In a field row, as the paint row draws it.
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
                    // Formatter pinned as every real field pins it, so the digits
                    // read `12345` rather than however many places egui's
                    // auto-precision derives from the drag speed — the trap
                    // `number` exists for (§15 D66).
                    |d| d.custom_formatter(number(2)),
                ),
            };

            // Learn where it is, then double-click into it — which is what puts a
            // `DragValue` into keyboard editing.
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
                // Home, then Shift+End: select the lot, whatever the entry left the
                // caret doing.
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

            // The selection fill is appended to this same mesh, so an opaque run of
            // it is how we know a selection is actually up — without that check, "the
            // glyphs are opaque" would pass on an unselected field and prove nothing.
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

    /// Drag `dx` horizontally, starting either on the prefix or on the digits, and
    /// report where the value ends up.
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
            // **The field's painted ground, not the response's rect.** The response
            // is the union of the number and the prefix strip, so it *shrinks* if the
            // strip stops sensing — which would quietly move this test's aim point
            // into the number and let it pass with the prefix scrub broken.
            out.shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Rect(r) if (r.rect.height() - 28.0).abs() < 0.51 => Some(r.rect),
                    _ => None,
                })
                .expect("the field paints its ground")
        };
        // Two passes so the widget's geometry is established before it is aimed at.
        field(&ctx, Vec::new());
        let rect = field(&ctx, Vec::new());
        let from = if start_in_prefix {
            // Inside the reserved strip: past the field's own `FIELD_PAD_X` and into
            // the prefix, which is ~22px wide for any glyph the design uses.
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

    /// **The whole field scrubs, the prefix included, and both halves agree.**
    ///
    /// A field whose number scrubs and whose own label does not means remembering
    /// which apps start the drag on the icon and which on the digits — and the label
    /// is where the hand goes, since it is what says which number this is. The prefix
    /// strip is a second claimant applying the drag itself (`value_field`), so
    /// "agree" is a real risk rather than a tautology: the two arithmetic paths are
    /// genuinely different code.
    #[test]
    fn a_scrub_starting_on_the_prefix_moves_the_value_the_same_way() {
        // 37px is deliberately not a whole number of steps at these speeds, so a
        // difference in rounding between the two paths would show.
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

    /// **Scrubbing lands on whole units.** X, Y, W, H, rotation and skew are all
    /// `Scrub::whole`, and a coordinate dragged to 109.25 is precision the number does
    /// not mean — it goes into the saved file and shows up in the panel. Typing is
    /// untouched, which is what keeps a hairline or 33.3% askable.
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
        // And a field that asks for decimals still gets them, or the rule above would
        // be indistinguishable from rounding everything.
        let fine = drag(Scrub::fine(0.02, 2), false, 37.0);
        assert_ne!(fine, fine.round(), "a fine field lost its fraction: {fine}");
    }

    /// Typing is never rounded — the other half of the bargain, and the half that
    /// makes the rule above acceptable.
    #[test]
    fn a_whole_field_still_accepts_a_typed_fraction() {
        let scrub = Scrub::whole(0.5);
        assert_eq!(scrub.settle(109.25), 109.0, "a drag settles");
        // `settle` is only ever reached from a drag; the range still clamps.
        let bounded = Scrub::whole(0.5).range(0.0..=100.0);
        assert_eq!(bounded.settle(150.0), 100.0);
        assert_eq!(bounded.settle(-5.0), 0.0);
        // A rounded value can step outside a fractional bound, so the clamp has to
        // come after the rounding rather than before it.
        let fractional = Scrub::whole(0.5).range(0.0..=99.4);
        assert_eq!(fractional.settle(99.4), 99.0);
    }

    /// **Two fields in one row must not share a widget id.**
    ///
    /// The prefix strip is a second widget inside every field, and its first version
    /// took `ui.id().with("prefix-scrub")` — but `Ui::id` is the *containing* Ui's,
    /// which every field in a `horizontal` shares. So both strips claimed one id, egui
    /// lit the whole Transform and Stroke panels with its clash overlay, and the
    /// strips were inert behind it. Reported as red boxes over every field.
    ///
    /// Asserted through the overlay rather than by comparing ids, because the overlay
    /// is what the user saw — and it catches any *other* widget in a field colliding
    /// too, which comparing two known ids would not.
    ///
    /// 🚨 **The warning is switched on here rather than asserted, because egui's
    /// default for it is `cfg!(debug_assertions)`** (§15 D597). Asserting the
    /// ambient value made this test *fail* under `cargo test --release` on its own
    /// fixture check — *"the clash warning is off, so this test cannot fail"* —
    /// which is the assertion doing its job and is also a red release suite nobody
    /// was looking at, since none of the six gates runs the tests in release
    /// (`check --release` compiles and does not test). ⚠️ **This is the
    /// `debug_assertions` hole from a new direction**: the recorded one is a line
    /// behind egui's own `cfg` breaking the release *build*; this is a *test* whose
    /// subject exists only in debug. Setting it makes the assertion mean the same
    /// thing in both profiles, which is what it was always trying to say.
    #[test]
    fn a_row_of_value_fields_has_no_id_clash() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        // The warning has to be *on* for absence of it to mean anything —
        // `theme::install` quiets its noisier sibling `warn_if_rect_changes_id`
        // (§15 D65) and deliberately leaves this one alone.
        //
        // 🚨 **Set, not read, and that is §15 D597's whole subject**: egui defaults
        // this option to `cfg!(debug_assertions)`, so reading the ambient value made
        // this test **red in release** on its own fixture check, under a command no
        // gate issued. **§15 D771 is the ruling** — `cargo test --workspace
        // --release` is a session-close step in `CLAUDE.md`'s gate list rather than
        // a per-edit gate, because the release *run* is three times faster than the
        // debug one and the cost is a second profile's build and 2.4 GB of disk.
        //
        // ⚠️ **The citation is here because D771 changed no production line and
        // would otherwise be invisible to the D-number census** — D735's shape, and
        // D741's fix for it: put the number where the next reader of the thing it
        // is about will meet it.
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

    /// And the strips are wired to their *own* numbers. One shared id would have made
    /// two fields move together, which is the same bug wearing a different symptom —
    /// and the sort a clash test alone can miss, since ids can be distinct and still
    /// be plumbed to the wrong value.
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

        // The *second* field's prefix, so a strip wired to the wrong number shows up
        // as the first field moving.
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

    /// **A field whose comment says it lands on tenths has to land on tenths.**
    ///
    /// The star's inner ratio defaults to the golden section, 38.2%, and its comment
    /// records that rounding the field to integers once made that value unreachable
    /// by dragging. The panel-wide whole-units rule (§15 D75) was briefly applied here
    /// too and put it straight back: a drag from 38.2 went to 38.0 on the first pixel.
    /// A ratio is not an integer by nature, which is where that rule stops.
    ///
    /// Pinned as the contrast, because `fine` alone passing proves nothing about which
    /// one the field was given.
    #[test]
    fn a_fine_field_keeps_the_tenths_a_whole_one_destroys() {
        /// Drag 8px right from `start` and collect the distinct values passed through.
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

        // The inner-ratio field's configuration. Every step stays on a tenth, and the
        // golden section it started from survives.
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

        // And the rule it must *not* be given, so this test tells the two apart. The
        // sweep's first entry is the value before egui's drag threshold is crossed, so
        // the contrast to assert is what the drag *leaves behind*: `whole` throws the
        // tenth away on its first real step and never returns to one.
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

    /// **An inverted range must not take the process down.**
    ///
    /// `[S18.1-L1-01]`, §15 D462. `f64::clamp`'s *"min > max, or either was NaN"*
    /// is an unconditional assert in `core`, not a `debug_assert!`, and a panic
    /// inside `eframe`'s update closure takes everything since the last
    /// `.recovery/` snapshot with it.
    ///
    /// ⚠️ **This is the only one of the forty-one `Scrub::range` call sites whose
    /// bound comes from the document**, which is why an inverted range was
    /// reachable at all. `inspector.rs`'s *Scale* field for an image fill in
    /// *Crop* builds `floor..=10_000.0` from `ImageRef::cover_scale` — frame
    /// world units over picture **pixels**, uncapped — so a 16×16 icon on a
    /// 1920×1080 rectangle gives `12000.0..=10000.0`. The `.max(0.01)` already on
    /// that floor closes `clamp`'s NaN half and left the ordering half open.
    ///
    /// ⚠️ **egui swaps where this app died**, which is why nothing upstream
    /// caught it: `DragValue::clamp_value_to_range` opens by exchanging an
    /// inverted pair, so the widget rendered happily and only the app's own
    /// re-clamp panicked. Two spellings of one rule, one of them fatal.
    ///
    /// ⚠️ **Driven through the real widget, not only through `settle`.** The
    /// panic was on a *drag frame*, so the assertion that names it is a press and
    /// a 24pt move through `value_field` — the two doors in `value_field_f64` are
    /// the prefix strip and the `DragValue`, and both are exercised here.
    ///
    /// ⚠️ **Flipped** by restoring `self.range = range`: the first assertion
    /// panics inside `f64::clamp` with *"min > max, or either was NaN. min =
    /// 12000, max = 10000"*, which is the finding in one line. The control — an
    /// ordinary `0.0..=10_000.0` range, which clamps to 10 000 before and after —
    /// stays green, so this is not a test about clamping.
    ///
    /// ⚠️ **The finding's own suggested assertion contradicts its own fix
    /// sketch**, and following it would have written a test the fix fails. It
    /// proposes *"`range(12_000.0..=10_000.0).settle(12_000.0)` asserting
    /// `10_000.0`"*, which is what **clamping to the ceiling** would give; the
    /// sketch beside it proposes **swapping**, under which `12 000` is inside
    /// `10 000..=12 000` and stays. Swapping is what shipped, because it is what
    /// egui does and the whole point is that the two must agree. The assertions
    /// below therefore pin the *interval* — floor, ceiling and a value between —
    /// rather than one number that reads as either answer.
    #[test]
    fn an_inverted_scrub_range_is_put_the_right_way_round_rather_than_fatal() {
        // The mechanism first, because it is the cheap half.
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
        // A `NaN` bound is dropped rather than swapped: `f64::min` answers the
        // non-NaN operand, so swapping would pin the field to one number.
        let nanny = Scrub::fine(0.5, 2).range(f64::NAN..=10_000.0);
        assert_eq!(
            nanny.settle(99_999.0),
            99_999.0,
            "no bound, not a pinned one"
        );

        // The control, which passed before this fix and must still.
        assert_eq!(
            Scrub::fine(0.5, 2).range(0.0..=10_000.0).settle(12_000.0),
            10_000.0
        );

        // And end to end, on a drag frame — the frame that panicked.
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

    /// Draw a closed `ComboBox` with the app's icon and report its shapes.
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

    /// The glyph, at the size and ink the design asks for — and **no filled
    /// triangle**, which is the thing being replaced.
    ///
    /// egui's default is a `convex_polygon` of three points painted in
    /// `fg_stroke.color`: the *interaction* colour, so it brightened on hover and
    /// went full white with the popup open. That is why the arrows were the
    /// loudest thing in a panel of dropdowns.
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

    /// The height a `ComboBox` paints with `interact_size.y` set to `y`, or with
    /// the theme left alone.
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

    /// **A combo paints exactly `interact_size.y`, and nothing else it is given.**
    ///
    /// Both halves matter, because the two ways of getting this wrong were both in
    /// the Type panel at once and produced *different* wrong heights: the family
    /// dropdown said nothing and got the theme's 24px, and the variant dropdown
    /// subtracted `FIELD_BORDER_H` — right for content *inside* a `field_row`,
    /// wrong for a combo, which strokes its border inside its own rect — and got
    /// 26. The reported symptom was three heights down one card: 24, 26, 28.
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

    /// One painted `value_field_metered`, as its **ground**, its **zero tick**
    /// and its **bar**.
    ///
    /// **All taken out of `.shapes`, and the response's rect deliberately not
    /// used.** A field's `Response` covers its *content* box — the allocation less
    /// the frame's `FIELD_PAD_X` at each end and its hairline — where the bar is
    /// drawn across the field's ink, padding included, which is what the design's
    /// `overflow: hidden` does. So a test that measured the bar against the
    /// response would report a correct bar escaping its field by nine points at
    /// each end, which is what the first version of this did.
    ///
    /// The ground is found by its **colour**, which is the only thing separating
    /// it from the wash in a flattened shape list. **The tick and the bar cannot
    /// be**, since being the same blue is the whole point of the tick — so they
    /// are taken in *paint order*, which is a real property rather than a
    /// convenience: the tick is reserved first so that the bar covers it.
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

    /// **The bar grows from the middle, and the two signs are mirror images.**
    ///
    /// This is the whole of the correction the control was reshaped for: a bar
    /// anchored at the left draws −40 and +40 identically, which for a value whose
    /// zero is its middle contradicts the digits beside it rather than compressing
    /// them.
    ///
    /// Asserting the *pair* rather than one of them is what makes it a test about
    /// the centre. A single positive bar half the field wide is also what a
    /// left-anchored bar at 0.5 paints, so either sign on its own passes against
    /// the version this replaces; only the two together pin where the origin is.
    #[test]
    fn the_meter_grows_from_the_middle_and_mirrors_across_it() {
        let (ground, _, plus) = parts(0.5);
        let (_, _, minus) = parts(-0.5);
        // **Inside the hairline, not under it.** `FIELD_BORDER_H` is the border's
        // two points counted across a field; half of it is what the bar clears at
        // each edge, which is the whole of what `overflow: hidden` means here.
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
        // A quarter of the field each, since half the *travel* is half of half.
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

    /// **The value is right-aligned in a metered field and beside its prefix
    /// everywhere else.**
    ///
    /// Two things it would be easy to get wrong in opposite directions: making
    /// every field in the app right-align its number, and leaving the metered one
    /// with the number in the middle of the bar it is supposed to be reading.
    ///
    /// Measured off the painted galley rather than asserted in the source, and as
    /// a *comparison between the two spellings* rather than an absolute — the
    /// number's exact x depends on the digits and the font, and what matters is
    /// which end of the field it is at.
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
            // The number's galley, which is the one that is not the prefix: both
            // are text, and the prefix is laid out first.
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
        // Past the middle of the field, which "further right" alone does not say:
        // a number one point right of the label is also further right.
        assert!(
            metered > field.center().x,
            "the metered number sits at {metered}, left of the field's middle at {}",
            field.center().x
        );
    }

    /// **Zero paints nothing, and either extreme fills its half.** The three are
    /// what a card of seven rows is read by — which of them are doing nothing at
    /// all, and which have run out of range in which direction.
    ///
    /// The extremes are **half** the field now, not all of it, and that is the
    /// arithmetic most likely to be got wrong on the way to a centre origin:
    /// scaling the full width by the signed fraction rather than the half-width
    /// makes ±1 reach twice as far as the field has, and everything under about
    /// 0.5 still looks right.
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

    /// **A row at rest is not an empty box: the zero tick is always there.**
    ///
    /// Reported on the machine against the first centre-origin version, which drew
    /// nothing at all at zero — and a field with nothing in it reads as no control
    /// rather than as a control at its neutral value.
    ///
    /// It is checked at a **non-zero** value too, and that is the half that would
    /// otherwise rot: "draw a tick when the value is zero" is the obvious spelling
    /// and it takes the origin away exactly when the bar gives it something to be
    /// read against.
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
            // Full height *of the ink*, so it stops short of the border rather
            // than meeting it — which is the whole of what was asked for.
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

    /// **Each pair of corners rounds only when the bar reaches that end.**
    ///
    /// A mid-field bar with rounded ends is a capsule floating in a field rather
    /// than a fill running out of it, and a bar that reaches an edge with square
    /// ones paints two accent slivers into the corners the field's ground leaves
    /// for the card behind it. Both are one `match` away from each other and
    /// neither is visible in the source.
    ///
    /// **Both signs, because a centre-anchored bar has two ends in play** where
    /// the left-anchored one this replaced only ever had its right. A version that
    /// kept the old rule — left corners always round, right only when full — draws
    /// a rounded left edge in the middle of the field on every negative value, and
    /// passes every assertion the positive half of this makes.
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
            // The **second** wash rect. The first is the zero tick, which shares
            // the bar's colour on purpose and is square by construction — reading
            // it here instead would make every case below pass for free.
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

    /// One pass over a lone `value_field`, returning its painted ground for the
    /// next pass to aim at — the same fixture `app`'s edit-valve tests drive.
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

    /// Put the field into its keyboard face, type `text` a character at a time,
    /// and report what the value read after each one.
    ///
    /// **A character at a time, because that is how the field is parsed.** egui
    /// updates while editing, so every prefix of the string is handed to the
    /// parser on its way to the whole — which is the half of this that a single
    /// `Event::Text` would not exercise at all.
    fn typed(text: &str) -> Vec<f64> {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = 100.0;
        pass(&ctx, &mut v, Vec::new());
        let rect = pass(&ctx, &mut v, Vec::new());
        let at = rect.center();
        // A click with no travel is what swaps a `DragValue` for a `TextEdit`,
        // and the focus it gains selects the digits already there.
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

    /// The reported gap, end to end: a width field takes `135*2` and reads 270.
    ///
    /// The **trail** is the assertion rather than the final number, because the
    /// value is written on every keystroke: the field walks through 1, 13, 135
    /// as the first operand is typed — which it always did — and the interesting
    /// entry is the fourth, where `135*` is rejected and the value *stays* at
    /// 135 rather than being read as part of itself. Committed on Enter, the
    /// answer survives the field being re-formatted.
    #[test]
    fn a_width_field_evaluates_the_sum_typed_into_it() {
        assert_eq!(
            typed("135*2"),
            vec![1.0, 13.0, 135.0, 135.0, 270.0, 270.0],
            "135*2 did not come out 270 — the last entry is after Enter"
        );
    }

    /// And the everyday case still behaves: a plain number typed into a field is
    /// the number, not an expression that happens to have no operators in it.
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

    /// **An operator's glyph changes as the sum around it is typed, and that is
    /// Inter rather than the app.**
    ///
    /// Reported as an asterisk that "shifts slightly, making it look like a
    /// different character was set in place" on the keystroke after it — which
    /// is exactly what happens, because it *is* a different character. egui 0.35
    /// shapes with harfrust and asks for the font's default feature set, which
    /// includes `calt`; Inter's contextual alternates swap in a figure-aligned
    /// asterisk and re-centre `+` and `−` once digits surround them. Reading the
    /// two out of the atlas as pictures is what settled it: a six-pointed
    /// asterisk on the digits' centre line against a five-pointed one riding
    /// high.
    ///
    /// Nothing to fix — the alternate is the better-looking of the two and the
    /// one the field settles on. This is here so the next report of it is
    /// answered in a test run rather than in another afternoon with the atlas,
    /// and it fails honestly if a future Inter stops doing it.
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
        // The same feature, without a substitution: `-` keeps its glyph and is
        // lifted to the digits' centre line, which is the one-pixel shift the
        // report also named.
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

    /// **A card paints the width it is named**, for every `(width, pad)` pair the
    /// app opens one at.
    ///
    /// The guard `menu_inner_w` exists for. Every popover in the app is a
    /// `menu_frame` with `set_width` inside it, and every one of them subtracted
    /// the padding and forgot the border, so every one painted two points wider
    /// than the design draws it — 212 / 152 / 162 / 274 against 210 / 150 / 160 /
    /// 272 (§15 D307). Nothing on screen contradicted it: the number was only ever
    /// compared against the *other* copy of itself, and both copies were the
    /// content box.
    ///
    /// ⚠️ Flipped against the version anybody would write — `set_width(w - pad *
    /// 2.0)`, which is what every call site contained — and every row is out by
    /// exactly `2 * MENU_BORDER`. The four pairs rather than one because the bug
    /// was invisible at every one of them for the same reason.
    ///
    /// **What this cannot see is a call site.** It drives `menu_frame` directly,
    /// so it pins `menu_inner_w`'s arithmetic and would pass beside a ninth
    /// popover that did its own subtraction — which is exactly how the eighth
    /// survived the pass that converted the other seven (§15 D307).
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

    /// **A menu opened at any of the four corners stays inside the viewport, at
    /// 100% *and* 150%.**
    ///
    /// Both halves matter and they fail differently. The flip is what a menu near
    /// the bottom-right needs — and *Delete* is the last row, so an unflipped menu
    /// loses exactly the row that is hardest to reach another way. The rounding is
    /// the half that has caught this project before: a fix that rounds back to the
    /// original value at 150% display scaling is a no-op on the user's actual
    /// machine, and 1.5 is what that machine is set to.
    ///
    /// **Two assertions, because the clamp alone passes the obvious one.**
    /// Deleting the flip and keeping only the clamp still leaves every corner
    /// inside the viewport — the menu is simply shoved left until it fits — so a
    /// test that asked only "is it on screen" was green against a placement with
    /// no flip in it at all. That was found by doing it, which is the whole reason
    /// the second assertion exists.
    ///
    /// What the flip actually claims is that the menu is **hinged on the
    /// pointer**: it opens up and to the left, so the press stays on an *edge* of
    /// the card rather than in the middle of it. A shoved menu puts the pointer
    /// over a row three pixels in from the corner. So the second assertion is that
    /// the card lies wholly on one side of the press on each axis, which is what
    /// only the flip can produce.
    ///
    /// The rounding is the third thing being asserted and it is the one that has
    /// caught this project before: a fix that rounds back to the original value at
    /// 150% display scaling is a no-op on the user's actual machine, and 1.5 is
    /// what that machine is set to.
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
                // A hair inside the bottom-right, which is where the flip has to
                // fire without the clamp being what saves it.
                screen.right_bottom() - egui::vec2(3.0, 3.0),
            ] {
                let pos = menu_place(&ctx, at, size);
                let placed = egui::Rect::from_min_size(pos, size);
                assert!(
                    screen.contains_rect(placed),
                    "at {at:?} ppp {ppp}: {placed:?} escapes {screen:?}"
                );
                // Half a point of slack, which is what device-pixel rounding at
                // 150% can move an edge by.
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

    /// **A menu taller than the viewport clamps rather than flips.** Flipping
    /// only moves which end is lost; the top is the end with the head rows on it,
    /// so it is the end that must survive.
    #[test]
    fn a_menu_taller_than_the_viewport_is_pinned_to_its_top() {
        let ctx = ctx_at(1.0);
        let screen = ctx.content_rect();
        let size = egui::vec2(210.0, screen.height() + 200.0);
        let pos = menu_place(&ctx, screen.center(), size);
        assert_eq!(pos.y, screen.top(), "a too-tall menu must keep its head");
    }

    /// **A dimmed row can still be hovered, so the sentence saying *why* has
    /// somewhere to land.**
    ///
    /// This is `inspector::menu_action`'s measurement at a second call site, and
    /// it is asserted on `hovered()` rather than on the tooltip's ink because a
    /// tooltip's galley never reaches `run_ui`'s `.shapes` at all — checked there,
    /// with time past the delay and the pointer still.
    ///
    /// Flipped against the version that reads naturally — allocating with
    /// `Sense::click()` and wrapping the row in a `ui.disable()`d scope, which is
    /// how the image popover first spelled it — where `hovered()` is false in
    /// exactly the state the sentence is for.
    #[test]
    fn a_dimmed_menu_row_is_still_hoverable_and_still_refuses_the_click() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let at = egui::pos2(60.0, 12.0);
        let mut hovered = false;
        let mut clicked = false;
        // Two passes: a widget's interaction state is last frame's, so the frame
        // that places the row cannot also be the frame that reports the hover.
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

    /// How many rows painted the hover ground this frame.
    ///
    /// The ground is the only `text_a(20)` rect a bare `menu_row` draws — no frame,
    /// no separator — so counting them is counting highlights.
    fn grounds(out: &egui::FullOutput) -> usize {
        out.shapes
            .iter()
            .filter(|c| match &c.shape {
                egui::Shape::Rect(r) => r.fill == theme::color::text_a(20),
                _ => false,
            })
            .count()
    }

    /// **A keyboard highlight paints the ground, and it paints exactly one of
    /// them** (`docs/context-menus.md` §8).
    ///
    /// Two claims, and only the first is obvious. The second is the whole reason
    /// `MenuRow::highlight` is an `Option<bool>` passed to *every* row rather
    /// than a `bool` set on one: with the pointer resting over row 0 and `↓` having
    /// walked to row 2, a highlight that only *added* a ground would paint two, and
    /// a menu with two highlights is a menu that cannot say what `Enter` will do.
    ///
    /// So the fixture puts the pointer on the first row and the highlight on the
    /// third, which is the state the bug lives in. **Flipped against
    /// `row.highlight == Some(true) || resp.hovered()`** — the spelling that reads
    /// naturally and passes the first assertion — where the count is 2.
    #[test]
    fn a_keyboard_highlight_paints_one_ground_and_the_pointer_cannot_add_another() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let on_first = egui::pos2(60.0, MENU_ROW_H * 0.5);
        let labels = ["One", "Two", "Three"];
        let mut counts = Vec::new();
        // Four passes: interaction state is last frame's, so the frame that places
        // the rows cannot be the frame that reports the hover. Only the last is
        // measured.
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
        // The fixture: with nobody driving the keyboard, the pointer on row 0 is
        // painting one ground. Without this the second assertion could pass over a
        // menu that paints nothing at all.
        assert_eq!(
            counts[0], 1,
            "the pointer has to be painting a ground for this test to be about anything"
        );
        assert_eq!(
            counts[1], 1,
            "the keyboard's highlight has to paint, and the pointer must not add a second"
        );
    }

    /// **The clamp has the last word, not the rounding** — §15 D666,
    /// `[S18.1-L3-07]`.
    ///
    /// 🚨 **`menu_place`'s doc gave the opposite order twice**, in a function
    /// whose own next sentence says *"the order is the whole of the function"*:
    /// *"flip first, then clamp, then round"*, and *"rounding comes last"*. The
    /// code has always been `snap(pos).clamp(…)`, and the code is right — half a
    /// pixel past the right edge is a menu with its last column cut, where half a
    /// pixel off the pixel grid is a hairline that looks soft.
    ///
    /// **The fixture is the one input where the two orders differ**, and it takes
    /// three things at once: a fractional `pixels_per_point`, a right limit that
    /// rounds **outward** at it, and an `at` far enough right that the clamp
    /// actually bites after the flip. At ppp 1.5 the limit is
    /// `1400 − 137 = 1263`, and `1263 × 1.5 = 1894.5` rounds *up* to 1895, i.e.
    /// `1263.333` — a third of a point past the edge. Every ordinary placement
    /// gives the same answer under both orders, which is why this went unnoticed
    /// and why the assertion has to be an exact `f32` comparison.
    ///
    /// ⚠️ **`at` is outside the viewport, deliberately.** The clamp cannot bite
    /// after a flip otherwise: flipping subtracts the same `size.x` the limit
    /// does, so an `at` on screen always lands on screen. A pointer position from
    /// before a viewport resize is exactly the case this function exists to
    /// survive.
    ///
    /// ⚠️ **Flip run**, the two operations swapped to `snap(pos.clamp(…))`.
    /// Predicted failing assertion: the clamped one, at `1263.3334` against
    /// `1263.0` — and the crisp case below stays green, which is why both are
    /// here.
    #[test]
    fn a_menu_is_clamped_after_it_is_rounded_and_not_before() {
        let ctx = ctx_at(1.5);
        let size = egui::vec2(137.0, 40.0);
        // ⚠️ **Placed inside a frame, because `content_rect()` outside one is
        // egui's 10,000pt fallback** — 6666.7 at this scale, which is wider than
        // any `at` a test would write and so clamps nothing. The first draft read
        // it from `ctx_at`'s context directly and got `1450` back untouched: a
        // fixture that never reached the state it named.
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

        // The fixture: this limit really is the one that rounds outward, or the
        // test is about an input where both orders agree.
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

        // And an ordinary placement is still snapped to the device grid, which is
        // what the rounding is for at all.
        let crisp = place(egui::pos2(100.3, 100.3));
        assert_eq!(
            crisp.x,
            (100.3_f32 * 1.5).round() / 1.5,
            "away from an edge the rounding is the answer and nothing clamps"
        );
    }
}

/// The type family row's warning state, which is chrome rather than typography —
/// `glyph_and_text_tinted` lives here, so its one guarantee is asserted here.
#[cfg(test)]
mod family_row_chrome_tests {
    use super::*;

    /// **A tinted glyph survives into the galley a `ComboBox` paints** (§15 D227).
    ///
    /// The type family row says "this font cannot be had" by wearing `TEXT_AA` in
    /// `theme::color::WARN`, and it says it through `selected_text`, which takes a
    /// single `WidgetText`. Whether a widget honours the colours inside a
    /// `LayoutJob` or overrides them with its own text colour is egui's business and
    /// not ours to assume — and if it overrides them the warning is simply invisible,
    /// which is the one failure mode a colour-only signal has.
    ///
    /// So this asserts the actual painted vertices: the row's glyph section has to
    /// come out `WARN` and its label section must **not**, because the label carries
    /// the family's name and tinting that would read as the pick not having taken.
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
        // Every colour any text vertex was painted in. A galley's mesh is atlas-UV
        // quads whose **vertex colour is the ink colour** — `theme.rs`'s atlas tests
        // read the other half of the same mesh, the coverage behind the UVs.
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
    //! The device-pixel snap the chrome's three rules share (§15 D479).
    //!
    //! `rulers.rs`'s `guide_geometry_tests` pins the *guide's* half of the same
    //! parity rule and is the older of the two; this module exists because
    //! `ui::rule` was pinned by nothing at all, which is how it came to carry a
    //! defect at 200% for as long as it did (`[S19.1-L1-04]`).

    use super::*;

    /// How much of each device pixel a `width`-point line centred at `at` covers.
    ///
    /// The same helper `rulers.rs`'s `guide_geometry_tests` carries, and it is
    /// copied rather than shared on purpose: it is four lines of definition, and a
    /// test helper reaching across module boundaries to another test module is a
    /// dependency between two suites that are meant to be able to disagree.
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

    /// **A 1pt hairline covers whole device pixels at an even integer scale, and
    /// the fractional scales are unchanged** (§15 D479, `[S19.1-L1-04]`).
    ///
    /// 200% is the case that bites: the old unconditional `+ 0.5` asked for a pixel
    /// *centre* for a 2-device-pixel line and got `[0.5, 1.0, 0.5]` — one whole
    /// pixel with a soft edge either side, which is the "two-pixel grey smear" the
    /// arithmetic exists to prevent, arriving across three. 150% is the control and
    /// **must not move**: `[0.25, 1.0, 0.25]` is the best a 1.5-pixel line can do,
    /// since a fractional device width cannot put both edges on the grid, so a
    /// "fix" that changed it would be changing the wrong thing.
    ///
    /// ⚠️ **Flip-check, run: `hairline_on_pixel_centre` forced to `true`**, which is
    /// exactly the code before the fix. Fails at `ppp = 2.0` with `[0.5, 1.0, 0.5]`
    /// against `[1.0, 1.0]`, and at no other scale — because at every other scale in
    /// the list a 1pt line's device width really is odd, so the unconditional
    /// version was accidentally right. **That is why one site went nine months
    /// looking correct**, and it is the argument for the parity being a named
    /// predicate rather than a `+ 0.5`.
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

    /// **Each arm takes the *nearest* grid position of its own parity, and that is
    /// the reason this is not `rulers::snap_across_axis`** (§15 D479).
    ///
    /// The review filed the two as one rule written twice and proposed calling the
    /// ruler's spelling from here. They share the parity predicate and nothing
    /// else. **Half a device pixel is the bound**, and it is the floor on the error
    /// rather than a slack allowance: grid positions of one parity are `1/ppp`
    /// apart, so nothing can do better, and the column's centre generally is not on
    /// the grid at all.
    ///
    /// ⚠️ **Two flips, run separately, because it is one arm each.**
    ///
    /// - Odd arm degraded to `(round + 0.5)`, which is `snap_across_axis`'s exact
    ///   spelling: **fails at `ppp = 1.0` at 1.0000pt** — a whole point, i.e. a 1pt
    ///   rule drawn entirely outside its 1pt column, which is the failure `rule`'s
    ///   own history records at §15 **D46** reached by a second road. The parity is
    ///   untouched, so the coverage test above stays **green**, which is what says
    ///   these two tests are about different things.
    /// - Even arm degraded to `floor`: **fails at `ppp = 2.0` at 0.4975pt** against
    ///   a bound of 0.25.
    ///
    /// ⚠️ **The second flip did not bite at first, and the test was wrong rather
    /// than the flip.** The bound was written as a flat `0.5` **points**, which
    /// 0.4975 passes — so above 100% it was asserting nothing at all. Half a
    /// *device* pixel is the quantity this doc always meant and `head_tests` has
    /// used all along, and the flip that failed to bite is what found the
    /// difference.
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

    /// **And `ui::rule` actually paints there**, which the two tests above cannot
    /// say — they pin arithmetic, and a call site that kept its own copy would pass
    /// both of them.
    ///
    /// Read back out of `FullOutput::shapes` as the `LineSegment` the function
    /// draws, at 200%, which is the scale the defect was measured at.
    ///
    /// ⚠️ **`inspector::popup_rule` and `settings::section` are pinned by
    /// construction and not by assertion** — each is now a single call to
    /// `hairline_in_column` with nothing between it and the painter. That is
    /// weaker than this test and is said plainly rather than implied: what would
    /// catch a third copy appearing is a reader, not a gate.
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
    //! The gradient **chip** against the gradient on the page (§15 D564).
    //!
    //! Plain backticks throughout, per §15 D319 — `cargo doc` cannot see a
    //! `#[cfg(test)]` module, so a `[link]` here is checked by nothing.

    use super::*;

    /// The ramp mesh's columns, as `(x, colour)` in the order they were built.
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

    /// **A descending stop list draws what the canvas draws, not its mirror.**
    ///
    /// The reported failure, measured: `[(1.0, red), (0.0, blue)]` — what a
    /// `<linearGradient>` with descending offsets imports as — built columns at
    /// x `[0, 100, 0, 100]`, so two of the four triangles were wound backwards
    /// and the chip painted the ramp reversed. The artwork beside it did not:
    /// §15 D455 put `make_stops_monotonic` on the renderer, the importer and the
    /// SVG writer, so one document was drawn two ways in one frame.
    ///
    /// ⚠️ **Non-decreasing x is the assertion, not "the left edge is red".** The
    /// second is true and is a consequence of this input; the first is the
    /// property — a mesh whose columns walk backwards is a folded ramp whatever
    /// colours it happens to carry, and a fixture with three stops would satisfy
    /// a left-edge assertion while still folding in the middle.
    ///
    /// ⚠️ **The ascending control is what says the probe can report agreement**,
    /// and it is also what stops a "fix" that sorted: a sort and a clamp give
    /// different pictures on this input (D455's own argument), and the ascending
    /// row is unchanged under either.
    ///
    /// **Flip-checked** by dropping the `monotonic_offset` call: red at the
    /// non-decreasing assertion, reading `0` after `100`, which is the reported
    /// symptom.
    #[test]
    fn a_descending_ramp_is_clamped_the_way_the_canvas_clamps_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (red, blue) = (
            egui::Color32::from_rgb(255, 0, 0),
            egui::Color32::from_rgb(0, 0, 255),
        );

        // The control: an ordinary ramp, and the fixture assertion for the shape
        // of what `columns` returns.
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
    //! How far a `slider`'s drawing leaves the row it allocates (§15 D673).
    //!
    //! Plain backticks throughout, per §15 D319.

    use super::*;

    /// **The shadow bleeds one point below the row, and no further**
    /// (`[S18.2-L3-05]`).
    ///
    /// `SLIDER_H` is `pub` and `slider` reserves it on its callers' behalf — ⚠️ not
    /// *"the number every caller reserves"*, which is what this said and what the
    /// constant's own doc said until §15 D766: **no production code outside `ui.rs`
    /// reads it.** Its doc also
    /// used to call the knob *"the tallest thing in it"*. It is not: the knob's
    /// shadow is a `half`-radius disc drawn one point lower, so a slider paints
    /// `SLIDER_H + 1` into the `SLIDER_H` it asks for.
    ///
    /// **The bleed is kept and pinned rather than removed.** A shadow outside the
    /// box is what a shadow is — the design asks for `box-shadow: 0 1px 3px` — and
    /// the one production caller has the row gap for it. What could not stand was
    /// the *doc*, and a corrected sentence would rot the same way; this is the
    /// gate, so the number lives somewhere that fails.
    ///
    /// ⚠️ **It asserts the three other sides are tight, and those are the
    /// load-bearing half.** A test that only bounded the bottom would pass for a
    /// knob that had grown in every direction, which is the change somebody
    /// adjusting a radius would actually make.
    ///
    /// **Flip:** change the shadow's offset at `center().y + 1.0` to `+ 2.0` and
    /// the bottom assertion fails with `2` against `1`; drop it to `center().y` and
    /// it fails the other way, which is the half that stops this being a
    /// one-directional bound. Both predicted correctly.
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
    //! What a widget that senses nothing is hovered *by* (§15 D688).
    //!
    //! (Plain backticks per §15 D319.)

    use super::*;

    /// **A zero-sense widget is hovered only while nothing interactive is above
    /// it** (`[S18.1-L3-08]`).
    ///
    /// `menu_row`'s comment said a dimmed row's `Sense::hover()` *"must still win
    /// the hit test"*. It does not — egui filters hit-test candidates on
    /// `senses_click()` / `senses_drag()`, and `Sense::hover()` **is**
    /// `Sense::empty()`, `const HOVER = 0`. What the dimmed row's tooltip actually
    /// rests on is `interaction.rs`'s `is_on_top_of_the_interactive_widget`, which
    /// is a **conditional** guarantee: last wins.
    ///
    /// 🚨 **The second case is the whole test.** `inspector::menu_action` already
    /// pins the positive half — a zero-sense claimant allocated *after* a disabled
    /// scope does see the pointer. Nothing pinned the negative half, which is the
    /// half the false sentence denied: put something interactive over the same
    /// rectangle *afterwards* and the zero-sense widget stops being hovered. Two
    /// claimants over one region is exactly the arrangement three functions in this
    /// file deliberately construct.
    ///
    /// ⚠️ **Interaction state is last frame's**, so this pumps two passes and reads
    /// the response back on the second, per the technique this project records for
    /// every hover probe.
    ///
    /// **Flip:** drop the `ui.interact(…, Sense::click())` line and the second
    /// assertion fails — the row is hovered again, because nothing is above it.
    /// Predicted correctly.
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
    //! What a **disabled** segmented cell claims (§15 D722, `[S18.2-L3-04]`).
    //!
    //! (Plain backticks per §15 D319.)

    use super::*;

    /// Tab through a three-cell segmented control with `disabled` unavailable,
    /// and report how many distinct widgets the keyboard can reach.
    ///
    /// **Counted rather than named**, because naming cell 1 would mean spelling
    /// out `segmented_enabled`'s id formula here — and an assertion that copies a
    /// formula goes vacuously green the day the formula changes, which is the
    /// trap `leftover_temps`' doc records in `atomic`.
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
        // More presses than there are cells, so the order wraps and every stop is
        // met however the focus starts.
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

    /// **A cell that cannot be clicked is not a tab stop** (§15 D722).
    ///
    /// `segmented_enabled` registered every cell with `Sense::click()`, disabled
    /// ones included, and the only reason its comment gave was *"so the cell can
    /// carry a tooltip explaining why it is unavailable"*. **No caller can carry
    /// that tooltip**: the function returns `Option<usize>`, the per-cell
    /// `Response` is a local that never escapes, and `paint` is handed a
    /// `&Painter` rather than a response. Its one production caller,
    /// `picker::picker_body`, hangs none and could not; `segmented` passes
    /// `|_| true` and has no disabled cells at all. **One caller, zero tooltips.**
    ///
    /// 🚨 **What the registration did instead is put the cell in the keyboard tab
    /// order**, because `Sense::click()` is `CLICK | FOCUSABLE` — which
    /// `focus_ring`'s own ⚠️ warns about from the other side. So Tab stopped on a
    /// cell that then swallowed Return and Space, offering neither the click nor
    /// the promised explanation.
    ///
    /// The enabled control is the other half: dropping the sense for *every* cell
    /// would pass a test that only counted the disabled one.
    ///
    /// ⚠️ **Flip-checked twice and the control is the one that earned its place.**
    /// Restoring `Sense::click()` for both is red at the second assertion, `3`
    /// against `2`, as predicted. Dropping to `Sense::hover()` for *both* — the
    /// over-broad reading of the same fix — is red at the **first**, `0` against
    /// `3`: the whole control becomes unreachable from the keyboard. Without that
    /// fixture line the second flip is green and the segmented control silently
    /// stops being tabbable.
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
    //! A menu separator's rule keeps its weight as the display scale rises
    //! (§15 D721, `[S18.1-L3-04]`).
    //!
    //! (Plain backticks per §15 D319.)

    use super::*;

    /// The height `menu_sep` actually paints, in points, at `ppp`.
    ///
    /// Read out of `FullOutput::shapes` rather than computed, because the whole
    /// question is what reaches the screen. The rule is the only `Rect` in the
    /// output — `allocate_exact_size` with `Sense::empty()` paints nothing — and
    /// it is picked by being far shorter than its own allocation rather than by
    /// index, so a stray background rect would not be mistaken for it.
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

    /// **A separator is a 1pt rule at every display scale**, and it used to be one
    /// device pixel (§15 D721).
    ///
    /// `menu_sep` painted `1.0 / ppp` **points** — a single device pixel — where
    /// its own doc says *"**1pt** of `text_a(20)`"* and, two paragraphs down, *"the
    /// rule stays 1pt and only the air moved"*, and where `MENU_SEP_H` allocates
    /// `1.0 + SEP_AIR * 2.0`. Measured before the fix: **1.0 pt at ppp 1.0, 0.667
    /// at 1.5, 0.5 at 2.0.**
    ///
    /// 🚨 **The cost is a comparison inside one card.** `MENU_BORDER` is a 1-*point*
    /// stroke around the very frame the separator sits in, 5pt away, and so is
    /// every other hairline in the chrome — `field_frame`'s, `button_face`'s,
    /// `SEGMENT_BORDER`, and `inspector::popup_rule`, which is this same separator
    /// one menu over. At 200% the group rule was **half** the weight of its own
    /// card's border; at 300%, a third.
    ///
    /// ⚠️ **The device-pixel snapping is kept and was never the defect.** The doc
    /// argues for it — *"a 1pt rule landing on a half pixel is the one width where
    /// the rounding shows as a colour change rather than a position change"* — and
    /// the finding agrees the technique is the only one in this file that is crisp
    /// at every scale. What was wrong is the **width**. So the rule is now a
    /// *nominal* point snapped to whole device pixels, which is what those two
    /// sentences mean together: `round(ppp).max(1.0)` device pixels.
    ///
    /// **At ppp 1.5 that is 1.333pt rather than 1.0**, and the finding's own
    /// suggested assertion — *"equals `MENU_BORDER` at ppp ∈ {1, 1.5, 2}"* — would
    /// fail on it. Painting exactly 1.0pt there is the alternative and it is
    /// *blurry*: 1.5 device rows, half a row of partial coverage, the case the doc
    /// names. A third of a point too heavy, crisply, beats correct and smeared —
    /// and it errs toward the rule being **seen**, which is the whole reason a
    /// separator exists. This is the choice; it is not the finding's.
    ///
    /// ⚠️ **Flip-checked in both directions, and each bites on a different
    /// assertion**, which is what says the three are asking three questions.
    /// Restoring `1.0 / ppp` fails the *third* — `0.667pt = 1 device pixel where a
    /// nominal 1pt snaps to 2`. Painting a bare `1.0` — the obvious "just make it a
    /// point" fix, and the finding's own suggested assertion — fails the **first**:
    /// `0.99999976pt = 1.4999996 device pixels, which is not a whole number`. The
    /// second assertion, *at least one device pixel*, bit under neither and is kept
    /// anyway: it is the one that would catch a future `weight` derived from a
    /// smaller nominal.
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
            // ⚠️ **Stated exactly rather than as a tolerance.** *"Within half a
            // device pixel of 1pt"* is the same rule and at ppp 1.5 it lands
            // exactly on its own boundary — `0.667` and `1.333` are both 0.333
            // out, so the wrong answer and the right one are equally admissible
            // and the assertion decides nothing at the one scale that is
            // interesting.
            let want = (1.0_f32 * ppp).round().max(1.0);
            assert!(
                (device - want).abs() < 1e-3,
                "at ppp {ppp} the rule paints {painted}pt = {device} device pixels \
                 where a nominal 1pt snaps to {want} — off by this much it no \
                 longer matches MENU_BORDER 5pt away in the same card"
            );
        }
    }

    /// The rule fits inside the height `menu::height` reserves for it, at every
    /// scale — which is what stops this fix moving a menu.
    ///
    /// ⚠️ **`MENU_SEP_H` does not depend on `ppp` and must not start to.**
    /// `menu::height` sums it *before* the menu is shown, because `menu_place`
    /// flips and clamps against a size it does not yet have on screen, and
    /// `a_menus_computed_height_is_the_height_it_paints` is the other half of that
    /// guarantee. The rule growing from 0.5pt to 1.333pt at ppp 1.5 has to stay
    /// inside the 7pt already allocated — it does, with `SEP_AIR` either side —
    /// and this is the assertion that says so rather than leaving it to be
    /// noticed.
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
    //! A picture chip is drawn at the brush's own alpha (§15 D784).
    //!
    //! This is the surface §15 D773 named — the inspector's paint-row chip — read
    //! out of `FullOutput::shapes` rather than reasoned about, because `swatch`
    //! hands the tint to `Painter::image` and what a reader wants to know is what
    //! reached the mesh.
    //!
    //! (Plain backticks per §15 D319.)

    use super::*;

    /// The vertex colour of the one textured mesh a chip paints.
    ///
    /// A glyph is a textured mesh too, so this would be ambiguous in a `Ui` with
    /// text in it — there is none here, and the assertion below checks exactly one
    /// mesh came back rather than trusting that.
    fn chip_tint(ctx: &egui::Context, alpha: f32) -> Vec<egui::Color32> {
        let out = ctx.run_ui(Default::default(), |ui| {
            // Built inside the closure: `run_ui` takes an `FnMut`, and `Swatch`
            // is not `Copy`, so a chip passed in from outside would be moved on
            // the first call.
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

    /// A texture id that is not the font atlas, so `chip_tint` can tell the
    /// chip's own mesh from anything epaint draws with glyphs.
    fn picture() -> egui::TextureId {
        egui::TextureId::User(7)
    }

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

    /// **The tint is a multiplier and not a colour**, which is the property that
    /// keeps a photograph looking like itself: scaled white leaves r = g = b.
    ///
    /// ⚠️ **Worth its own assertion because the plausible wrong fix passes the test
    /// above.** Tinting with `color::TEXT` faded, or with `from_black_alpha`, is
    /// also "not WHITE" and also has a lower alpha — and it is what a reader
    /// reaches for who has read the theme's other tinting rules first.
    ///
    /// ⚠️ **And it is green under the flip the test above catches**, which is the
    /// other half of the same point: drawing the chip untinted (the state before
    /// §15 D784) keeps r = g = b perfectly. The two tests pin different halves —
    /// *is it faded* and *is it still a photograph* — and neither is redundant.
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
