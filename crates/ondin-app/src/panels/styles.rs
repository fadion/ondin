//! Styles in the inspector (§5.3f, D1038 F1–F10; §15 D1063).
//!
//! The four style slots' cards — Fill, Stroke, Effects and, since step 6, Type
//! (§15 D1068) — wear a `⋯` in their
//! header, always last (F8): *Apply style…*, *Create style…* and
//! *Detach style* (F4). A styled card **collapses to one capsule** — caret,
//! preview, name and a short summary, *Detach* in the summary's place on hover
//! (F1, F2) — and opened it shows the card's rows greyed and inert, *Read-only —
//! Detach to edit* (F3): the inspector is the one door that does not detach
//! (D1038 (1)). Its `+` hides while any card in scope is styled, and nothing
//! moves (F8, F10).
//!
//! A style's values change through the browser's *Edit* (G9), which here opens
//! **an edit mode on the style's users** rather than a card inside the browser
//! (§15 D1063): the browser selects them, the card opens live under an *editing*
//! capsule, and the commit door turns its writes into an edit of the style
//! (`style::edit_through`), which the commit writes back onto every user.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui;
use eframe::egui;
use ondin_core::style::{self, Style, StyleId, StyleSlot, StyleValue};
use ondin_core::{Brush, Document, NodeId, Operation, Transaction};

/// The capsule's height and its corner — fully rounded, the reference shape
/// (F1, D1038's capsule rule).
const CAPSULE_H: f32 = 28.0;
/// The menu's and the picker's widths (F4, F5).
const MENU_W: f32 = 210.0;
const PICKER_W: f32 = 256.0;
const ROW_H: f32 = 26.0;
/// Greyed rows' opacity under a read-only capsule (F3).
pub(crate) const GREYED: f32 = 0.42;

/// How a card's targets stand towards styles in one slot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StyleShown {
    /// None of them is styled here.
    Plain,
    /// Every one is bound to this style.
    One(StyleId),
    /// Every one is styled, not all alike — the styles, in id order (F10).
    Mixed(Vec<StyleId>),
    /// Some are styled and some are not (F10's second case).
    Partial,
}

impl StyleShown {
    pub(crate) fn any(&self) -> bool {
        !matches!(self, StyleShown::Plain)
    }
}

/// Where `targets` stand in `slot`.
pub(crate) fn shown(doc: &Document, targets: &[NodeId], slot: StyleSlot) -> StyleShown {
    let refs: Vec<Option<StyleId>> = targets
        .iter()
        .filter_map(|t| doc.get(*t))
        .map(|n| n.styles().get(slot))
        .collect();
    if refs.is_empty() || refs.iter().all(Option::is_none) {
        return StyleShown::Plain;
    }
    if refs.iter().any(Option::is_none) {
        return StyleShown::Partial;
    }
    let mut ids: Vec<StyleId> = refs.into_iter().flatten().collect();
    ids.sort();
    ids.dedup();
    match ids.as_slice() {
        [one] => StyleShown::One(*one),
        _ => StyleShown::Mixed(ids),
    }
}

/// The `⋯` menu, open under a card's header.
pub(crate) struct StyleMenu {
    slot: StyleSlot,
    targets: Vec<NodeId>,
    under: egui::Rect,
}

/// *Apply style…*'s picker (F5), hung under the card's header.
pub(crate) struct StylePicker {
    slot: StyleSlot,
    targets: Vec<NodeId>,
    under: egui::Rect,
    query: String,
    focus: bool,
    highlight: usize,
}

/// A style just made by *Create style…*, its name being typed in the
/// capsule (F6): `Enter` keeps, `Escape` undoes.
pub(crate) struct StyleNaming {
    pub(crate) style: StyleId,
    text: String,
    focus: bool,
    refusal: Option<String>,
}

/// What a capsule was asked this frame.
#[derive(Default)]
struct CapsuleOut {
    toggled: bool,
    detach: bool,
}

/// The values a card's targets agree on in `slot`, if they do — what *Create
/// style from this…* makes a style of.
fn agreed(doc: &Document, targets: &[NodeId], slot: StyleSlot) -> Option<StyleValue> {
    // A layer with no layout has no layout value, so a set holding one agrees on
    // none (§15 D1075). As drawn, so two layers bound to different families do
    // not agree on the empty literal they both store (§15 D1077).
    let mut values = targets
        .iter()
        .filter(|t| doc.get(**t).is_some())
        .map(|t| style::value_drawn(doc, *t, slot));
    let first = values.next()??;
    values
        .all(|v| v.is_some_and(|v| v.same_look(&first)))
        .then_some(first)
}

/// A layout's glyph (F7): Phosphor's *columns* for a row, the same picture
/// turned, *rows*, for a column — the display glyph turned with the direction —
/// and the four squares for a grid (§15 D1076).
pub(crate) fn layout_glyph(display: &ondin_core::container::Display) -> &'static str {
    use ondin_core::container::{Display, FlexDirection};
    match display {
        Display::Flex(f) => match f.direction {
            FlexDirection::Row | FlexDirection::RowReverse => icon::COLUMNS,
            FlexDirection::Column | FlexDirection::ColumnReverse => icon::ROWS,
        },
        Display::Grid(_) => icon::GRID_FOUR,
    }
}

/// A style's short summary (F1): `1 fill`, `1 · inside`, `2 shadows`.
fn summary(doc: &Document, value: &StyleValue) -> String {
    let plural = |n: usize, one: &str, many: &str| match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    match value {
        StyleValue::Fills(v) => plural(v.len(), "fill", "fills"),
        StyleValue::Strokes(v) => match v.as_slice() {
            [one] => format!(
                "{} · {}",
                number(width_shown(doc, one)),
                format!("{:?}", one.align).to_lowercase()
            ),
            v => plural(v.len(), "stroke", "strokes"),
        },
        StyleValue::Effects(v) => {
            use ondin_core::EffectKind as K;
            let shadows = v
                .iter()
                .all(|e| matches!(e.kind, K::DropShadow(_) | K::InnerShadow(_)));
            if shadows && !v.is_empty() {
                plural(v.len(), "shadow", "shadows")
            } else {
                plural(v.len(), "effect", "effects")
            }
        }
        // `Inter · 16` — the family and the size, what a type style is read by.
        StyleValue::Text(t) => format!("{} · {}", t.font_family, number(t.font_size)),
        // `Column · 12` — the direction and the main-axis gap; a grid its track
        // counts, `Grid · 3 × 2` (§15 D1076).
        StyleValue::Layout(l) => {
            use ondin_core::container::{Display, FlexDirection};
            match &l.display {
                Display::Flex(f) => {
                    // The main axis's gap: a row's column gap (slot 0), a
                    // column's row gap (slot 1), `SpacingVars`' order.
                    let (word, gap, slot) = match f.direction {
                        FlexDirection::Row | FlexDirection::RowReverse => ("Row", f.column_gap, 0),
                        FlexDirection::Column | FlexDirection::ColumnReverse => {
                            ("Column", f.row_gap, 1)
                        }
                    };
                    match l.spacing_vars[slot].is_some() {
                        // A bound gap's literal is its 0: the summary names the
                        // binding rather than a number the layout never lays.
                        true => format!("{word} · bound gap"),
                        false => format!("{word} · {}", number(gap)),
                    }
                }
                Display::Grid(g) => format!(
                    "Grid · {} × {}",
                    g.columns.len().max(1),
                    g.rows.len().max(1)
                ),
            }
        }
    }
}

/// A stroke style's width as drawn in the document's default mode, the colour's
/// rule in [`preview_colour`]: a bound width's literal is its 0 (§15 D1073), and
/// the summary read `0 · inside` and the ring drew the clamp's 1 over a style
/// whose layers draw its variable's width.
fn width_shown(doc: &Document, s: &ondin_core::Stroke) -> f64 {
    match s.width_var {
        Some(v) => doc.stroke_width_drawn_in(doc.variables(), doc.root(), v),
        None => s.width,
    }
}

/// The colour a fill or stroke style is previewed in — its top visible paint
/// as drawn in the document's default mode, a gradient's first stop.
fn preview_colour(doc: &Document, value: &StyleValue) -> Option<ondin_core::peniko::Color> {
    let brushes: Vec<&ondin_core::BrushSource> = match value {
        StyleValue::Fills(v) => v
            .iter()
            .rev()
            .filter(|f| f.visible)
            .map(|f| &f.brush)
            .collect(),
        StyleValue::Strokes(v) => v
            .iter()
            .rev()
            .filter(|s| s.visible)
            .map(|s| &s.brush)
            .collect(),
        StyleValue::Effects(_) | StyleValue::Text(_) | StyleValue::Layout(_) => return None,
    };
    let b = brushes.first()?;
    match doc.resolve_brush(doc.root(), b).into_owned() {
        Brush::Solid(c) => Some(c),
        other => crate::panels::paint::stops_of(&other)
            .first()
            .map(|(_, c)| *c),
    }
}

/// A style's preview: a 14×14 swatch for a fill, a ring for a stroke, a small
/// shadowed square for effects (F1, G8), and *Ag* for a text style (§15 D1068)
/// — in the chrome's face, since a document's families are not the UI's fonts.
pub(crate) fn paint_preview(p: &egui::Painter, doc: &Document, style: &Style, rect: egui::Rect) {
    let ink = |a: u8| color::text_a(a);
    match &style.value {
        StyleValue::Fills(_) => {
            // The Fill row's chip (§15 D1082); a style with no colour to show
            // reads as the checkerboard alone.
            let c = preview_colour(doc, &style.value)
                .map_or(egui::Color32::TRANSPARENT, ui::color_to_egui);
            ui::paint_colour_chip(p, rect, c);
        }
        StyleValue::Strokes(v) => {
            let c = preview_colour(doc, &style.value)
                .map(ui::color_to_egui)
                .unwrap_or(ink(178));
            let w = v
                .first()
                .map_or(1.0, |s| width_shown(doc, s).clamp(1.0, 3.0)) as f32;
            p.rect_stroke(rect, 4, egui::Stroke::new(w, c), egui::StrokeKind::Inside);
        }
        StyleValue::Effects(_) => {
            let sq = egui::Rect::from_center_size(rect.center(), egui::vec2(11.0, 11.0));
            p.rect_filled(
                sq.translate(egui::vec2(0.0, 2.0)).expand(1.5),
                4,
                egui::Color32::from_black_alpha(140),
            );
            p.rect_filled(sq, 3, ink(178));
        }
        StyleValue::Text(_) => {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Ag",
                egui::FontId::proportional(11.0),
                ink(204),
            );
        }
        // F7: the display glyph turned with the direction (§15 D1076).
        StyleValue::Layout(l) => {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                layout_glyph(&l.display),
                theme::icon_font(13.0),
                ink(204),
            );
        }
    }
}

/// A capsule's frame: the fully rounded ground, `hover` lifting it (F1, F2).
fn capsule_ground(
    ui: &mut egui::Ui,
    id: egui::Id,
    sense: egui::Sense,
) -> (egui::Rect, egui::Response) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, CAPSULE_H), egui::Sense::hover());
    let resp = ui.interact(rect, id, sense);
    (rect, resp)
}

/// A styled card's capsule (F1–F3, G9's *editing*): caret, preview, name and
/// summary; on hover, *Detach* — or while editing, *Done* — in the summary's
/// place. The capsule's own click opens or closes the card under it.
fn capsule(
    ui: &mut egui::Ui,
    doc: &Document,
    style: &Style,
    open: bool,
    editing: bool,
) -> CapsuleOut {
    let mut out = CapsuleOut::default();
    let (rect, resp) = capsule_ground(
        ui,
        ui.id().with(("style-capsule", style.id)),
        egui::Sense::click(),
    );
    let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
    let p = ui.painter();
    p.rect_filled(
        rect,
        CAPSULE_H / 2.0,
        color::text_a(if hovered { 38 } else { 26 }),
    );
    if editing {
        p.rect_stroke(
            rect,
            CAPSULE_H / 2.0,
            egui::Stroke::new(1.0, color::ACCENT.gamma_multiply(0.65)),
            egui::StrokeKind::Inside,
        );
    }
    let mid = rect.center().y;
    let mut x = rect.left() + 9.0;
    p.text(
        egui::pos2(x + 5.5, mid),
        egui::Align2::CENTER_CENTER,
        if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        },
        theme::icon_font(11.0),
        color::text_a(115),
    );
    x += 11.0 + 8.0;
    let sw = egui::Rect::from_min_size(egui::pos2(x, mid - 7.0), egui::vec2(14.0, 14.0));
    paint_preview(p, doc, style, sw);
    x += 14.0 + 8.0;

    // The right-hand end: the summary at rest, the chip on hover.
    let chip_label = if editing { "Done" } else { "Detach" };
    let chip_glyph = if editing {
        icon::CHECK
    } else {
        icon::LINK_BREAK
    };
    let font = egui::FontId::proportional(10.5);
    let chip_w = p
        .layout_no_wrap(chip_label.into(), font.clone(), theme::text::STRONG)
        .size()
        .x
        + 11.0
        + 5.0
        + 16.0;
    let chip = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 4.0 - chip_w, mid - 10.0),
        egui::vec2(chip_w, 20.0),
    );
    let tail = if editing {
        "editing".to_string()
    } else {
        summary(doc, &style.value)
    };
    let tail_w = p
        .layout_no_wrap(tail.clone(), font.clone(), color::text_a(107))
        .size()
        .x;
    let name_right = if hovered {
        chip.left() - 6.0
    } else {
        rect.right() - 4.0 - 6.0 - tail_w - 6.0
    };
    let name = elided(p, &style.name, 12.0, name_right - x);
    p.galley(
        egui::pos2(x, mid - name.size().y / 2.0),
        name,
        theme::text::STRONG,
    );
    if hovered {
        let target = ui.interact(
            chip,
            ui.id().with(("style-capsule-chip", style.id)),
            egui::Sense::click(),
        );
        let p = ui.painter();
        p.rect_filled(
            chip,
            10.0,
            color::text_a(if target.hovered() { 48 } else { 36 }),
        );
        p.text(
            egui::pos2(chip.left() + 8.0 + 5.5, mid),
            egui::Align2::CENTER_CENTER,
            chip_glyph,
            theme::icon_font(11.0),
            color::text_a(191),
        );
        p.text(
            egui::pos2(chip.left() + 8.0 + 11.0 + 5.0, mid),
            egui::Align2::LEFT_CENTER,
            chip_label,
            font,
            theme::text::STRONG,
        );
        if target.clicked() {
            out.detach = true;
            return out;
        }
    } else {
        ui.painter().text(
            egui::pos2(rect.right() - 4.0 - 6.0, mid),
            egui::Align2::RIGHT_CENTER,
            tail,
            font,
            color::text_a(107),
        );
    }
    out.toggled = resp.clicked();
    out
}

/// F10's capsule over several layers styled differently: no caret, a split
/// swatch, *Mixed* and the count, its tooltip naming them.
fn mixed_capsule(ui: &mut egui::Ui, doc: &Document, styles: &[StyleId]) {
    let (rect, resp) = capsule_ground(ui, ui.id().with("style-mixed"), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CAPSULE_H / 2.0, color::text_a(26));
    let mid = rect.center().y;
    let x = rect.left() + 9.0;
    let sw = egui::Rect::from_min_size(egui::pos2(x, mid - 7.0), egui::vec2(14.0, 14.0));
    let found: Vec<&Style> = styles.iter().filter_map(|s| doc.styles().get(*s)).collect();
    if let [a, b, ..] = found.as_slice() {
        let (l, r) = sw.split_left_right_at_fraction(0.5);
        paint_preview(&p.with_clip_rect(l), doc, a, sw);
        paint_preview(&p.with_clip_rect(r), doc, b, sw);
    }
    p.text(
        egui::pos2(x + 22.0, mid),
        egui::Align2::LEFT_CENTER,
        "Mixed",
        egui::FontId::proportional(12.0),
        color::text_a(140),
    );
    p.text(
        egui::pos2(rect.right() - 10.0, mid),
        egui::Align2::RIGHT_CENTER,
        format!("{} styles", styles.len()),
        egui::FontId::proportional(10.5),
        color::text_a(107),
    );
    let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
    resp.on_hover_text(names.join("\n"));
}

/// F3's footer under a read-only card, and G9's under one being edited.
fn footer(ui: &mut egui::Ui, glyph: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(theme::icon_text(glyph, 10.0, color::text_a(102)));
        ui.label(
            egui::RichText::new(text)
                .size(10.0)
                .color(color::text_a(102)),
        );
    });
}

impl OndinApp {
    /// A style slot's card: [`OndinApp::panel_full`] with its `⋯`, and, while its
    /// targets are styled, the capsule in place of the rows (F1–F3, F10) — the
    /// rows themselves `body`. Answers whether the header's `+` was pressed,
    /// which a styled card never offers.
    pub(super) fn style_card(
        &mut self,
        ui: &mut egui::Ui,
        title: &'static str,
        slot: StyleSlot,
        targets: &[NodeId],
        action: Option<ui::HeadAction>,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> bool {
        self.style_card_live(ui, title, slot, targets, action, false, body, |_, _| {})
    }

    /// [`Self::style_card`] whose rows stay **live** under a styled capsule when
    /// `live` — the Type card over a live session's selection (§15 D1068): its
    /// writes there are spans, the text's own marks over the style, which keep
    /// the style (§15 D1067), so the inspector still detaches nothing.
    ///
    /// `tail` is the card's rows **the style does not hold**, drawn live under
    /// whatever `body` is in — the Type card's alignment row and its popup's
    /// opener, since a text style holds the character defaults and not the
    /// paragraph or the box (§15 D1067). Without it a styled card locked fields
    /// its style never writes (`arch-scribe`'s read of the first cut).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn style_card_live(
        &mut self,
        ui: &mut egui::Ui,
        title: &'static str,
        slot: StyleSlot,
        targets: &[NodeId],
        action: Option<ui::HeadAction>,
        live: bool,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
        tail: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> bool {
        let state = shown(&self.session.doc, targets, slot);
        let editing = matches!(state, StyleShown::One(s) if self.session.style_edit == Some(s));
        // The `+` hides while any card in scope is styled, and nothing moves:
        // the `⋯` keeps its corner (F8, F10).
        let action = if state.any() && !editing {
            None
        } else {
            action
        };
        let clicks = self.panel_full(ui, title, None, action, None, true, |app, ui| {
            app.style_rows(ui, &state, slot, targets, editing, live, body);
            tail(app, ui);
        });
        if clicks.more
            && let Some(under) = clicks.more_rect
        {
            // **The `⋯` toggles its menu** (§15 D1087): a second click on it
            // closes what the first opened. The menu's own press-away test spares
            // the button, so without this the click only opened it again.
            let open = self
                .style_menu
                .as_ref()
                .is_some_and(|m| m.slot == slot && m.targets == targets);
            self.style_menu = (!open).then(|| StyleMenu {
                slot,
                targets: targets.to_vec(),
                under,
            });
        }
        clicks.acted
    }

    /// The rows a style card draws for `state`: the capsule and its greyed,
    /// editing or live rows, F10's mixed capsule, or `body` plain.
    #[allow(clippy::too_many_arguments)]
    fn style_rows(
        &mut self,
        ui: &mut egui::Ui,
        state: &StyleShown,
        slot: StyleSlot,
        targets: &[NodeId],
        editing: bool,
        live: bool,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let app = self;
        match state {
            StyleShown::One(s) => {
                let Some(style) = app.session.doc.styles().get(*s).cloned() else {
                    return;
                };
                if app.style_naming.as_ref().is_some_and(|n| n.style == *s) {
                    app.naming_capsule(ui, &style);
                    return;
                }
                let open = editing || live || app.style_open.contains(&slot);
                let out = capsule(ui, &app.session.doc, &style, open, editing);
                if out.detach {
                    if editing {
                        app.session.style_edit = None;
                    } else {
                        app.detach_styles(slot, targets);
                    }
                    return;
                }
                if out.toggled && !editing && !app.style_open.remove(&slot) {
                    app.style_open.insert(slot);
                }
                if !open {
                    return;
                }
                ui.add_space(2.0);
                if editing && !live {
                    body(app, ui);
                    let n = style::users(&app.session.doc, *s).len();
                    let what = match n {
                        1 => "Changes reach its 1 layer as you type".to_string(),
                        n => format!("Changes reach its {n} layers as you type"),
                    };
                    footer(ui, icon::SLIDERS_HORIZONTAL, &what);
                } else if live {
                    // Over a selection — under an *Edit* too: what the rows
                    // write there is the one layer's spans, which `edit_through`
                    // passes by, so the edit's footer would promise a reach
                    // they do not have (`arch-scribe`'s read).
                    body(app, ui);
                    footer(ui, icon::TEXT_AA, "Styles the selection — the style stays");
                } else {
                    // Greyed and inert — the inspector's read-only door (D1038
                    // (1)); a bound variable inside still draws its chip.
                    ui.scope(|ui| {
                        ui.disable();
                        ui.set_opacity(GREYED);
                        body(app, ui);
                    });
                    footer(ui, icon::LOCK_SIMPLE, "Read-only — Detach to edit");
                }
            }
            StyleShown::Mixed(list) => mixed_capsule(ui, &app.session.doc, list),
            // Some styled and some not (F10): the rows greyed and inert, as a
            // styled card's are — the inspector is the one door that does
            // not detach (D1038 (1)); the `⋯` detaches or applies to them all.
            StyleShown::Partial => {
                ui.scope(|ui| {
                    ui.disable();
                    ui.set_opacity(GREYED);
                    body(app, ui);
                });
                footer(ui, icon::LOCK_SIMPLE, "Read-only — Detach to edit");
            }
            StyleShown::Plain => body(app, ui),
        }
    }

    /// F6's capsule: the new style's name being typed, the capsule ringed in
    /// the accent; `Enter` keeps the name, `Escape` undoes the create.
    fn naming_capsule(&mut self, ui: &mut egui::Ui, style: &Style) {
        let Some(mut n) = self.style_naming.take() else {
            return;
        };
        let (rect, _) = capsule_ground(ui, ui.id().with("style-naming"), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CAPSULE_H / 2.0, color::text_a(26));
        p.rect_stroke(
            rect,
            CAPSULE_H / 2.0,
            egui::Stroke::new(1.0, color::ACCENT.gamma_multiply(0.65)),
            egui::StrokeKind::Inside,
        );
        let mid = rect.center().y;
        let sw = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 12.0, mid - 7.0),
            egui::vec2(14.0, 14.0),
        );
        paint_preview(p, &self.session.doc, style, sw);
        // **One row tall, centred on the capsule** (§15 D1083): `put` stretches a
        // `Frame::NONE` edit over the whole box with its row at the top, so the
        // name sat high in the capsule.
        let row_h = ui.fonts_mut(|f| f.row_height(&egui::FontId::proportional(12.0)));
        let field = egui::Rect::from_min_max(
            egui::pos2(sw.right() + 8.0, mid - row_h / 2.0),
            egui::pos2(rect.right() - 12.0, mid + row_h / 2.0),
        );
        let id = ui.id().with(("style-naming-field", style.id));
        let edit = ui.put(
            field,
            egui::TextEdit::singleline(&mut n.text)
                .id(id)
                .frame(egui::Frame::NONE)
                .font(egui::FontId::proportional(12.0))
                .text_color(theme::text::STRONG),
        );
        if n.focus {
            edit.request_focus();
            n.focus = false;
        }
        let (enter, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            // `Escape` undoes (F6): the create was the last step.
            self.undo();
            return;
        }
        if let Some(why) = &n.refusal {
            ui.label(
                egui::RichText::new(why)
                    .size(10.5)
                    .color(theme::text::MUTED),
            );
        } else {
            ui.label(
                egui::RichText::new("↵ create   Esc cancel")
                    .size(10.5)
                    .color(color::text_a(115)),
            );
        }
        if enter || edit.lost_focus() {
            let text = n.text.trim().to_string();
            let taken = self
                .session
                .doc
                .styles()
                .by_name(style.slot(), &text)
                .is_some_and(|s| s.id != style.id);
            if text.is_empty() || taken {
                n.refusal = Some(if text.is_empty() {
                    "A style needs a name.".into()
                } else {
                    format!("{text} already exists.")
                });
                n.focus = true;
                self.style_naming = Some(n);
                return;
            }
            if text != style.name {
                self.commit_edit(Transaction(vec![Operation::SetStyleName {
                    id: style.id,
                    name: text,
                }]));
            }
            return;
        }
        self.style_naming = Some(n);
    }

    /// Detach `targets`' `slot` from whatever style each is bound to, their
    /// values kept — F2's chip and F4's *Detach style*.
    fn detach_styles(&mut self, slot: StyleSlot, targets: &[NodeId]) {
        let doc = &self.session.doc;
        let ops: Vec<Operation> = targets
            .iter()
            .filter(|t| doc.get(**t).is_some_and(|n| n.styles().get(slot).is_some()))
            .map(|t| style::bind_op(*t, slot, None))
            .collect();
        if !ops.is_empty() {
            self.commit_edit(Transaction(ops));
        }
    }

    /// The operations applying `s` to `targets`, and the values the commit will
    /// write beside them — what the picker's highlight previews (F5).
    fn apply_tx(
        &self,
        s: StyleId,
        slot: StyleSlot,
        targets: &[NodeId],
        preview: bool,
    ) -> Transaction {
        let mut ops: Vec<Operation> = targets
            .iter()
            .map(|t| style::bind_op(*t, slot, Some(s)))
            .collect();
        let doc = &self.session.doc;
        if preview && let Some(st) = doc.styles().get(s) {
            ops.extend(
                targets
                    .iter()
                    .filter_map(|t| doc.get(*t))
                    .flat_map(|n| st.value.write_ops(n)),
            );
        }
        Transaction(ops)
    }

    /// *Create style…* (F4, F6): the agreed list made a style under a
    /// free name, every target bound to it, and the capsule opened on its name.
    fn create_style(&mut self, slot: StyleSlot, targets: &[NodeId]) {
        let Some(&from) = targets.first() else { return };
        let base = format!("{} style", slot.title());
        let name = style::free_name(self.session.doc.styles(), slot, &base);
        let id = self.session.ids.mint_style();
        let Some(ops) = style::create_ops(&self.session.doc, id, &name, from, slot, targets) else {
            return;
        };
        if self.commit_edit(Transaction(ops)) {
            // Open, or the name is typed in a card nobody can see.
            self.open_paint_panel(slot.title());
            self.style_naming = Some(StyleNaming {
                style: id,
                text: name,
                focus: true,
                refusal: None,
            });
        }
    }

    /// The `⋯` menu and the *Apply style…* picker, drawn over everything.
    pub(crate) fn style_popups_ui(&mut self, ctx: &egui::Context) {
        // An edit whose style is gone, or none of whose users is selected any
        // more, is over.
        if let Some(s) = self.session.style_edit {
            let doc = &self.session.doc;
            let users = style::users(doc, s);
            let selected = self.session.selection.ids();
            // Every selected layer a user: one that is not would read the card
            // as *Partial* while the door still turned its writes into the
            // style's (`arch-scribe`'s read of the first cut).
            if doc.styles().get(s).is_none()
                || selected.is_empty()
                || !selected.iter().all(|n| users.contains(n))
            {
                self.session.style_edit = None;
            }
        }
        if let Some(n) = &self.style_naming
            && self.session.doc.styles().get(n.style).is_none()
        {
            self.style_naming = None;
        }
        self.style_menu_ui(ctx);
        self.style_picker_ui(ctx);
    }

    fn style_menu_ui(&mut self, ctx: &egui::Context) {
        let Some(menu) = self.style_menu.take() else {
            return;
        };
        let doc = &self.session.doc;
        let state = shown(doc, &menu.targets, menu.slot);
        let n = menu.targets.len();
        let styled = menu
            .targets
            .iter()
            .filter(|t| {
                doc.get(**t)
                    .is_some_and(|x| x.styles().get(menu.slot).is_some())
            })
            .count();
        let apply = match n {
            1 => "Apply style…".to_string(),
            n => format!("Apply style to {n} layers…"),
        };
        // Create is the agreed list's — off over several styles or a mix (F10).
        let can_create = match &state {
            StyleShown::Plain => agreed(doc, &menu.targets, menu.slot).is_some(),
            StyleShown::One(_) => true,
            StyleShown::Mixed(_) | StyleShown::Partial => false,
        };
        // Every row names how many layers it acts on (F10) — *Create* too, which
        // binds them all. One layer's rows are the maintainer's shorter words,
        // *Create style* and *Create style as a copy* (§15 D1086).
        let create = match (&state, n) {
            (StyleShown::One(_), 1) => "Create style as a copy…".to_string(),
            (StyleShown::One(_), n) => format!("Create style from these {n} as a copy…"),
            (_, 1) => "Create style…".to_string(),
            (_, n) => format!("Create style from these {n} layers…"),
        };
        let detach = match (n, styled) {
            (1, _) | (_, 0) => "Detach style".to_string(),
            (_, k) => format!("Detach style from {k} layers"),
        };
        let size = egui::vec2(MENU_W, 3.0 * ROW_H + 10.0);
        let pos = ui::menu_place(
            ctx,
            egui::pos2(menu.under.right() - MENU_W + 8.0, menu.under.bottom() + 6.0),
            size,
        );
        let mut act: Option<u8> = None;
        let rect = egui::Area::new(egui::Id::new("style-card-menu"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::menu_frame(4.0).show(ui, |ui| {
                    ui.set_width(MENU_W - 8.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    if ui::menu_row(ui, ui::MenuRow::new(icon::SWATCHES, &apply), ROW_H).clicked() {
                        act = Some(0);
                    }
                    let c = ui::menu_row(
                        ui,
                        ui::MenuRow::new(icon::PLUS_SQUARE, &create).enabled(can_create),
                        ROW_H,
                    );
                    if c.clicked() && can_create {
                        act = Some(1);
                    }
                    let d = ui::menu_row(
                        ui,
                        ui::MenuRow::new(icon::LINK_BREAK, &detach).enabled(styled > 0),
                        ROW_H,
                    );
                    if d.clicked() && styled > 0 {
                        act = Some(2);
                    }
                });
            })
            .response
            .rect;
        match act {
            Some(0) => {
                self.style_picker = Some(StylePicker {
                    slot: menu.slot,
                    targets: menu.targets,
                    under: menu.under,
                    query: String::new(),
                    focus: true,
                    highlight: 0,
                });
            }
            Some(1) => self.create_style(menu.slot, &menu.targets),
            Some(2) => self.detach_styles(menu.slot, &menu.targets),
            _ => {
                let away = ctx.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer
                            .interact_pos()
                            .is_some_and(|p| !rect.contains(p) && !menu.under.contains(p))
                });
                if !away {
                    self.style_menu = Some(menu);
                }
            }
        }
    }

    fn style_picker_ui(&mut self, ctx: &egui::Context) {
        let Some(mut pk) = self.style_picker.take() else {
            return;
        };
        let doc = &self.session.doc;
        let query = pk.query.trim().to_lowercase();
        // Grouped by the path before the last `/`, groups and rows by name (F5).
        let mut rows: Vec<&Style> = doc
            .styles()
            .of_slot(pk.slot)
            .filter(|s| query.is_empty() || s.name.to_lowercase().contains(&query))
            .collect();
        // *Kit groups last* (F5, §15 D1065): the document's own, then each kit's
        // under its name, then its id, so two kits of one name never interleave.
        let kit_of = |s: &Style| {
            s.kit()
                .map(|k| (super::binding::kit_name(doc, k), k.to_string()))
        };
        rows.sort_by(|a, b| {
            let g = |s: &Style| s.name.rsplit_once('/').map(|(g, _)| g.to_string());
            kit_of(a)
                .cmp(&kit_of(b))
                .then(g(a).cmp(&g(b)))
                .then(a.name.cmp(&b.name))
        });
        let kit_heads = rows
            .iter()
            .filter_map(|s| s.kit())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let counts: Vec<usize> = rows.iter().map(|s| style::users(doc, s.id).len()).collect();
        pk.highlight = pk.highlight.min(rows.len().saturating_sub(1));
        let (up, down, enter, escape) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        // The search holds the keyboard, so the app's `Escape` ladder never hears
        // the key while it does — the picker answers it here, in one press.
        if escape {
            self.session.clear_gesture_preview();
            return;
        }
        if down && pk.highlight + 1 < rows.len() {
            pk.highlight += 1;
        }
        if up {
            pk.highlight = pk.highlight.saturating_sub(1);
        }
        let mut pick: Option<StyleId> = enter
            .then(|| rows.get(pk.highlight).map(|s| s.id))
            .flatten();
        let groups = rows
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                let g = |s: &Style| s.name.rsplit_once('/').map(|(g, _)| g.to_string());
                *i == 0 || g(s) != g(rows[i - 1]) || s.kit() != rows[i - 1].kit()
            })
            .filter(|(_, s)| s.name.contains('/'))
            .count();
        let h = 40.0 + (rows.len().max(1) as f32) * ROW_H + (groups + kit_heads) as f32 * 22.0;
        let pos = ui::menu_place(
            ctx,
            egui::pos2(menu_left(pk.under), pk.under.bottom() + 6.0),
            egui::vec2(PICKER_W, h.min(360.0)),
        );
        let slot_word = pk.slot.title().to_lowercase();
        let rect = egui::Area::new(egui::Id::new("style-picker"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::menu_frame(4.0).show(ui, |ui| {
                    ui.set_width(PICKER_W - 8.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let search = ui::search_field(
                        ui,
                        &mut pk.query,
                        egui::Id::new("style-picker-search"),
                        &format!("Search {slot_word} styles"),
                        PICKER_W - 8.0,
                    );
                    if pk.focus {
                        search.request_focus();
                        pk.focus = false;
                    }
                    ui.add_space(4.0);
                    if rows.is_empty() {
                        let text = if doc.styles().of_slot(pk.slot).next().is_none() {
                            format!("No {slot_word} styles yet")
                        } else {
                            "Nothing matches".to_string()
                        };
                        ui.label(
                            egui::RichText::new(text)
                                .size(11.0)
                                .color(theme::text::FAINT),
                        );
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            let mut last_group: Option<(Option<&str>, Option<&str>)> = None;
                            let mut last_kit: Option<&str> = None;
                            for (i, s) in rows.iter().enumerate() {
                                let (group, leaf) = match s.name.rsplit_once('/') {
                                    Some((g, l)) => (Some(g), l),
                                    None => (None, s.name.as_str()),
                                };
                                if let Some(k) = s.kit()
                                    && last_kit != Some(k)
                                {
                                    last_kit = Some(k);
                                    super::binding::kit_header(
                                        ui,
                                        &super::binding::kit_name(doc, k),
                                        ui.available_width(),
                                    );
                                }
                                if last_group != Some((s.kit(), group)) {
                                    last_group = Some((s.kit(), group));
                                    if let Some(g) = group {
                                        let (r, _) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), 22.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().text(
                                            egui::pos2(r.left() + 8.0, r.center().y),
                                            egui::Align2::LEFT_CENTER,
                                            g.to_uppercase(),
                                            egui::FontId::proportional(9.5),
                                            theme::text::FAINT,
                                        );
                                    }
                                }
                                let (r, resp) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), ROW_H),
                                    egui::Sense::click(),
                                );
                                if resp.hovered()
                                    && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO)
                                {
                                    pk.highlight = i;
                                }
                                let p = ui.painter();
                                if i == pk.highlight {
                                    p.rect_filled(r, 4, color::text_a(23));
                                }
                                let sw = egui::Rect::from_min_size(
                                    egui::pos2(r.left() + 8.0, r.center().y - 7.0),
                                    egui::vec2(14.0, 14.0),
                                );
                                paint_preview(p, doc, s, sw);
                                p.text(
                                    egui::pos2(sw.right() + 8.0, r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    leaf,
                                    egui::FontId::proportional(11.5),
                                    theme::text::STRONG,
                                );
                                p.text(
                                    egui::pos2(r.right() - 8.0, r.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    counts[i].to_string(),
                                    egui::FontId::proportional(10.5),
                                    color::text_a(102),
                                );
                                if resp.clicked() {
                                    pick = Some(s.id);
                                }
                            }
                        });
                });
            })
            .response
            .rect;
        if let Some(s) = pick {
            self.session.clear_gesture_preview();
            let tx = self.apply_tx(s, pk.slot, &pk.targets, false);
            self.commit_edit(tx);
            // A text style applied over a live session writes its node's defaults
            // in the commit (`style::settle`), which the editor's layout cannot
            // see: re-open it on them, the editor's spans kept (§15 D1068).
            if pk.slot == StyleSlot::Text
                && self
                    .text
                    .as_ref()
                    .is_some_and(|t| pk.targets.contains(&t.id))
            {
                self.text_session_restyled();
                self.preview_session();
            }
            return;
        }
        let away = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
        });
        if away {
            self.session.clear_gesture_preview();
            return;
        }
        // The highlighted row previews on the canvas (F5).
        match rows.get(pk.highlight) {
            Some(s) => {
                let tx = self.apply_tx(s.id, pk.slot, &pk.targets, true);
                self.session.set_preview(&tx);
            }
            None => self.session.clear_gesture_preview(),
        }
        self.style_picker = Some(pk);
    }

    /// `Escape` over the styles' chrome, one thing a press: the picker, the
    /// menu, a name being typed (undone), an edit. Whether it took the key.
    pub(crate) fn escape_styles(&mut self) -> bool {
        if self.style_picker.take().is_some() {
            self.session.clear_gesture_preview();
            return true;
        }
        if self.style_menu.take().is_some() {
            return true;
        }
        if self.style_naming.take().is_some() {
            self.undo();
            return true;
        }
        if self.session.style_edit.take().is_some() {
            return true;
        }
        false
    }

    /// The browser's *Edit* (G9, §15 D1063): select the style's users and open
    /// their card on the style, live.
    pub(crate) fn edit_style(&mut self, s: StyleId) {
        let users = style::users(&self.session.doc, s);
        // A kit's style is edited in its kit (§15 D1065): read-only here, as its
        // vendored mains are.
        let kits = self
            .session
            .doc
            .styles()
            .get(s)
            .is_some_and(|st| st.origin.is_some());
        if users.is_empty() || kits {
            return;
        }
        let slot = self.session.doc.styles().get(s).map(Style::slot);
        // Every user, a text style's too: the Type card draws over several text
        // layers (§15 D1077), where it once drew for one and this selected a text
        // style's first user alone (§15 D1068).
        self.session.selection.set(users);
        self.session.style_edit = Some(s);
        if let Some(slot) = slot {
            self.style_open.insert(slot);
        }
    }
}

/// `x` to two places, its trailing zeros dropped: `1`, `1.5`, `0.25`.
fn number(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `text` laid out on one line no wider than `width`, an ellipsis where it is
/// cut.
fn elided(p: &egui::Painter, text: &str, size: f32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(egui::FontId::proportional(size), theme::text::STRONG),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    p.layout_job(job)
}

/// The picker's left edge: under the card, its right edge at the `⋯`'s.
fn menu_left(under: egui::Rect) -> f32 {
    under.right() - PICKER_W + 8.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::item::Keyed;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{Fill, IdSource, NodeKind};

    const RED: Color = Color::from_rgba8(200, 30, 30, 255);
    const GREEN: Color = Color::from_rgba8(20, 200, 40, 255);

    /// Two red 80×60 rects, the first selected.
    fn app(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x5E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let mut ops = Vec::new();
        for id in [a, b] {
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(80.0, 60.0),
                    corner_radii: RoundedRectRadii::from_single_radius(0.0),
                },
                transform: None,
                name: None,
            });
            ops.push(Operation::SetFills {
                id,
                fills: vec![Keyed::new(
                    ids.mint_item(),
                    Fill {
                        brush: Brush::Solid(RED).into(),
                        visible: true,
                    },
                )],
            });
        }
        doc.apply(&Transaction(ops)).expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a]);
        (app, a, b)
    }

    /// One frame of the inspector and the styles' popups, its shapes returned.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |_| {
            app.inspector_panel(ctx);
            app.style_popups_ui(ctx);
        })
        .shapes
    }

    fn drawn(shapes: &[egui::epaint::ClippedShape], s: &str) -> bool {
        fn find(shape: &egui::Shape, s: &str) -> bool {
            match shape {
                egui::Shape::Text(t) => t.galley.text() == s,
                egui::Shape::Vec(v) => v.iter().any(|x| find(x, s)),
                _ => false,
            }
        }
        shapes.iter().any(|c| find(&c.shape, s))
    }

    fn fill_colour(app: &OndinApp, id: NodeId) -> Brush {
        app.session.doc.get(id).unwrap().paint().fills[0]
            .brush
            .literal()
            .cloned()
            .unwrap()
    }

    fn fill_style(app: &OndinApp, id: NodeId) -> Option<StyleId> {
        app.session.doc.get(id).unwrap().styles().fills
    }

    /// **A styled Fill card is its capsule, and *Detach* keeps the values**
    /// (F1, F2): after *Create style…*, the card draws the style's
    /// name and its summary where the rows were, and detaching leaves the red.
    #[test]
    fn a_styled_card_draws_its_capsule_and_detaching_keeps_its_values() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        app.create_style(StyleSlot::Fills, &[a]);
        let s = fill_style(&app, a).expect("bound by the create");
        assert_eq!(app.session.doc.styles().get(s).unwrap().name, "Fill style");
        app.style_naming = None;
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = frame(&ctx, &mut app, vec![]);
        }
        assert!(drawn(&shapes, "1 fill"), "the capsule's summary is drawn");
        app.detach_styles(StyleSlot::Fills, &[a]);
        assert_eq!(fill_style(&app, a), None);
        assert_eq!(fill_colour(&app, a), Brush::Solid(RED));
        // Asked for, so no status (F2: *no toast*).
        assert!(!app.session.status().text.starts_with("Detached"));
        let shapes = frame(&ctx, &mut app, vec![]);
        assert!(!drawn(&shapes, "1 fill"), "the plain card is back");
    }

    /// **The Container card is a style card** (§15 D1075, D1076, F7): a column
    /// frame's layout made a style through *Create style…*, the card
    /// draws its capsule — *Column · 6* — where the rows were; applied to a plain
    /// frame it gives it the column; detached, the frame keeps it.
    ///
    /// Flip, run: `inspector_container` drawn through `self.panel` again (no
    /// `style_card`) fails *"the capsule's summary is drawn"*.
    #[test]
    fn the_container_card_is_a_layout_style_card() {
        use ondin_core::container::{Display, Flex, FlexDirection};
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0x5F);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let mut ops = Vec::new();
        for (i, id) in [a, b].into_iter().enumerate() {
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Artboard {
                    size: Size::new(120.0, 120.0),
                },
                transform: None,
                name: None,
            });
        }
        ops.push(Operation::SetDisplay {
            id: a,
            display: Some(Display::Flex(Flex {
                direction: FlexDirection::Column,
                row_gap: 6.0,
                ..Default::default()
            })),
        });
        doc.apply(&Transaction(ops)).expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a]);
        let layout = |app: &OndinApp, id| app.session.doc.get(id).unwrap().styles().layout;

        app.create_style(StyleSlot::Layout, &[a]);
        let s = layout(&app, a).expect("bound by the create");
        app.style_naming = None;
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = frame(&ctx, &mut app, vec![]);
        }
        assert!(
            drawn(&shapes, "Column · 6"),
            "the capsule's summary is drawn"
        );

        assert!(app.commit_edit(Transaction(vec![style::bind_op(
            b,
            StyleSlot::Layout,
            Some(s)
        )])));
        assert_eq!(
            app.session.doc.get(b).unwrap().display(),
            app.session.doc.get(a).unwrap().display(),
            "b takes the column"
        );
        app.detach_styles(StyleSlot::Layout, &[b]);
        assert_eq!(layout(&app, b), None);
        assert!(
            app.session.doc.get(b).unwrap().display().is_some(),
            "and keeps it"
        );
    }

    /// **`Escape` while the new style's name is typed undoes the create** (F6).
    #[test]
    fn escape_on_a_new_styles_name_undoes_the_create() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        app.create_style(StyleSlot::Fills, &[a]);
        assert!(app.style_naming.is_some());
        assert!(app.escape_styles());
        assert!(app.session.doc.styles().is_empty(), "the create is undone");
        assert_eq!(fill_style(&app, a), None);
    }

    /// **The browser's *Edit* makes a card's write an edit of the style, which
    /// reaches every user** (§15 D1063). Outside the edit the same write detaches
    /// the one layer.
    ///
    /// Flip, run: the commit door's `edit_through` dropped fails at *"b follows
    /// the edit"* — `a` detached and green, `b` still the style's red.
    #[test]
    fn an_edit_of_a_styles_user_edits_the_style() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app(&ctx);
        app.create_style(StyleSlot::Fills, &[a, b]);
        app.style_naming = None;
        let s = fill_style(&app, a).unwrap();
        app.edit_style(s);
        assert_eq!(app.session.selection.ids().len(), 2, "its users selected");
        let mut fills = app.session.doc.get(a).unwrap().paint().fills.clone();
        fills[0].value.brush = Brush::Solid(GREEN).into();
        assert!(app.commit_edit(Transaction(vec![Operation::SetFills {
            id: a,
            fills: fills.clone(),
        }])));
        assert_eq!(
            fill_colour(&app, b),
            Brush::Solid(GREEN),
            "b follows the edit"
        );
        assert_eq!(fill_style(&app, a), Some(s), "a stays styled");
        // A selection holding a layer that does not use the style ends the edit.
        let c = app.session.ids.mint();
        let root = app.session.doc.root();
        assert!(app.commit_edit(Transaction(vec![Operation::CreateNode {
            id: c,
            parent: root,
            index: 0,
            kind: NodeKind::Ellipse {
                size: Size::new(10.0, 10.0),
            },
            transform: None,
            name: None,
        }])));
        app.session.selection.set(vec![a, b, c]);
        frame(&ctx, &mut app, vec![]);
        assert_eq!(app.session.style_edit, None, "a non-user selected ends it");
        fills[0].value.brush = Brush::Solid(RED).into();
        app.commit_edit(Transaction(vec![Operation::SetFills { id: a, fills }]));
        assert_eq!(fill_style(&app, a), None);
        assert_eq!(fill_colour(&app, b), Brush::Solid(GREEN));
        // And says so, as a door's detach does (D1038 (2)).
        assert_eq!(app.session.status().text, "Detached Fill style");
    }

    /// **`Escape` closes the picker in one press, its preview with it**, though
    /// its search holds the keyboard the app's ladder would otherwise answer.
    #[test]
    fn escape_closes_the_picker_and_its_preview() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app(&ctx);
        app.create_style(StyleSlot::Fills, &[a]);
        app.style_naming = None;
        app.style_picker = Some(StylePicker {
            slot: StyleSlot::Fills,
            targets: vec![b],
            under: egui::Rect::from_min_size(egui::pos2(900.0, 100.0), egui::vec2(20.0, 20.0)),
            query: String::new(),
            focus: true,
            highlight: 0,
        });
        frame(&ctx, &mut app, vec![]);
        assert!(app.session.has_gesture_preview());
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        frame(&ctx, &mut app, vec![escape]);
        assert!(app.style_picker.is_none(), "one press closes it");
        assert!(!app.session.has_gesture_preview(), "and its preview");
        assert_eq!(fill_style(&app, b), None, "nothing applied");
    }

    /// **The picker's highlight previews on the canvas and `Enter` applies it**
    /// (F5).
    #[test]
    fn the_pickers_highlight_previews_and_enter_applies() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app(&ctx);
        app.create_style(StyleSlot::Fills, &[a]);
        app.style_naming = None;
        let s = fill_style(&app, a).unwrap();
        app.style_picker = Some(StylePicker {
            slot: StyleSlot::Fills,
            targets: vec![b],
            under: egui::Rect::from_min_size(egui::pos2(900.0, 100.0), egui::vec2(20.0, 20.0)),
            query: String::new(),
            focus: true,
            highlight: 0,
        });
        frame(&ctx, &mut app, vec![]);
        assert!(app.session.has_gesture_preview(), "the highlight previews");
        assert_eq!(fill_style(&app, b), None, "nothing committed yet");
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        frame(&ctx, &mut app, vec![enter]);
        assert_eq!(fill_style(&app, b), Some(s));
        assert!(!app.session.has_gesture_preview());
        assert!(app.style_picker.is_none());
    }

    /// Two text layers, `a` at 40px and `b` at 12px, `a` selected.
    fn text_app(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x5F);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let ops = [(a, 40.0, "Heading"), (b, 12.0, "Some body text")]
            .into_iter()
            .map(|(id, size, content)| Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Text {
                    content: content.into(),
                    style: Box::new(ondin_core::TextStyle {
                        font_size: size,
                        ..Default::default()
                    }),
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
            })
            .collect();
        doc.apply(&Transaction(ops)).expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![a]);
        (app, a, b)
    }

    /// `frame` on a screen tall enough for the Type card's footer: a `Label`
    /// below the visible area paints nothing, and a text layer's cards run long.
    fn tall_frame(ctx: &egui::Context, app: &mut OndinApp) -> Vec<egui::epaint::ClippedShape> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 2400.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |_| {
            app.inspector_panel(ctx);
            app.style_popups_ui(ctx);
        })
        .shapes
    }

    fn font_size(app: &OndinApp, id: NodeId) -> f64 {
        match app.session.doc.get(id).unwrap().kind() {
            NodeKind::Text { style, .. } => style.font_size,
            _ => panic!("text"),
        }
    }

    /// **A styled Type card is read-only, and live over a session's selection**
    /// (§15 D1068): what its rows write there is spans, which keep the style
    /// (§15 D1067), so the one door that does not detach still does not.
    ///
    /// Flip, run: `style_card_live` called with `live: false` fails at *"live over
    /// a selection"* — the selection's card is the read-only one.
    #[test]
    fn a_styled_type_card_is_read_only_and_live_over_a_selection() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = text_app(&ctx);
        app.create_style(StyleSlot::Text, &[a]);
        app.style_naming = None;
        assert!(app.session.doc.get(a).unwrap().styles().text.is_some());
        // Three passes, as the fill capsule's test: the first lays out no text.
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = frame(&ctx, &mut app, vec![]);
        }
        assert!(drawn(&shapes, "Type style"), "the capsule names the style");
        assert!(drawn(&shapes, "Inter · 40"), "its summary");
        app.style_open.insert(StyleSlot::Text);
        let shapes = tall_frame(&ctx, &mut app);
        assert!(drawn(&shapes, "Read-only — Detach to edit"));

        app.begin_edit_text(Some(a), None);
        app.text
            .as_mut()
            .expect("a live session")
            .editor
            .select_all();
        let shapes = tall_frame(&ctx, &mut app);
        assert!(
            drawn(&shapes, "Styles the selection — the style stays"),
            "live over a selection"
        );
        assert!(!drawn(&shapes, "Read-only — Detach to edit"));
    }

    /// **A styled Type card leaves live what its style does not hold** (§15
    /// D1068's amendment): the alignment row and the popup's opener sit in the
    /// card's tail, drawn under a collapsed capsule too, so the popup stays open
    /// for its Paragraph and Box tabs — and its Character tab, the style's, is
    /// read-only.
    ///
    /// Flip, run: the alignment row moved back into the card's `body` fails at
    /// *"the popup outlives the collapse"* — a collapsed capsule draws no opener,
    /// and `type_menu_popup` closes a popup with none.
    #[test]
    fn a_styled_type_card_leaves_its_paragraph_live_and_locks_the_character_tab() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = text_app(&ctx);
        app.create_style(StyleSlot::Text, &[a]);
        app.style_naming = None;
        app.type_menu = Some(crate::app::TypeTab::Paragraph);
        for _ in 0..3 {
            tall_frame(&ctx, &mut app);
        }
        assert!(
            app.type_menu.is_some(),
            "the popup outlives the collapse: the opener is in the tail"
        );
        app.type_menu = Some(crate::app::TypeTab::Character);
        let shapes = tall_frame(&ctx, &mut app);
        let note = "The text style holds the greyed rows. Detach to edit";
        assert!(drawn(&shapes, note), "the Character tab is the style's");

        app.begin_edit_text(Some(a), None);
        app.text
            .as_mut()
            .expect("a live session")
            .editor
            .select_all();
        let shapes = tall_frame(&ctx, &mut app);
        assert!(!drawn(&shapes, note), "and live over a selection");
    }

    /// **A text style applied over a live session re-opens its editor** on the
    /// defaults the commit wrote (`style::settle`), as a Type reset does
    /// (`a_type_reset_restyles_a_live_text_session`): the editor's own layout
    /// is what the next keystroke draws, and the document cannot see it.
    ///
    /// Flip, run: the picker's `text_session_restyled` dropped fails at *"the
    /// editor lays out at 40"* — the document is 40 and the editor still 12.
    #[test]
    fn applying_a_text_style_over_a_live_session_restyles_its_editor() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = text_app(&ctx);
        app.create_style(StyleSlot::Text, &[a]);
        app.style_naming = None;
        app.session.selection.set(vec![b]);
        app.begin_edit_text(Some(b), None);
        let height = |app: &OndinApp| app.text.as_ref().unwrap().editor.text_layout().size.height;
        let before = height(&app);
        app.style_picker = Some(StylePicker {
            slot: StyleSlot::Text,
            targets: vec![b],
            under: egui::Rect::from_min_size(egui::pos2(900.0, 100.0), egui::vec2(20.0, 20.0)),
            query: String::new(),
            focus: true,
            highlight: 0,
        });
        frame(&ctx, &mut app, vec![]);
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        frame(&ctx, &mut app, vec![enter]);
        assert_eq!(font_size(&app, b), 40.0, "the document took the style");
        assert!(
            height(&app) > before * 2.0,
            "the editor lays out at 40: {} against {before}",
            height(&app)
        );
    }

    /// **A stroke style's bound width reads as drawn** in its summary — `4 · …`
    /// for a width bound to a variable of 4, not the literal's `0` (§15 D1073's
    /// reading (e), fixed in its third amendment). The ring reads the same
    /// `width_shown`, untested — it is pixels.
    ///
    /// Flip: `width_shown` returning `s.width` for a bound width fails on the
    /// summary, at `0 · center`.
    #[test]
    fn a_stroke_styles_bound_width_reads_as_drawn() {
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let ctx = egui::Context::default();
        let (mut app, _, _) = app(&ctx);
        let mode = app.session.ids.mint_mode();
        let v = app.session.ids.mint_var();
        assert!(app.session.commit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: v,
                    name: "stroke/thick".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(4.0))].into(),
                    origin: None,
                    forked: None,
                },
            },
        ])));
        let stroke = ondin_core::Stroke {
            width: 0.0,
            width_var: Some(v),
            ..Default::default()
        };
        let value = StyleValue::Strokes(vec![Keyed::new(app.session.ids.mint_item(), stroke)]);
        let text = summary(&app.session.doc, &value);
        assert!(text.starts_with("4 · "), "summary {text:?}");
    }

    /// **Two layers bound to different family variables do not agree** (§15
    /// D1077), so *Create* is off over them; bound to the same one, they do.
    ///
    /// Both store the empty literal a bound family keeps, so compared on the
    /// literal they agreed, and the style — made from the first layer's family as
    /// drawn — rewrote the second one's *Georgia* to *Inter*.
    ///
    /// Flip: `agreed` reading `StyleValue::of(n, slot)` again fails on
    /// *"Inter and Georgia agreed"*.
    #[test]
    fn layers_bound_to_different_families_do_not_agree() {
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let ctx = egui::Context::default();
        let (mut app, a, b) = text_app(&ctx);
        let mode = app.session.ids.mint_mode();
        let (inter, georgia) = (app.session.ids.mint_var(), app.session.ids.mint_var());
        let var = |id, name: &str, family: &str| Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::String,
                cells: [(mode, Cell::String(family.into()))].into(),
                origin: None,
                forked: None,
            },
        };
        assert!(app.session.commit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            var(inter, "font/ui", "Inter"),
            var(georgia, "font/serif", "Georgia"),
        ])));
        // The same size on both, so the family is the one difference.
        let size = Transaction(
            [a, b]
                .into_iter()
                .map(|id| Operation::SetTextStyle {
                    id,
                    style: ondin_core::TextStyle {
                        font_size: 16.0,
                        ..Default::default()
                    },
                    spans: None,
                })
                .collect(),
        );
        assert!(app.session.commit(size));
        for (id, v) in [(a, inter), (b, georgia)] {
            let ops = ondin_core::build::bind_text_ops(&app.session.doc, id, [None, Some(v)]);
            assert!(app.session.commit(Transaction(ops)));
        }
        let doc = &app.session.doc;
        assert!(
            agreed(doc, &[a, b], StyleSlot::Text).is_none(),
            "Inter and Georgia agreed"
        );
        let ops = ondin_core::build::bind_text_ops(&app.session.doc, b, [None, Some(inter)]);
        assert!(app.session.commit(Transaction(ops)));
        assert!(
            agreed(&app.session.doc, &[a, b], StyleSlot::Text).is_some(),
            "two layers on one family disagreed"
        );
    }

    /// **A text style's *Edit* selects every user**, as the paint styles' does
    /// — the Type card draws over several text layers (§15 D1077), where it once
    /// drew for one and this selected the first user alone (§15 D1068) — and the
    /// several-layer card's write goes into the style, both users still bound.
    ///
    /// Flip: `edit_style` selecting `users.into_iter().take(1)` again fails on
    /// *"every user selected"*.
    #[test]
    fn a_text_styles_edit_selects_every_user_and_writes_the_style() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = text_app(&ctx);
        app.create_style(StyleSlot::Text, &[a, b]);
        app.style_naming = None;
        let s = app.session.doc.get(a).unwrap().styles().text.unwrap();
        assert_eq!(font_size(&app, b), 40.0, "the fixture: b took the style");
        app.edit_style(s);
        let selected = app.session.selection.ids().to_vec();
        assert_eq!(selected.len(), 2, "every user selected");
        let subject =
            super::super::TypeSubject::several(&app, &selected).expect("a several-layer subject");
        app.apply_char_attrs(&subject, vec![ondin_core::CharAttr::Size(20.0)]);
        assert_eq!(font_size(&app, a), 20.0);
        assert_eq!(font_size(&app, b), 20.0);
        for id in [a, b] {
            assert_eq!(app.session.doc.get(id).unwrap().styles().text, Some(s));
        }
        match &app.session.doc.styles().get(s).unwrap().value {
            ondin_core::style::StyleValue::Text(t) => assert_eq!(t.font_size, 20.0),
            other => panic!("not a text style: {other:?}"),
        }
    }

    /// Where `s` is drawn: the centre of its galley.
    fn text_at(shapes: &[egui::epaint::ClippedShape], s: &str) -> Option<egui::Pos2> {
        fn find(shape: &egui::Shape, s: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == s => {
                    Some(t.pos + t.galley.rect.center().to_vec2())
                }
                egui::Shape::Vec(v) => v.iter().find_map(|x| find(x, s)),
                _ => None,
            }
        }
        shapes.iter().find_map(|c| find(&c.shape, s))
    }

    /// **Image editing's card is read-only under a styled Fill card** (§15
    /// D1080, D1038 (1)): the card is an `Area`, which inherits neither the
    /// styled rows' disable nor their grey, so its every control wrote the fills
    /// and the commit detached the style. A picture fill made a style, its
    /// capsule open, image editing entered, and the card's *Fit* segment
    /// clicked: the style stays and the picture keeps its mode.
    ///
    /// Flip, run: the card's `if locked { ui.disable(); … }` taken out fails at
    /// *"the style stays"*, the click having written *Fit* and cut the style.
    #[test]
    fn the_image_card_is_read_only_under_a_styled_fill() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        let bytes = vec![7u8; 8];
        let image = crate::app::content_hash(&bytes);
        let item = app.session.doc.get(a).unwrap().paint().fills[0].id;
        assert!(app.session.commit(Transaction(vec![
            Operation::AddImage {
                id: image.clone(),
                entry: ondin_core::ImageEntry {
                    source: ondin_core::ImageSource::Embedded(bytes.into()),
                    format: ondin_core::ImageFormat::Png,
                    width: 4,
                    height: 4,
                },
            },
            Operation::SetFills {
                id: a,
                fills: vec![Keyed::new(
                    item,
                    Fill {
                        brush: ondin_core::image_brush(image).into(),
                        visible: true,
                    },
                )],
            },
        ])));
        app.create_style(StyleSlot::Fills, &[a]);
        app.style_naming = None;
        let s = fill_style(&app, a).expect("the fixture: styled");
        app.style_open.insert(StyleSlot::Fills);
        app.begin_image_edit(a);
        assert_eq!(app.edited_image(), Some((a, 0)), "the fixture: editing");
        let mode = |app: &OndinApp| match app.session.doc.get(a).unwrap().paint().fills[0]
            .brush
            .literal()
        {
            Some(Brush::Image(img)) => img.image.fit,
            other => panic!("a picture, got {other:?}"),
        };
        let before = mode(&app);
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = tall_frame(&ctx, &mut app);
        }
        let at = text_at(&shapes, "Fit").expect("the card's mode strip is drawn");
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        // On the screen the strip was found on: the card hangs off its row, and
        // a shorter screen lays the panel out elsewhere.
        let tall = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 2400.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |_| {
                app.inspector_panel(&ctx);
                app.style_popups_ui(&ctx);
            });
        };
        tall(&mut app, vec![egui::Event::PointerMoved(at)]);
        tall(&mut app, vec![button(true)]);
        tall(&mut app, vec![button(false)]);
        tall(&mut app, vec![]);
        assert_eq!(fill_style(&app, a), Some(s), "the style stays");
        assert_eq!(mode(&app), before, "and the picture keeps its mode");
    }
}
