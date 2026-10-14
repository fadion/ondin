//! Binding an inspector field to a variable (§5.3f, D1038 A1–D3; §15 D1041).
//!
//! **The chip** — a bound field's value is a capsule naming its variable at the
//! field's own width, the only fully round thing in the inspector (*the capsule
//! rule*), with no hue of its own: the accent only while it holds the keyboard.
//! Its tooltip is the resolved value and the mode it resolved in; a hover ×
//! detaches; focused, `Backspace` detaches and any other key replaces it.
//!
//! **The `--` list** — typing `--` into a bindable field opens a list of the
//! variables of the field's kind under it, filtered by what follows; `↑`/`↓`
//! move the highlight, which previews on the canvas, `Enter` binds and `Escape`
//! restores. One list in the whole app, keyed by the field it is under.
//!
//! **The field menu** (`context-menus.md` §6.7) — *Bind to variable…*, *Extract
//! into variable…* (disabled when bound), *Detach variable* (omitted when not).

use crate::app::OndinApp;
use crate::panels::PaintSlot;
use crate::theme::{self, color, icon};
use crate::ui;
use eframe::egui;
use ondin_core::variable::{Cell, Mode, Value, VarId, VarKind, Variable};
use ondin_core::{NodeId, Operation, PaintTarget, Transaction};

/// A field that can be bound, and where its binding writes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FieldKey {
    /// A fill's or a stroke's colour: `slot` on node `id` — one of its own
    /// (`Fill`/`Stroke`) or a row of the selection's shared list (`Selection`).
    Paint { id: NodeId, slot: PaintSlot },
    /// A rectangle's corner radius, all four corners, on node `id`.
    Radius { id: NodeId },
    /// One corner of a rectangle's radius, `slot` its place in a `CornerVars`
    /// (TL, TR, BR, BL) — a per-corner field (§15 D1049).
    Corner { id: NodeId, slot: usize },
}

impl FieldKey {
    /// The node the field belongs to — where its binding resolves, in the mode
    /// in force there.
    pub(crate) fn at(self) -> NodeId {
        match self {
            FieldKey::Paint { id, .. } | FieldKey::Radius { id } | FieldKey::Corner { id, .. } => {
                id
            }
        }
    }

    /// The glyph a radius field's badge carries: the corners icon for all four,
    /// the elbow pointing into its corner for one — the per-corner fields' own
    /// (§15 D1049). `CORNERS_OUT` for a paint, which draws no badge.
    pub(crate) fn badge(self) -> &'static str {
        match self {
            FieldKey::Corner { slot: 0, .. } => icon::ARROW_ELBOW_LEFT_DOWN,
            FieldKey::Corner { slot: 1, .. } => icon::ARROW_ELBOW_RIGHT_DOWN,
            FieldKey::Corner { slot: 2, .. } => icon::ARROW_ELBOW_RIGHT_UP,
            FieldKey::Corner { .. } => icon::ARROW_ELBOW_LEFT_UP,
            FieldKey::Radius { .. } | FieldKey::Paint { .. } => icon::CORNERS_OUT,
        }
    }

    pub(crate) fn kind(self) -> VarKind {
        match self {
            FieldKey::Paint { .. } => VarKind::Color,
            FieldKey::Radius { .. } | FieldKey::Corner { .. } => VarKind::Number,
        }
    }
}

/// The `--` list while it is open.
#[derive(Clone, Debug)]
pub(crate) struct Complete {
    pub(crate) key: FieldKey,
    /// What follows the `--`.
    pub(crate) query: String,
    pub(crate) highlight: usize,
    /// The field it hangs under, as last drawn.
    pub(crate) under: egui::Rect,
    /// Whether it was opened from the field menu rather than by typing — then the
    /// list carries a search field of its own, the field itself not being typed
    /// into.
    pub(crate) own_search: bool,
    /// Whether the search should take the keyboard next frame.
    pub(crate) focus: bool,
    /// The frame this list was last asked for. A typing field asks every frame it
    /// holds `--`; a list nobody asked for since the frame before closes.
    pub(crate) seen: u64,
}

/// A bound field's chip text, re-typed: what replaces the chip once a key other
/// than `Backspace` or `--` lands on it (D1038 A3).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Retype {
    pub(crate) key: FieldKey,
    pub(crate) text: String,
    pub(crate) focus: bool,
}

/// What a chip was asked this frame.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ChipOut {
    pub(crate) detach: bool,
    /// A key typed onto the focused chip — the start of a literal, or of `--`.
    pub(crate) typed: Option<String>,
    pub(crate) menu: Option<egui::Pos2>,
}

/// The chip's capsule: 20 tall, fully round (D1038 A1).
const CHIP_H: f32 = 20.0;
const LIST_ROW_H: f32 = 26.0;
const LIST_W: f32 = 256.0;

/// Draw a bound field's chip in `rect` — the field's value area — and read what
/// it is asked. `name` is the variable's full name, `tip` the resolved value and
/// mode, `x` whether the hover × is offered (a chip in a mixed row has none).
pub(crate) fn chip(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    name: &str,
    tip: &str,
    x: bool,
) -> ChipOut {
    let mut out = ChipOut::default();
    let cap = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.center().y - CHIP_H / 2.0),
        egui::pos2(rect.right(), rect.center().y + CHIP_H / 2.0),
    );
    let resp = ui.interact(cap, id, egui::Sense::click());
    let focused = resp.has_focus();
    if resp.clicked() {
        resp.request_focus();
    }
    let hovered = resp.hovered();
    let p = ui.painter();
    let (fill, ink) = if focused {
        (color::ACCENT.gamma_multiply(0.28), color::ACCENT_100)
    } else if hovered {
        (color::text_a(43), color::TEXT)
    } else {
        (color::text_a(28), color::TEXT)
    };
    p.rect_filled(cap, CHIP_H / 2.0, fill);
    // The hover ×, in the chip's last 14 points (A7).
    let xr = egui::Rect::from_center_size(
        egui::pos2(cap.right() - 3.0 - 7.0, cap.center().y),
        egui::vec2(14.0, 14.0),
    );
    let show_x = x && (hovered || ui.rect_contains_pointer(xr));
    let text_right = if show_x {
        xr.left() - 4.0
    } else {
        cap.right() - 8.0
    };
    let font = egui::FontId::proportional(11.5);
    let shown = elide_groups(ui, name, &font, text_right - (cap.left() + 8.0));
    p.with_clip_rect(egui::Rect::from_x_y_ranges(
        cap.left()..=text_right,
        cap.y_range(),
    ))
    .text(
        egui::pos2(cap.left() + 8.0, cap.center().y + 1.0),
        egui::Align2::LEFT_CENTER,
        shown,
        font,
        ink,
    );
    if show_x {
        let xresp = ui.interact(xr, id.with("x"), egui::Sense::click());
        ui.painter().circle_filled(
            xr.center(),
            7.0,
            if xresp.hovered() {
                color::text_a(70)
            } else {
                color::text_a(46)
            },
        );
        ui.painter().text(
            xr.center(),
            egui::Align2::CENTER_CENTER,
            icon::X,
            egui::FontId::new(9.0, egui::FontFamily::Name("phosphor".into())),
            color::TEXT,
        );
        if xresp.on_hover_text("Detach variable").clicked() {
            out.detach = true;
        }
    }
    if resp.secondary_clicked() {
        out.menu = resp.interact_pointer_pos();
    }
    resp.on_hover_text(tip);
    if focused {
        // The keys a focused chip takes for itself — read and consumed here, so
        // the keymap never sees a `Backspace` that would delete the layer (an
        // egui focus also holds the keymap off, `input::resolve`).
        let (back, typed) = ui.input_mut(|i| {
            let back = i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Delete);
            let mut typed = String::new();
            i.events.retain(|e| match e {
                egui::Event::Text(t) => {
                    typed.push_str(t);
                    false
                }
                _ => true,
            });
            (back, typed)
        });
        if back {
            out.detach = true;
        } else if !typed.is_empty() {
            out.typed = Some(typed);
        }
    }
    out
}

/// `name` fitted to `width`: whole groups cut from the left first —
/// `…/width-collapsed` — then the leaf, with an ellipsis (D1038 A5).
pub(crate) fn elide_groups(ui: &egui::Ui, name: &str, font: &egui::FontId, width: f32) -> String {
    let fits = |s: &str| {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(s.to_string(), font.clone(), color::TEXT)
                .size()
                .x
        }) <= width
    };
    if fits(name) {
        return name.to_string();
    }
    let parts: Vec<&str> = name.split('/').collect();
    for cut in 1..parts.len() {
        let s = format!("…/{}", parts[cut..].join("/"));
        if fits(&s) {
            return s;
        }
    }
    let leaf = parts.last().copied().unwrap_or(name);
    let mut chars: Vec<char> = leaf.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s = format!("{}…", chars.iter().collect::<String>());
        if fits(&s) {
            return s;
        }
    }
    "…".into()
}

/// A resolved value and the mode it resolved in, for a chip's tooltip:
/// `16 · Dark`, `8BA5E0 · Light` — the full name first when the chip elides it.
pub(crate) fn chip_tip(name: &str, value: Option<Value>, mode: Option<&Mode>) -> String {
    let v = match value {
        Some(Value::Color(c)) => ui::hex_of(c),
        Some(Value::Number(x)) => trim_number(x),
        None => "—".into(),
    };
    match mode {
        Some(m) => format!("{name} · {v} · {}", m.name),
        None => format!("{name} · {v}"),
    }
}

fn trim_number(x: f64) -> String {
    let r = (x * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r}")
    }
}

/// The variables `--query` lists for a field of `kind`: a substring match on the
/// full name, grouped by group in name order (B1).
pub(crate) fn matches<'a>(
    table: &'a ondin_core::Variables,
    kind: VarKind,
    query: &str,
) -> Vec<&'a Variable> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<&Variable> = table
        .iter()
        .filter(|v| v.kind == kind && (q.is_empty() || v.name.to_lowercase().contains(&q)))
        .collect();
    out.sort_by(|a, b| {
        let (ga, _) = ondin_core::variable::split_name(&a.name);
        let (gb, _) = ondin_core::variable::split_name(&b.name);
        ga.cmp(gb).then(a.name.cmp(&b.name))
    });
    out
}

/// B4's refusal (§15 D1044): a field's buffer, `--` and all, that names a whole
/// variable of `kind` and then goes on — `--space/4 * 2` — when a variable fills
/// the whole field, a live relationship breaching §13 decision 1 (D1037). The
/// variable, and the byte of `text` where the rest begins.
///
/// `None` while anything still matches the query, so a name that is the start of
/// a longer one (`brand/50` of `brand/500`) is still being typed rather than
/// refused; the longest whole name wins.
pub(crate) fn refused<'a>(
    table: &'a ondin_core::Variables,
    kind: VarKind,
    text: &str,
) -> Option<(&'a Variable, usize)> {
    let q = text.strip_prefix("--")?;
    if !matches(table, kind, q).is_empty() {
        return None;
    }
    table
        .iter()
        .filter(|v| v.kind == kind)
        .filter(|v| {
            q.get(..v.name.len())
                .is_some_and(|head| head.to_lowercase() == v.name.to_lowercase())
                && !q[v.name.len()..].trim().is_empty()
        })
        .max_by_key(|v| v.name.len())
        .map(|v| (v, 2 + v.name.len()))
}

/// A bindable field's text laid out with B4's refused tail — from byte `at` on —
/// dimmed to 40%: the layouter every `--`-taking `TextEdit` shares. With no
/// refusal it is the plain line `TextEdit` would have laid out itself, in the
/// colour it would have chosen.
pub(crate) fn refusal_galley(
    ui: &egui::Ui,
    text: &str,
    at: Option<usize>,
    font: &egui::FontId,
) -> std::sync::Arc<egui::Galley> {
    let ink = ui
        .visuals()
        .override_text_color
        .unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
    let mut job = egui::text::LayoutJob::default();
    match at.filter(|a| text.is_char_boundary(*a)) {
        Some(a) => {
            job.append(&text[..a], 0.0, egui::TextFormat::simple(font.clone(), ink));
            job.append(
                &text[a..],
                0.0,
                egui::TextFormat::simple(font.clone(), color::text_a(102)),
            );
        }
        None => job.append(text, 0.0, egui::TextFormat::simple(font.clone(), ink)),
    }
    job.wrap.max_width = f32::INFINITY;
    ui.fonts_mut(|f| f.layout_job(job))
}

/// B4's dotted underline under a field's refused tail, from byte `at` to the end
/// of the text the field drew — egui underlines solid only, so it is painted.
pub(crate) fn underline_refusal(
    ui: &egui::Ui,
    out: &egui::text_edit::TextEditOutput,
    at: Option<usize>,
) {
    let text = out.galley.text();
    let Some(a) = at.filter(|a| text.is_char_boundary(*a) && *a < text.len()) else {
        return;
    };
    let x = |chars: usize| {
        out.galley
            .pos_from_cursor(egui::text::CCursor::new(chars))
            .min
            .x
    };
    let (x0, x1) = (x(text[..a].chars().count()), x(text.chars().count()));
    let y = out.galley_pos.y + out.galley.rect.bottom() + 0.5;
    ui.painter()
        .with_clip_rect(out.text_clip_rect)
        .extend(egui::Shape::dotted_line(
            &[
                egui::pos2(out.galley_pos.x + x0, y),
                egui::pos2(out.galley_pos.x + x1, y),
            ],
            color::text_a(102),
            3.0,
            0.6,
        ));
}

impl OndinApp {
    /// The nodes a field's binding writes to.
    fn field_ids(&self, key: FieldKey) -> Vec<NodeId> {
        match key {
            FieldKey::Paint {
                slot: PaintSlot::Selection(..),
                ..
            } => self.session.selection.ids().to_vec(),
            FieldKey::Paint { id, .. } | FieldKey::Radius { id } | FieldKey::Corner { id, .. } => {
                vec![id]
            }
        }
    }

    /// The transaction that binds `key` to `var`.
    pub(crate) fn bind_tx(&self, key: FieldKey, var: VarId) -> Transaction {
        let doc = &self.session.doc;
        let ids = self.field_ids(key);
        match key {
            FieldKey::Paint { slot, .. } => match paint_item(slot) {
                Some((target, index)) => {
                    ondin_core::bind_brush_at_all(doc, &ids, target, index, var)
                }
                None => Transaction::default(),
            },
            FieldKey::Radius { id } => {
                Transaction(ondin_core::bind_corners_ops(doc, id, [Some(var); 4]))
            }
            // One corner: `bind_corners_ops` leaves a `None` slot as it is.
            FieldKey::Corner { id, slot } => {
                let mut vars = [None; 4];
                vars[slot] = Some(var);
                Transaction(ondin_core::bind_corners_ops(doc, id, vars))
            }
        }
    }

    /// The transaction that detaches `key` to the value it draws.
    pub(crate) fn detach_tx(&self, key: FieldKey) -> Transaction {
        let doc = &self.session.doc;
        let ids = self.field_ids(key);
        match key {
            FieldKey::Paint { slot, .. } => match paint_item(slot) {
                Some((target, index)) => ondin_core::detach_brush_at_all(doc, &ids, target, index),
                None => Transaction::default(),
            },
            FieldKey::Radius { id } => Transaction(ondin_core::detach_corners_ops(doc, id)),
            FieldKey::Corner { id, slot } => {
                Transaction(ondin_core::detach_corner_ops(doc, id, slot))
            }
        }
    }

    /// Bind `key` to `var`, closing the list.
    pub(crate) fn bind_field(&mut self, key: FieldKey, var: VarId) {
        self.var_complete = None;
        self.var_retype = None;
        self.session.clear_gesture_preview();
        let tx = self.bind_tx(key, var);
        self.commit_edit(tx);
    }

    /// Detach `key`, which then reads the value it resolved to.
    pub(crate) fn detach_field(&mut self, key: FieldKey) {
        let tx = self.detach_tx(key);
        self.commit_edit(tx);
    }

    /// A key typed onto a focused chip (A3): `-` starts the `--` list, anything
    /// else starts a literal that replaces the binding when it is committed.
    pub(crate) fn chip_typed(&mut self, key: FieldKey, text: String) {
        self.var_retype = Some(Retype {
            key,
            text,
            focus: true,
        });
    }

    /// Called by a bindable field every frame it holds the keyboard, with what is
    /// typed in it: text opening with `--` opens (or narrows) the list under it.
    pub(crate) fn bind_typing(
        &mut self,
        ctx: &egui::Context,
        key: FieldKey,
        text: &str,
        under: egui::Rect,
    ) {
        let frame = ctx.cumulative_pass_nr();
        match text.strip_prefix("--") {
            Some(q) => {
                let reset = self
                    .var_complete
                    .as_ref()
                    .is_none_or(|c| c.key != key || c.query != q);
                let c = self.var_complete.get_or_insert(Complete {
                    key,
                    query: String::new(),
                    highlight: 0,
                    under,
                    own_search: false,
                    focus: false,
                    seen: frame,
                });
                if reset {
                    c.highlight = 0;
                }
                c.key = key;
                c.query = q.to_string();
                c.under = under;
                c.own_search = false;
                c.seen = frame;
            }
            None => {
                if self
                    .var_complete
                    .as_ref()
                    .is_some_and(|c| c.key == key && !c.own_search)
                {
                    self.close_complete();
                }
            }
        }
    }

    /// Open the list from the field menu (*Bind to variable…*), with a search of
    /// its own, the current variable highlighted.
    pub(crate) fn open_complete(&mut self, ctx: &egui::Context, key: FieldKey, under: egui::Rect) {
        let current = self.field_var(key);
        let table = self.session.doc.variables();
        let list = matches(table, key.kind(), "");
        let highlight = current
            .and_then(|v| list.iter().position(|x| x.id == v))
            .unwrap_or(0);
        self.var_complete = Some(Complete {
            key,
            query: String::new(),
            highlight,
            under,
            own_search: true,
            focus: true,
            seen: ctx.cumulative_pass_nr(),
        });
    }

    fn close_complete(&mut self) {
        if self.var_complete.take().is_some() {
            self.session.clear_gesture_preview();
        }
    }

    /// The variable `key` is bound to, when all of it is bound to one.
    pub(crate) fn field_var(&self, key: FieldKey) -> Option<VarId> {
        let doc = &self.session.doc;
        match key {
            FieldKey::Paint { slot, .. } => {
                let (target, index) = paint_item(slot)?;
                let mut vars = self.field_ids(key).into_iter().map(|n| {
                    let node = doc.get(n)?;
                    let list = match target {
                        PaintTarget::Fill => node.paint().fills.get(index).map(|f| &f.brush),
                        PaintTarget::Stroke => node.paint().strokes.get(index).map(|s| &s.brush),
                    };
                    list.and_then(|b| b.var())
                });
                let first = vars.next()??;
                vars.all(|v| v == Some(first)).then_some(first)
            }
            FieldKey::Radius { id } => {
                let vars = doc.get(id)?.corner_vars();
                let first = vars[0]?;
                vars.iter().all(|v| *v == Some(first)).then_some(first)
            }
            FieldKey::Corner { id, slot } => doc.get(id)?.corner_vars().get(slot).copied()?,
        }
    }

    /// The variables `key`'s targets are bound to, in name order and each once,
    /// when **every** target is bound and they are not all one — A4's *Mixed*
    /// capsule (§15 D1045). `None` when one is unbound (the plain grey *Mixed*,
    /// D130's), or all share one (`field_var`'s chip).
    pub(crate) fn field_mixed_vars(&self, key: FieldKey) -> Option<Vec<VarId>> {
        let FieldKey::Paint { slot, .. } = key else {
            return None;
        };
        let (target, index) = paint_item(slot)?;
        let doc = &self.session.doc;
        let mut vars = Vec::new();
        for n in self.field_ids(key) {
            let node = doc.get(n)?;
            let b = match target {
                PaintTarget::Fill => &node.paint().fills.get(index)?.brush,
                PaintTarget::Stroke => &node.paint().strokes.get(index)?.brush,
            };
            let v = b.var()?;
            if !vars.contains(&v) {
                vars.push(v);
            }
        }
        let table = doc.variables();
        vars.sort_by(|a, b| {
            let name = |v: &VarId| table.get(*v).map(|x| x.name.as_str()).unwrap_or("");
            name(a).cmp(name(b))
        });
        (vars.len() > 1).then_some(vars)
    }

    /// A4's *Mixed* capsule in `rect`: the selected layers bound to different
    /// variables, its tooltip listing them (§15 D1045). D1038 rules the look and
    /// not the keys, so they are the chip's: `Backspace` detaches every layer to
    /// what it draws, another key starts a literal for them all, `--` rebinds them
    /// all to one. No hover × — the chip's own rule for a mixed row — and no
    /// field menu, as on the plain *Mixed* row (`context-menus.md` §6.7).
    pub(crate) fn mixed_chip(
        &mut self,
        ui: &mut egui::Ui,
        key: FieldKey,
        vars: &[VarId],
        rect: egui::Rect,
    ) {
        let table = self.session.doc.variables();
        let tip = vars
            .iter()
            .filter_map(|v| table.get(*v).map(|x| x.name.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        let id = egui::Id::new(("var-chip", format!("{key:?}")));
        let out = chip(ui, rect, id, "Mixed", &tip, false);
        if out.detach {
            self.detach_field(key);
        } else if let Some(text) = out.typed {
            self.chip_typed(key, text);
        }
    }

    /// A bound paint field's own `%` — the binding's alpha, when every target is
    /// bound to one variable at one alpha. `None` for anything else.
    pub(crate) fn field_alpha(&self, key: FieldKey) -> Option<f32> {
        let FieldKey::Paint { slot, .. } = key else {
            return None;
        };
        let (target, index) = paint_item(slot)?;
        self.field_var(key)?;
        let doc = &self.session.doc;
        let mut alphas = self.field_ids(key).into_iter().map(|n| {
            let node = doc.get(n)?;
            let b = match target {
                PaintTarget::Fill => &node.paint().fills.get(index)?.brush,
                PaintTarget::Stroke => &node.paint().strokes.get(index)?.brush,
            };
            match b {
                ondin_core::BrushSource::Bound { alpha, .. } => Some(*alpha),
                ondin_core::BrushSource::Literal(_) => None,
            }
        });
        let first = alphas.next()??;
        alphas.all(|a| a == Some(first)).then_some(first)
    }

    /// The transaction that sets a bound paint field's `%` to `alpha`, keeping
    /// each target's binding.
    pub(crate) fn rebind_alpha_tx(&self, id: NodeId, slot: PaintSlot, alpha: f32) -> Transaction {
        let key = FieldKey::Paint { id, slot };
        let Some((target, index)) = paint_item(slot) else {
            return Transaction::default();
        };
        let ids = self.field_ids(key);
        ondin_core::set_brush_at_all(&self.session.doc, &ids, target, index, |_, b| match b {
            ondin_core::BrushSource::Bound { var, .. } => {
                ondin_core::BrushSource::Bound { var: *var, alpha }
            }
            other => other.clone(),
        })
    }

    /// The `--` list, drawn once a frame under the field it belongs to.
    pub(crate) fn var_complete_ui(&mut self, ctx: &egui::Context) {
        let Some(mut c) = self.var_complete.take() else {
            return;
        };
        // A typing field asks for its list every frame; one that stopped asking
        // (it lost the keyboard, its text lost the `--`) has closed it.
        if !c.own_search && c.seen + 1 < ctx.cumulative_pass_nr() {
            self.session.clear_gesture_preview();
            return;
        }
        let doc = &self.session.doc;
        let table = doc.variables();
        let list = matches(table, c.key.kind(), &c.query);
        c.highlight = c.highlight.min(list.len().saturating_sub(1));
        let (down, up, enter, escape) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if down && !list.is_empty() {
            c.highlight = (c.highlight + 1).min(list.len() - 1);
        }
        if up {
            c.highlight = c.highlight.saturating_sub(1);
        }
        // The mode the values read in: the selected layer's (B1's footer).
        let at = match c.key {
            FieldKey::Paint { id, .. } | FieldKey::Radius { id } | FieldKey::Corner { id, .. } => {
                id
            }
        };
        let mode = doc.mode_at(at).and_then(|m| table.mode(m)).cloned();
        let mut chosen: Option<VarId> = None;
        let pos = ui::menu_place(
            ctx,
            c.under.left_bottom() + egui::vec2(0.0, 6.0),
            egui::vec2(LIST_W, (list.len().clamp(1, 8) as f32) * LIST_ROW_H + 40.0),
        );
        let rect = egui::Area::new(egui::Id::new("var-complete"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::menu_frame(4.0).show(ui, |ui| {
                    ui.set_width(LIST_W - 8.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    if c.own_search {
                        let resp = ui::text_field(
                            ui,
                            egui::vec2(LIST_W - 8.0, 26.0),
                            &mut c.query,
                            "Search variables",
                            11.5,
                        );
                        if c.focus {
                            c.focus = false;
                            resp.request_focus();
                        }
                        ui.add_space(3.0);
                    }
                    // B4: a whole name with more after it is refused in one plain
                    // sentence — no red, no shake — and `Enter` stays inert, the
                    // list having no row to bind.
                    let refusal = (!c.own_search)
                        .then(|| format!("--{}", c.query))
                        .and_then(|t| refused(table, c.key.kind(), &t).map(|(v, at)| (v, t, at)));
                    if let Some((v, t, at)) = &refusal {
                        let font = egui::FontId::proportional(11.5);
                        let plain = egui::TextFormat::simple(font.clone(), theme::text::DIM);
                        let mut job = egui::text::LayoutJob::default();
                        job.append(
                            "A variable fills the whole field. Remove ",
                            0.0,
                            plain.clone(),
                        );
                        job.append(
                            t[*at..].trim(),
                            0.0,
                            egui::TextFormat::simple(egui::FontId::monospace(11.0), color::TEXT),
                        );
                        job.append(&format!(" to bind {}.", v.name), 0.0, plain);
                        job.wrap.max_width = LIST_W - 8.0 - 16.0;
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(job);
                        });
                        ui.add_space(4.0);
                    } else if list.is_empty() {
                        ui.label(
                            egui::RichText::new(if table.iter().any(|v| v.kind == c.key.kind()) {
                                "No variable matches"
                            } else {
                                match c.key.kind() {
                                    VarKind::Color => {
                                        "No colour variables yet — Shift+S to add one"
                                    }
                                    VarKind::Number => {
                                        "No number variables yet — Shift+S to add one"
                                    }
                                }
                            })
                            .size(11.5)
                            .color(theme::text::DIM),
                        );
                    }
                    egui::ScrollArea::vertical()
                        .max_height(8.0 * (LIST_ROW_H + 1.0))
                        .show(ui, |ui| {
                            for (i, v) in list.iter().enumerate() {
                                let value = mode.as_ref().and_then(|m| table.resolve(v.id, m.id));
                                if list_row(ui, v, value, &c.query, i == c.highlight).clicked() {
                                    chosen = Some(v.id);
                                }
                            }
                        });
                    if let Some(m) = &mode {
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(format!("{} · ↑↓ preview · ↵ bind", m.name))
                                .size(10.0)
                                .color(theme::text::FAINT),
                        );
                    }
                });
            })
            .response
            .rect;
        if enter && chosen.is_none() {
            chosen = list.get(c.highlight).map(|v| v.id);
        }
        if let Some(var) = chosen {
            self.bind_field(c.key, var);
            return;
        }
        if escape {
            self.session.clear_gesture_preview();
            return;
        }
        // A press anywhere else closes a list opened from the menu.
        let away = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
        });
        if c.own_search && away {
            self.session.clear_gesture_preview();
            return;
        }
        // The highlighted row previews live on the canvas (B1) — a paint's, and
        // since §15 D1050 a radius's: `absorb` draws a `SetCornerVars` as the
        // radii it will resolve to, where D1039 refused it.
        match list.get(c.highlight) {
            Some(v) => {
                let tx = self.bind_tx(c.key, v.id);
                self.session.set_preview(&tx);
            }
            None => self.session.clear_gesture_preview(),
        }
        self.var_complete = Some(c);
    }

    /// *Extract into variable…* (D1038 D1–D3): a new variable holding `key`'s
    /// value in every mode, the field bound to it, in one transaction — then the
    /// browser opens on its name. A first variable brings the first mode.
    pub(crate) fn extract_field(&mut self, key: FieldKey, value: Cell) {
        let table = self.session.doc.variables();
        let mut ops = Vec::new();
        let modes: Vec<ondin_core::ModeId> = match table.modes() {
            [] => {
                let m = self.session.ids.mint_mode();
                ops.push(Operation::AddMode {
                    mode: Mode {
                        id: m,
                        name: "Mode 1".into(),
                    },
                    index: 0,
                    cells: vec![],
                });
                vec![m]
            }
            modes => modes.iter().map(|m| m.id).collect(),
        };
        let stem = match key.kind() {
            VarKind::Color => "colour",
            VarKind::Number => "number",
        };
        let taken: Vec<&str> = table.iter().map(|v| v.name.as_str()).collect();
        let name = (1..)
            .map(|n| format!("{stem} {n}"))
            .find(|c| !taken.contains(&c.as_str()))
            .expect("an unbounded range has a free name");
        let id = self.session.ids.mint_var();
        ops.push(Operation::AddVariable {
            var: Variable {
                id,
                name: name.clone(),
                kind: key.kind(),
                cells: modes.into_iter().map(|m| (m, value)).collect(),
            },
        });
        // The bind names the variable by id and reads only the field, so it is
        // built against the document as it stands; the commit judges the two
        // together.
        ops.extend(self.bind_tx(key, id).0);
        if self.commit_edit(Transaction(ops)) {
            if self.var_browser.is_none() {
                self.toggle_var_browser();
            }
            if let Some(b) = self.var_browser.as_mut() {
                b.start_rename(id, name);
            }
        }
    }

    /// The field menu (§6.7), open at a point on a bindable field.
    pub(crate) fn field_menu_ui(&mut self, ctx: &egui::Context) {
        let Some((key, at, under)) = self.field_menu else {
            return;
        };
        let bound = self.field_var(key).is_some();
        let mut close = false;
        let size = egui::vec2(220.0, if bound { 3.0 } else { 2.0 } * 26.0 + 8.0);
        let pos = ui::menu_place(ctx, at, size);
        let mut act: Option<u8> = None;
        let rect = egui::Area::new(egui::Id::new("field-menu"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::menu_frame(4.0).show(ui, |ui| {
                    ui.set_width(220.0 - 8.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let r =
                        ui::MenuRow::new(icon::LINK_SIMPLE, "Bind to variable…").accel(Some("--"));
                    if ui::menu_row(ui, r, 26.0).clicked() {
                        act = Some(0);
                    }
                    let r =
                        ui::MenuRow::new(icon::EXPORT, "Extract into variable…").enabled(!bound);
                    if ui::menu_row(ui, r, 26.0).clicked() {
                        act = Some(1);
                    }
                    // Omitted unbound — §3's one exception, a row that only undoes.
                    if bound {
                        let r = ui::MenuRow::new(icon::LINK_BREAK, "Detach variable")
                            .accel(Some("Backspace"));
                        if ui::menu_row(ui, r, 26.0).clicked() {
                            act = Some(2);
                        }
                    }
                });
            })
            .response
            .rect;
        match act {
            Some(0) => {
                self.field_menu = None;
                self.open_complete(ctx, key, under);
                return;
            }
            Some(1) => {
                close = true;
                if let Some(value) = self.field_value(key) {
                    self.extract_field(key, value);
                }
            }
            Some(2) => {
                close = true;
                self.detach_field(key);
            }
            _ => {}
        }
        let away = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
        });
        if close || away || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.field_menu = None;
        }
    }

    /// A bound field's chip in `rect`, and what it was asked: detach, a key typed
    /// onto it, its menu. Shared by every bindable field.
    pub(crate) fn field_chip(
        &mut self,
        ui: &mut egui::Ui,
        key: FieldKey,
        var: VarId,
        rect: egui::Rect,
    ) {
        let doc = &self.session.doc;
        let table = doc.variables();
        let at = match key {
            FieldKey::Paint { id, .. } | FieldKey::Radius { id } | FieldKey::Corner { id, .. } => {
                id
            }
        };
        let name = table.get(var).map(|v| v.name.clone()).unwrap_or_default();
        let mode = doc.mode_at(at).and_then(|m| table.mode(m));
        let tip = chip_tip(&name, doc.resolve_var(at, var), mode);
        let id = egui::Id::new(("var-chip", format!("{key:?}")));
        let out = chip(ui, rect, id, &name, &tip, true);
        if out.detach {
            self.detach_field(key);
        } else if let Some(text) = out.typed {
            self.chip_typed(key, text);
        }
        if let Some(at) = out.menu {
            self.field_menu = Some((key, at, rect));
        }
    }

    /// The corner-radius field while bound: its icon, then the chip. `mark` is the
    /// field's override mark on an instance; the answer is whether its ↺ was
    /// clicked.
    pub(super) fn radius_chip(
        &mut self,
        ui: &mut egui::Ui,
        key: FieldKey,
        var: VarId,
        w: f32,
        mark: Option<&super::component::OverrideMark>,
    ) -> bool {
        let mut chip_rect = egui::Rect::NOTHING;
        let mut reset = false;
        ui::field_row(ui, egui::vec2(w, 28.0), |ui| {
            let (slot, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
            let p = ui.painter();
            match mark {
                // **The badge is the mark's place, as a field's label is** (§15
                // D1048, D981): bright with the dot at rest, ↺ while the pointer
                // is on the row, and a click on it the reset — the chip having no
                // label of its own to carry one.
                Some(m) => {
                    if ui.rect_contains_pointer(ui.max_rect()) {
                        p.text(
                            slot.center(),
                            egui::Align2::CENTER_CENTER,
                            icon::ARROW_COUNTER_CLOCKWISE,
                            theme::icon_font(13.0),
                            color::TEXT,
                        );
                    } else {
                        p.text(
                            slot.center(),
                            egui::Align2::CENTER_CENTER,
                            key.badge(),
                            theme::icon_font(14.0),
                            color::TEXT,
                        );
                        ui::override_dot(p, slot.right_top() + egui::vec2(1.0, 2.0));
                    }
                    reset = resp.on_hover_text(&m.tip).clicked();
                }
                // **The badge says the field will not scrub** (D1038 (3), §15
                // D1046): unbound it is the scrub handle; bound, a drag would
                // detach by accident, so it does not scrub and its cursor
                // refuses. The glyph is the session's — D1038 names no cursor.
                None => {
                    p.text(
                        slot.center(),
                        egui::Align2::CENTER_CENTER,
                        key.badge(),
                        theme::icon_font(14.0),
                        theme::text::FAINT,
                    );
                    resp.on_hover_cursor(egui::CursorIcon::NotAllowed);
                }
            }
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width() + 5.0, 28.0),
                egui::Sense::hover(),
            );
            chip_rect = rect;
        });
        self.field_chip(ui, key, var, chip_rect);
        reset
    }

    /// The corner-radius field typed over (A3): the buffer the chip's first key
    /// began. `Enter` writes the radius and detaches; `--` reopens the list.
    pub(crate) fn radius_retype(
        &mut self,
        ui: &mut egui::Ui,
        key: FieldKey,
        mut rt: Retype,
        w: f32,
    ) {
        let (FieldKey::Radius { id } | FieldKey::Corner { id, .. }) = key else {
            return;
        };
        let edit_id = egui::Id::new(("var-retype", format!("{key:?}")));
        let mut field = None;
        let table = self.session.doc.variables();
        let font = egui::FontId::proportional(12.0);
        ui::field_row(ui, egui::vec2(w, 28.0), |ui| {
            ui.label(theme::icon_text(key.badge(), 14.0, theme::text::FAINT));
            // B4's refused tail dims as it is typed (§15 D1044).
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                let at = refused(table, VarKind::Number, buf.as_str()).map(|r| r.1);
                refusal_galley(ui, buf.as_str(), at, &font)
            };
            let out = egui::TextEdit::singleline(&mut rt.text)
                .id(edit_id)
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .font(font.clone())
                .layouter(&mut layouter)
                .show(ui);
            let at = refused(table, VarKind::Number, &rt.text).map(|r| r.1);
            underline_refusal(ui, &out, at);
            field = Some(out.response);
        });
        let Some(field) = field else { return };
        if rt.focus {
            rt.focus = false;
            field.request_focus();
            caret_to_end(ui.ctx(), edit_id, &rt.text);
        }
        self.bind_typing(ui.ctx(), key, &rt.text, field.rect);
        // `Enter` on a refused `--` leaves the buffer focused and as typed (B4,
        // §15 D1044's *Fix*) — the `TextEdit` gave the keyboard up on it.
        if field.lost_focus()
            && refused(self.session.doc.variables(), VarKind::Number, &rt.text).is_some()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
        {
            rt.focus = true;
            self.var_retype = Some(rt);
            return;
        }
        if field.lost_focus() {
            self.var_retype = None;
            if ui::defocus_commits(&field)
                && !rt.text.starts_with("--")
                && let Some(r) = crate::expr::eval(&rt.text).filter(|r| r.is_finite())
            {
                // **A chip typed over is detached first**, so the radius written
                // is the literal drawn whatever it is — 0 included, which changes
                // no bound literal. **Anything else is the plain field's write**
                // (§15 D1044): the buffer is the unbound field's too since a `--`
                // hands it here, and on a rect with *some* corners bound the
                // commit's `keep_corner_bindings` must judge each corner as it
                // would the `DragValue`'s edit, not see them all detached.
                let mut ops = match self.field_var(key) {
                    Some(_) => self.detach_tx(key).0,
                    None => Vec::new(),
                };
                let r = r.max(0.0);
                let geometry = match key {
                    // One corner: the other three keep their literals, the same
                    // `CornerRadii` the per-corner field writes (§15 D1049).
                    FieldKey::Corner { slot, .. } => {
                        match self.session.doc.get(id).map(|n| n.kind()) {
                            Some(ondin_core::NodeKind::Rect { corner_radii, .. }) => {
                                let mut c = *corner_radii;
                                match slot {
                                    0 => c.top_left = r,
                                    1 => c.top_right = r,
                                    2 => c.bottom_right = r,
                                    _ => c.bottom_left = r,
                                }
                                ondin_core::GeometryPatch::CornerRadii(c)
                            }
                            _ => return,
                        }
                    }
                    _ => ondin_core::GeometryPatch::CornerRadius(r),
                };
                ops.push(Operation::SetGeometry { id, geometry });
                self.commit_edit(Transaction(ops));
            }
            return;
        }
        self.var_retype = Some(rt);
    }

    /// `key`'s value as a cell — what *Extract* files.
    fn field_value(&self, key: FieldKey) -> Option<Cell> {
        let doc = &self.session.doc;
        match key {
            FieldKey::Paint { id, slot } => {
                let (target, index) = paint_item(slot)?;
                let node = doc.get(id)?;
                let b = match target {
                    PaintTarget::Fill => &node.paint().fills.get(index)?.brush,
                    PaintTarget::Stroke => &node.paint().strokes.get(index)?.brush,
                };
                match doc.resolve_brush(id, b).into_owned() {
                    // **The colour without its `%`**: the `%` stays the fill's own
                    // field, which the bind keeps (`bind_brush_at_all`), so filing
                    // the alpha too drew a 50% fill at 25% (`arch-scribe`'s
                    // reading of the first cut).
                    ondin_core::Brush::Solid(c) => Some(Cell::Color(c.with_alpha(1.0))),
                    _ => None,
                }
            }
            FieldKey::Radius { id } => match self.session.resolved.used_kind(doc, id)? {
                ondin_core::NodeKind::Rect { corner_radii, .. } => {
                    Some(Cell::Number(corner_radii.top_left))
                }
                _ => None,
            },
            FieldKey::Corner { id, slot } => match self.session.resolved.used_kind(doc, id)? {
                ondin_core::NodeKind::Rect { corner_radii, .. } => Some(Cell::Number(match slot {
                    0 => corner_radii.top_left,
                    1 => corner_radii.top_right,
                    2 => corner_radii.bottom_right,
                    _ => corner_radii.bottom_left,
                })),
                _ => None,
            },
        }
    }
}

/// Put a just-focused field's caret after its text rather than selecting it —
/// the chip's first key is the start of what is typed, not something to replace.
pub(crate) fn caret_to_end(ctx: &egui::Context, id: egui::Id, text: &str) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let end = egui::text::CCursor::new(text.chars().count());
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(end)));
    state.store(ctx, id);
}

/// The colour picker's *Variables* face: a row's height and a group header's
/// (D1038 E1).
const FACE_ROW_H: f32 = 24.0;
const FACE_HEAD_H: f32 = 20.0;

impl OndinApp {
    /// The colour picker's *Variables* face (D1038 E1, §15 D1043): a search, then
    /// every colour variable under its group's header, each row its swatch, full
    /// name and hex **in the layer's mode**. The current variable carries the
    /// row tint and a check, and is scrolled into view once on open; a click on
    /// another row binds `key` to it, keeping its `%`. `h` is the height the face
    /// fills, the Solid face's, so the popup does not jump between tabs.
    pub(crate) fn variables_face(
        &mut self,
        ui: &mut egui::Ui,
        target: FaceTarget,
        bound: Option<VarId>,
        w: f32,
        h: f32,
    ) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let mut search = std::mem::take(&mut p.search);
        let mut scrolled = p.scrolled;
        ui::field_row(ui, egui::vec2(w, ui::CONTROL_H), |ui| {
            ui.label(theme::icon_text(
                icon::MAGNIFYING_GLASS,
                13.0,
                theme::text::DIM,
            ));
            ui.add(
                egui::TextEdit::singleline(&mut search)
                    .id(egui::Id::new("picker-vars-search"))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::proportional(11.5))
                    .hint_text(egui::RichText::new("Search colours").size(11.5)),
            )
        });
        let list_h = (h - ui::CONTROL_H - ui.spacing().item_spacing.y).max(4.0 * FACE_ROW_H);
        let doc = &self.session.doc;
        let table = doc.variables();
        let list = matches(table, VarKind::Color, &search);
        let mut chosen: Option<VarId> = None;
        egui::ScrollArea::vertical()
            .id_salt("picker-vars")
            .max_height(list_h)
            .min_scrolled_height(list_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                if list.is_empty() {
                    let none = match table.iter().any(|v| v.kind == VarKind::Color) {
                        true => "No variable matches",
                        false => "No colour variables yet — Shift+S to add one",
                    };
                    ui.label(egui::RichText::new(none).size(11.5).color(theme::text::DIM));
                }
                let mut group: Option<&str> = None;
                for v in &list {
                    let (g, _) = ondin_core::variable::split_name(&v.name);
                    if group != Some(g) {
                        group = Some(g);
                        if !g.is_empty() {
                            face_header(ui, g, w);
                        }
                    }
                    let on = bound == Some(v.id);
                    // A layer's hexes read in its mode; a cell's in the cell's
                    // own, and a row that would loop is inert and says why (G15:
                    // *this variable*, or *uses* the name it would go round by).
                    let (value, refusal) = match target {
                        FaceTarget::Field(key) => (doc.resolve_var(key.at(), v.id), None),
                        FaceTarget::Cell(var, mode) => (
                            table.resolve(v.id, mode),
                            table.would_loop(var, mode, v.id).then(|| {
                                if v.id == var {
                                    "this variable".to_owned()
                                } else {
                                    let via = match v.cells.get(&mode) {
                                        Some(ondin_core::variable::Cell::Alias(t)) => *t,
                                        _ => var,
                                    };
                                    let name = table.get(via).map_or("", |x| x.name.as_str());
                                    format!("uses {name}")
                                }
                            }),
                        ),
                    };
                    let resp = face_row(ui, v, value, on, w, refusal.as_deref());
                    if on && !scrolled {
                        resp.scroll_to_me(Some(egui::Align::Center));
                        scrolled = true;
                    }
                    if resp.clicked() && refusal.is_none() {
                        chosen = Some(v.id);
                    }
                }
            });
        if let Some(p) = self.picker.as_mut() {
            p.search = search;
            p.scrolled = scrolled;
        }
        let Some(var) = chosen.filter(|v| bound != Some(*v)) else {
            return;
        };
        match target {
            FaceTarget::Field(key) => self.bind_field(key, var),
            FaceTarget::Cell(of, mode) => {
                self.commit_edit(Transaction(vec![Operation::SetVariableCell {
                    id: of,
                    mode,
                    cell: Cell::Alias(var),
                }]));
            }
        }
    }
}

/// What the picker's *Variables* face picks for: a bindable field, a pick
/// binding it; or a colour variable's cell in one mode, a pick making it an
/// alias (D1038 G-series, §15 D1052).
#[derive(Clone, Copy, Debug)]
pub(crate) enum FaceTarget {
    Field(FieldKey),
    Cell(VarId, ondin_core::ModeId),
}

/// A group's header in the picker's *Variables* face: its path, uppercase and
/// tracked, like a card's title.
fn face_header(ui: &mut egui::Ui, group: &str, w: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, FACE_HEAD_H), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() + 3.0, rect.center().y + 1.0),
        egui::Align2::LEFT_CENTER,
        group.to_uppercase(),
        egui::FontId::proportional(9.5),
        color::text_a(97),
    );
}

/// One row of the picker's *Variables* face: swatch, full name, hex, and the
/// check on the current variable (E1).
fn face_row(
    ui: &mut egui::Ui,
    v: &Variable,
    value: Option<Value>,
    on: bool,
    w: f32,
    refusal: Option<&str>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, FACE_ROW_H), egui::Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(rect, 4, color::text_a(26));
    } else if resp.hovered() && refusal.is_none() {
        p.rect_filled(rect, 4, color::text_a(15));
    }
    let y = rect.center().y;
    let sw = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 3.0 + 6.0, y),
        egui::vec2(12.0, 12.0),
    );
    if let Some(Value::Color(c)) = value {
        ui::paint_checkerboard(p, sw, 3.0);
        p.rect_filled(sw, 3, ui::color_to_egui(c));
    }
    p.rect_stroke(
        sw,
        3,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(89)),
        egui::StrokeKind::Inside,
    );
    let check_w = 12.0;
    // A row that would loop dims and reads why in place of its hex (G15).
    let hex = match (refusal, value) {
        (Some(why), _) => why.to_owned(),
        (None, Some(Value::Color(c))) => ui::hex_of(c),
        _ => String::new(),
    };
    let hex_right = rect.right() - 3.0 - check_w - 8.0;
    let hex_galley = ui
        .fonts_mut(|f| f.layout_no_wrap(hex, egui::FontId::proportional(10.5), color::text_a(102)));
    let name_left = sw.right() + 8.0;
    let name_right = hex_right - hex_galley.size().x - 8.0;
    let font = egui::FontId::proportional(11.5);
    let shown = elide_groups(ui, &v.name, &font, name_right - name_left);
    p.text(
        egui::pos2(name_left, y + 1.0),
        egui::Align2::LEFT_CENTER,
        shown,
        font,
        match refusal {
            Some(_) => color::text_a(102),
            None => color::TEXT,
        },
    );
    p.galley(
        egui::pos2(
            hex_right - hex_galley.size().x,
            y - hex_galley.size().y / 2.0 + 1.0,
        ),
        hex_galley,
        color::TEXT,
    );
    if on {
        p.text(
            egui::pos2(rect.right() - 3.0 - check_w / 2.0, y),
            egui::Align2::CENTER_CENTER,
            icon::CHECK,
            egui::FontId::new(12.0, egui::FontFamily::Name("phosphor".into())),
            color::text_a(204),
        );
    }
    resp
}

/// A paint slot's list and index, for the slots a binding can write.
pub(crate) fn paint_item(slot: PaintSlot) -> Option<(PaintTarget, usize)> {
    match slot {
        PaintSlot::Fill(i) => Some((PaintTarget::Fill, i)),
        PaintSlot::Stroke(i) => Some((PaintTarget::Stroke, i)),
        PaintSlot::Selection(t, i) => Some((t, i)),
        _ => None,
    }
}

/// One row of the `--` list: a colour's swatch, the full name — the match
/// bright, the rest at 60% — and the value in the footer's mode (B1, B2).
fn list_row(
    ui: &mut egui::Ui,
    v: &Variable,
    value: Option<Value>,
    query: &str,
    lit: bool,
) -> egui::Response {
    let w = LIST_W - 8.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, LIST_ROW_H), egui::Sense::click());
    let p = ui.painter();
    if lit || resp.hovered() {
        p.rect_filled(rect, 4, color::text_a(23));
    }
    let mut x = rect.left() + 8.0;
    let y = rect.center().y;
    if let Some(Value::Color(c)) = value {
        let sw = egui::Rect::from_center_size(egui::pos2(x + 6.0, y), egui::vec2(12.0, 12.0));
        ui::paint_checkerboard(p, sw, 3.0);
        p.rect_filled(sw, 0, ui::color_to_egui(c));
        x = sw.right() + 8.0;
    }
    let font = egui::FontId::proportional(11.5);
    let mut job = egui::text::LayoutJob::default();
    let lower = v.name.to_lowercase();
    let q = query.trim().to_lowercase();
    let hit = (!q.is_empty()).then(|| lower.find(&q)).flatten();
    let dim = egui::TextFormat::simple(font.clone(), color::text_a(153));
    let bright = egui::TextFormat::simple(font.clone(), color::TEXT);
    match hit {
        Some(at) if v.name.is_char_boundary(at) && v.name.is_char_boundary(at + q.len()) => {
            job.append(&v.name[..at], 0.0, dim.clone());
            job.append(&v.name[at..at + q.len()], 0.0, bright);
            job.append(&v.name[at + q.len()..], 0.0, dim);
        }
        _ => job.append(&v.name, 0.0, if q.is_empty() { bright } else { dim }),
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    p.galley(
        egui::pos2(x, y - galley.size().y / 2.0),
        galley,
        color::TEXT,
    );
    let value = match value {
        Some(Value::Color(c)) => ui::hex_of(c),
        Some(Value::Number(n)) => trim_number(n),
        None => String::new(),
    };
    p.text(
        egui::pos2(rect.right() - 8.0, y),
        egui::Align2::RIGHT_CENTER,
        value,
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{BrushSource, Fill, NodeKind, keyed_by_position};

    const RED: Color = Color::from_rgba8(200, 30, 30, 255);

    /// A headless app on a canvas, holding a red rect and a white colour
    /// variable `brand/500` and a number variable `radius/md` at 6.
    fn fixture() -> (egui::Context, OndinApp, NodeId, VarId, VarId) {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.canvas_rect =
            egui::Rect::from_min_max(egui::pos2(300.0, 40.0), egui::pos2(900.0, 780.0));
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (rect, mode, brand, radius) = (
            app.session.ids.mint(),
            app.session.ids.mint_mode(),
            app.session.ids.mint_var(),
            app.session.ids.mint_var(),
        );
        let root = app.session.doc.root();
        assert!(app.commit_edit(Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 20.0),
                    corner_radii: RoundedRectRadii::from_single_radius(2.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: keyed_by_position([Fill {
                    brush: RED.with_alpha(0.5).into(),
                    visible: true,
                }]),
            },
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
                    id: brand,
                    name: "brand/500".into(),
                    kind: VarKind::Color,
                    cells: [(mode, Cell::Color(Color::WHITE))].into(),
                },
            },
            Operation::AddVariable {
                var: Variable {
                    id: radius,
                    name: "radius/md".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(6.0))].into(),
                },
            },
        ])));
        (ctx, app, rect, brand, radius)
    }

    fn fill0(app: &OndinApp, rect: NodeId) -> BrushSource {
        app.session.doc.get(rect).unwrap().paint().fills[0]
            .brush
            .clone()
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// **Typing `--br` and `Enter` binds the fill to the one match, keeping its
    /// `%`**; the list previews the highlighted variable on the canvas while it is
    /// open (B1, B2).
    #[test]
    fn typing_dashes_and_enter_binds_the_highlighted_variable() {
        let (ctx, mut app, rect, brand, _) = fixture();
        let k = FieldKey::Paint {
            id: rect,
            slot: PaintSlot::Fill(0),
        };
        let under = egui::Rect::from_min_size(egui::pos2(960.0, 200.0), egui::vec2(200.0, 28.0));
        let _ = ctx.run_ui(Default::default(), |_| {
            app.bind_typing(&ctx, k, "--br", under);
            app.var_complete_ui(&ctx);
        });
        assert!(app.var_complete.is_some(), "the list is open");
        assert!(app.session.has_gesture_preview(), "the highlight previews");
        let input = egui::RawInput {
            events: vec![key(egui::Key::Enter)],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |_| app.var_complete_ui(&ctx));
        assert_eq!(
            fill0(&app, rect),
            BrushSource::Bound {
                var: brand,
                alpha: 0.5
            }
        );
        assert!(app.var_complete.is_none() && !app.session.has_gesture_preview());
        assert_eq!(app.field_var(k), Some(brand));
        assert_eq!(
            app.field_alpha(k),
            Some(0.5),
            "the `%` is the binding's own"
        );
    }

    /// A list nobody typed into since the frame before closes, and takes its
    /// preview with it — the field lost the keyboard, or its `--`.
    #[test]
    fn a_list_its_field_stopped_asking_for_closes() {
        let (ctx, mut app, rect, _, _) = fixture();
        let k = FieldKey::Paint {
            id: rect,
            slot: PaintSlot::Fill(0),
        };
        let under = egui::Rect::from_min_size(egui::pos2(960.0, 200.0), egui::vec2(200.0, 28.0));
        let _ = ctx.run_ui(Default::default(), |_| {
            app.bind_typing(&ctx, k, "--", under);
            app.var_complete_ui(&ctx);
        });
        for _ in 0..3 {
            let _ = ctx.run_ui(Default::default(), |_| app.var_complete_ui(&ctx));
        }
        assert!(app.var_complete.is_none());
        assert!(!app.session.has_gesture_preview());
        assert_eq!(
            fill0(&app, rect),
            BrushSource::from(RED.with_alpha(0.5)),
            "nothing written"
        );
    }

    /// **Detaching leaves the colour drawn**, its `%` folded in; the radius the
    /// same, through `detach_corners_ops`.
    #[test]
    fn detaching_leaves_what_the_field_drew() {
        let (_ctx, mut app, rect, brand, radius) = fixture();
        let paint = FieldKey::Paint {
            id: rect,
            slot: PaintSlot::Fill(0),
        };
        app.bind_field(paint, brand);
        app.detach_field(paint);
        assert_eq!(
            fill0(&app, rect),
            BrushSource::from(Color::WHITE.multiply_alpha(0.5))
        );
        let corners = FieldKey::Radius { id: rect };
        app.bind_field(corners, radius);
        assert_eq!(app.field_var(corners), Some(radius));
        app.detach_field(corners);
        assert_eq!(app.field_var(corners), None);
        assert!(matches!(
            app.session.doc.get(rect).unwrap().kind(),
            NodeKind::Rect { corner_radii, .. } if corner_radii.top_left == 6.0
        ));
    }

    /// **Extracting a translucent fill draws what it drew**: the variable takes
    /// the colour opaque and the fill keeps its `%`. The first cut filed the
    /// alpha into the variable as well, and the bind kept it as the `%` too — a
    /// 50% fill drew at 25%.
    ///
    /// Flip, run: filing the resolved colour with its alpha fails *"drawn as
    /// before"*.
    #[test]
    fn extracting_a_translucent_fill_draws_what_it_drew() {
        let (_ctx, mut app, rect, _, _) = fixture();
        let k = FieldKey::Paint {
            id: rect,
            slot: PaintSlot::Fill(0),
        };
        let value = app.field_value(k).unwrap();
        app.extract_field(k, value);
        let doc = &app.session.doc;
        let drawn = doc.resolve_brush(rect, &fill0(&app, rect)).into_owned();
        assert_eq!(
            drawn,
            ondin_core::Brush::Solid(RED.with_alpha(0.5)),
            "drawn as before"
        );
    }

    /// *Extract into variable…* files the value as a new variable, binds the
    /// field to it in the same transaction — one undo takes both — and opens the
    /// browser on its name (D1–D3).
    #[test]
    fn extract_files_the_value_and_binds_the_field() {
        let (_ctx, mut app, rect, _, _) = fixture();
        let k = FieldKey::Radius { id: rect };
        let before = app.session.doc.clone();
        app.extract_field(k, Cell::Number(2.0));
        let var = app.field_var(k).expect("bound to the new variable");
        let v = app.session.doc.variables().get(var).unwrap();
        assert_eq!(v.name, "number 1");
        assert!(v.cells.values().all(|c| *c == Cell::Number(2.0)));
        assert!(app.var_browser.is_some(), "the browser opens on it");
        app.session.undo();
        assert_eq!(
            app.session.doc, before,
            "one undo takes the variable and the binding"
        );
    }

    /// **A focused chip takes `Backspace` for itself** — the detach, not the
    /// keymap's delete — and hands any other key on as the start of a retype.
    ///
    /// Flip, run: reading `Backspace` with `key_pressed` rather than consuming it
    /// fails *"and is consumed"* — the key would reach the keymap too, where an
    /// egui focus is the only other thing holding *Delete selection* off.
    #[test]
    fn a_focused_chip_takes_backspace_and_hands_on_a_key() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let id = egui::Id::new("chip-under-test");
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(120.0, 28.0));
        let run = |events: Vec<egui::Event>| {
            let mut out = ChipOut::default();
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                out = chip(ui, rect, id, "brand/500", "tip", true);
            });
            out
        };
        let _ = run(vec![]);
        ctx.memory_mut(|m| m.request_focus(id));
        let _ = run(vec![]);
        let out = run(vec![key(egui::Key::Backspace)]);
        assert!(out.detach, "Backspace detaches");
        assert!(
            !ctx.input(|i| i.key_pressed(egui::Key::Backspace)),
            "and is consumed, so the keymap does not delete the layer"
        );
        ctx.memory_mut(|m| m.request_focus(id));
        let out = run(vec![egui::Event::Text("8".into())]);
        assert_eq!(out.typed.as_deref(), Some("8"));
    }

    /// Where text `s` was drawn this frame, by its galley's centre.
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

    /// One frame of the picker, its shapes returned.
    fn picker_frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 900.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |_| app.picker_ui(ctx)).shapes
    }

    /// Click the text `s` the picker drew, over three frames: move, press, release.
    fn click_text(ctx: &egui::Context, app: &mut OndinApp, s: &str) {
        let shapes = picker_frame(ctx, app, vec![]);
        let at = text_at(&shapes, s).unwrap_or_else(|| panic!("{s:?} is drawn"));
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        picker_frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
        picker_frame(ctx, app, vec![button(true)]);
        picker_frame(ctx, app, vec![button(false)]);
    }

    /// **The colour picker's *Variables* tab binds, and *Solid* detaches** (D1038
    /// E1, §15 D1043), driven through the popup's own clicks: the tab opens the
    /// face over an unbound fill and writes nothing; a row binds the fill, keeping
    /// its `%`; the face then stays up, the paint's kind being the binding; and
    /// *Solid* leaves the colour drawn, `%` folded in.
    ///
    /// Flip, run: sending *Solid* through `write_slot` like any kind change — the
    /// drawn colour back through `edited_brush` — fails *"Solid off a binding
    /// detaches"* with the binding kept, which is the edge D1041 recorded.
    #[test]
    fn the_pickers_variables_tab_binds_and_solid_detaches() {
        let (ctx, mut app, rect, brand, _) = fixture();
        crate::theme::install(&ctx);
        app.session.selection.set(vec![rect]);
        app.picker = Some(crate::panels::picker::Picker::new(
            rect,
            PaintSlot::Fill(0),
            egui::pos2(400.0, 300.0),
        ));
        for _ in 0..2 {
            picker_frame(&ctx, &mut app, vec![]);
        }
        click_text(&ctx, &mut app, "Variables");
        assert!(
            app.picker.as_ref().is_some_and(|p| p.browse),
            "the face is up"
        );
        assert_eq!(
            fill0(&app, rect),
            BrushSource::from(RED.with_alpha(0.5)),
            "the tab alone writes nothing"
        );
        click_text(&ctx, &mut app, "brand/500");
        assert_eq!(
            fill0(&app, rect),
            BrushSource::Bound {
                var: brand,
                alpha: 0.5
            },
            "a row binds, keeping the `%`"
        );
        let shapes = picker_frame(&ctx, &mut app, vec![]);
        assert!(
            text_at(&shapes, "BRAND").is_some(),
            "the face stays up over the binding, under its group's header"
        );
        click_text(&ctx, &mut app, "Solid");
        assert_eq!(
            fill0(&app, rect),
            BrushSource::from(Color::WHITE.multiply_alpha(0.5)),
            "Solid off a binding detaches to the colour drawn"
        );
        assert!(app.picker.as_ref().is_some_and(|p| !p.browse));
    }

    /// A bound fill's picker opens on the *Variables* face (E1) — the binding is
    /// the paint's kind. The last assertion is only that an effect's colour is not
    /// a slot `paint_item` binds, which is what keeps its picker at three tabs; no
    /// effect picker is drawn here and no tabs are counted.
    #[test]
    fn a_bound_fill_opens_on_the_variables_face() {
        let (ctx, mut app, rect, brand, _) = fixture();
        crate::theme::install(&ctx);
        app.bind_field(
            FieldKey::Paint {
                id: rect,
                slot: PaintSlot::Fill(0),
            },
            brand,
        );
        app.session.selection.set(vec![rect]);
        app.picker = Some(crate::panels::picker::Picker::new(
            rect,
            PaintSlot::Fill(0),
            egui::pos2(400.0, 300.0),
        ));
        let mut shapes = vec![];
        for _ in 0..3 {
            shapes = picker_frame(&ctx, &mut app, vec![]);
        }
        assert!(
            text_at(&shapes, "Search colours").is_some(),
            "the face is up"
        );
        assert!(text_at(&shapes, "brand/500").is_some());
        assert!(
            paint_item(PaintSlot::Effect(0)).is_none(),
            "an effect's colour is not a bindable slot"
        );
    }

    /// **B4: a whole name with more after it is refused, and only then** (§15
    /// D1044): `--radius/md * 2` refuses ` * 2`, case aside; a name still being
    /// typed, or the whole name alone, is not; and a whole name that is the start
    /// of a longer one is not refused while the longer one matches.
    ///
    /// Flip, run: dropping the `matches(..).is_empty()` guard fails *"a longer
    /// name still matching is not refused"* — `--brand/500-` read as `brand/500`
    /// and a refused `-`.
    #[test]
    fn a_whole_name_with_more_after_it_is_refused() {
        let (_ctx, mut app, _rect, _, _) = fixture();
        let mode = app.session.doc.variables().modes()[0].id;
        let alt = app.session.ids.mint_var();
        assert!(app.commit_edit(Transaction(vec![Operation::AddVariable {
            var: Variable {
                id: alt,
                name: "brand/500-alt".into(),
                kind: VarKind::Color,
                cells: [(mode, Cell::Color(Color::BLACK))].into(),
            },
        }])));
        let table = app.session.doc.variables();
        let at = |kind, text| refused(table, kind, text).map(|(v, at)| (v.name.clone(), at));
        assert_eq!(
            at(VarKind::Number, "--radius/md * 2"),
            Some(("radius/md".into(), 11))
        );
        assert_eq!(
            at(VarKind::Number, "--RADIUS/md*2"),
            Some(("radius/md".into(), 11)),
            "case aside, as the list matches"
        );
        assert_eq!(at(VarKind::Number, "--radius/m"), None, "still being typed");
        assert_eq!(
            at(VarKind::Number, "--radius/md"),
            None,
            "the whole name binds"
        );
        assert_eq!(
            at(VarKind::Number, "--radius/md  "),
            None,
            "trailing space is nothing"
        );
        assert_eq!(
            at(VarKind::Number, "--nothing * 2"),
            None,
            "no name: B3's no match"
        );
        assert_eq!(
            at(VarKind::Number, "--brand/500 x"),
            None,
            "a colour's name in a number field is no match"
        );
        assert_eq!(
            at(VarKind::Color, "--brand/500-"),
            None,
            "a longer name still matching is not refused"
        );
        assert_eq!(
            at(VarKind::Color, "--brand/500 * 2"),
            Some(("brand/500".into(), 11))
        );
    }

    /// **The `--` list says a refusal in one sentence, and `Enter` binds nothing**
    /// (B4): *Remove `* 2` to bind radius/md*, the list open after the key.
    #[test]
    fn a_refused_query_says_why_and_enter_is_inert() {
        let (ctx, mut app, rect, _, _) = fixture();
        crate::theme::install(&ctx);
        let k = FieldKey::Radius { id: rect };
        let under = egui::Rect::from_min_size(egui::pos2(960.0, 200.0), egui::vec2(200.0, 28.0));
        // Two passes: an area's first is egui's invisible sizing pass.
        let mut shapes = vec![];
        for _ in 0..2 {
            shapes = ctx
                .run_ui(Default::default(), |_| {
                    app.bind_typing(&ctx, k, "--radius/md * 2", under);
                    app.var_complete_ui(&ctx);
                })
                .shapes;
        }
        let said = "A variable fills the whole field. Remove * 2 to bind radius/md.";
        assert!(text_at(&shapes, said).is_some(), "the sentence is drawn");
        let input = egui::RawInput {
            events: vec![key(egui::Key::Enter)],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |_| {
            app.bind_typing(&ctx, k, "--radius/md * 2", under);
            app.var_complete_ui(&ctx);
        });
        assert_eq!(app.field_var(k), None, "Enter binds nothing");
        assert!(app.var_complete.is_some(), "and the list stays");
    }

    /// **The radius buffer a `--` is handed to writes a number typed back over it,
    /// on an unbound rect too, and `Escape` writes nothing** (§15 D1044): the
    /// corner field's `DragValue` passes `--` text to `radius_retype` so B4's tail
    /// can be drawn, which makes that buffer the unbound field's as well as a
    /// chip's.
    #[test]
    fn the_radius_buffer_writes_a_number_and_escape_leaves_it() {
        let (ctx, mut app, rect, _, _) = fixture();
        crate::theme::install(&ctx);
        let k = FieldKey::Radius { id: rect };
        let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                if let Some(rt) = app.var_retype.clone() {
                    app.radius_retype(ui, k, rt, 160.0);
                }
            });
        };
        let radius = |app: &OndinApp| match app.session.doc.get(rect).unwrap().kind() {
            NodeKind::Rect { corner_radii, .. } => corner_radii.top_left,
            _ => unreachable!(),
        };
        app.var_retype = Some(Retype {
            key: k,
            text: "--radius/md * 2".into(),
            focus: true,
        });
        run(&mut app, vec![]);
        run(&mut app, vec![key(egui::Key::Escape)]);
        assert!(app.var_retype.is_none(), "Escape leaves the buffer");
        assert_eq!(radius(&app), 2.0, "and writes nothing");
        app.var_retype = Some(Retype {
            key: k,
            text: "12".into(),
            focus: true,
        });
        run(&mut app, vec![]);
        run(&mut app, vec![key(egui::Key::Enter)]);
        assert_eq!(radius(&app), 12.0, "a number typed back writes");
        assert_eq!(app.field_var(k), None);
    }

    /// **A4: two layers bound to different variables read *Mixed* in a capsule**,
    /// its tooltip the two names; one unbound, or both on one variable, is not
    /// that case (§15 D1045). Focused, `Backspace` detaches both, each to what it
    /// draws.
    ///
    /// Flip, run: letting `field_mixed_vars` answer with one variable (`> 0` for
    /// `> 1`) fails *"one variable is the chip's"*.
    #[test]
    fn layers_bound_to_different_variables_read_mixed_in_a_capsule() {
        let (ctx, mut app, rect, brand, _) = fixture();
        crate::theme::install(&ctx);
        let mode = app.session.doc.variables().modes()[0].id;
        let (other, accent) = (app.session.ids.mint(), app.session.ids.mint_var());
        let root = app.session.doc.root();
        assert!(app.commit_edit(Transaction(vec![
            Operation::CreateNode {
                id: other,
                parent: root,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::from_single_radius(0.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: other,
                fills: keyed_by_position([Fill {
                    brush: RED.into(),
                    visible: true,
                }]),
            },
            Operation::AddVariable {
                var: Variable {
                    id: accent,
                    name: "accent/400".into(),
                    kind: VarKind::Color,
                    cells: [(mode, Cell::Color(Color::BLACK))].into(),
                },
            },
        ])));
        app.session.selection.set(vec![rect, other]);
        let row = FieldKey::Paint {
            id: rect,
            slot: PaintSlot::Selection(PaintTarget::Fill, 0),
        };
        let one = |id| FieldKey::Paint {
            id,
            slot: PaintSlot::Fill(0),
        };
        app.bind_field(one(rect), brand);
        assert_eq!(
            app.field_mixed_vars(row),
            None,
            "one unbound is the grey Mixed"
        );
        app.bind_field(one(other), brand);
        assert_eq!(
            app.field_mixed_vars(row),
            None,
            "one variable is the chip's"
        );
        app.bind_field(one(other), accent);
        assert_eq!(
            app.field_mixed_vars(row),
            Some(vec![accent, brand]),
            "both, in name order"
        );
        let id = egui::Id::new(("var-chip", format!("{row:?}")));
        let at = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(140.0, 28.0));
        let draw = |app: &mut OndinApp, events| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| app.mixed_chip(ui, row, &[accent, brand], at))
                .shapes
        };
        let shapes = draw(&mut app, vec![]);
        assert!(
            text_at(&shapes, "Mixed").is_some(),
            "the capsule reads Mixed"
        );
        ctx.memory_mut(|m| m.request_focus(id));
        draw(&mut app, vec![]);
        draw(&mut app, vec![key(egui::Key::Backspace)]);
        assert_eq!(
            fill0(&app, rect),
            BrushSource::from(Color::WHITE.multiply_alpha(0.5))
        );
        assert_eq!(fill0(&app, other), BrushSource::from(Color::BLACK));
    }

    /// **A bound radius field's badge refuses the scrub cursor** (D1038 (3), §15
    /// D1046): the pointer over the corner icon asks for `NotAllowed`.
    ///
    /// Flip, run: removing the `on_hover_cursor` fails with the default cursor.
    #[test]
    fn a_bound_radius_badge_says_it_will_not_scrub() {
        let (ctx, mut app, rect, _, radius) = fixture();
        crate::theme::install(&ctx);
        let k = FieldKey::Radius { id: rect };
        app.bind_field(k, radius);
        // The badge's centre, measured: it draws at (10, 7)–(24, 21) here.
        let over = egui::pos2(17.0, 14.0);
        let mut cursor = egui::CursorIcon::Default;
        for events in [vec![], vec![egui::Event::PointerMoved(over)], vec![]] {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| {
                ui.set_min_width(180.0);
                app.radius_chip(ui, k, radius, 160.0, None);
            });
            cursor = out.platform_output.cursor_icon;
        }
        assert_eq!(cursor, egui::CursorIcon::NotAllowed);
    }

    /// **`Enter` on a refused `--` leaves either field as typed, the list still
    /// up** (B4, §15 D1044's *Fix*): the hex field and the radius buffer, each
    /// drawn for real, the key landing once the buffer holds the keyboard.
    ///
    /// Flip, run: without the hex field's `refused_enter` arm it fails *"the hex
    /// buffer is kept"* — the `TextEdit` gave the keyboard up on `Enter` and
    /// `lost_focus` dropped the text, the defect `arch-scribe` read. And without
    /// `radius_retype`'s arm it fails *"the radius buffer is kept"*.
    #[test]
    fn enter_on_a_refusal_leaves_the_field_as_typed() {
        let (ctx, mut app, rect, _, _) = fixture();
        crate::theme::install(&ctx);
        let typed = "--brand/500 * 2";
        let hex = FieldKey::Paint {
            id: rect,
            slot: PaintSlot::Fill(0),
        };
        app.var_retype = Some(Retype {
            key: hex,
            text: typed.into(),
            focus: true,
        });
        let brush = ondin_core::Brush::Solid(RED.with_alpha(0.5));
        let hex_frame = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                app.paint_hex_field(ui, rect, PaintSlot::Fill(0), &brush, false);
                app.var_complete_ui(&ctx);
            });
        };
        for _ in 0..3 {
            hex_frame(&mut app, vec![]);
        }
        assert!(
            app.var_complete.is_some(),
            "the list is up over the refusal"
        );
        hex_frame(&mut app, vec![key(egui::Key::Enter)]);
        for _ in 0..3 {
            hex_frame(&mut app, vec![]);
        }
        assert!(
            matches!(&app.paint_hex, Some((_, _, t)) if t == typed),
            "the hex buffer is kept"
        );
        assert!(app.var_complete.is_some(), "and the list stays up");
        assert_eq!(fill0(&app, rect), BrushSource::from(RED.with_alpha(0.5)));

        let radius = FieldKey::Radius { id: rect };
        app.paint_hex = None;
        app.var_complete = None;
        app.var_retype = Some(Retype {
            key: radius,
            text: "--radius/md * 2".into(),
            focus: true,
        });
        let radius_frame = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                if let Some(rt) = app.var_retype.clone() {
                    app.radius_retype(ui, radius, rt, 160.0);
                }
                app.var_complete_ui(&ctx);
            });
        };
        radius_frame(&mut app, vec![]);
        radius_frame(&mut app, vec![]);
        radius_frame(&mut app, vec![key(egui::Key::Enter)]);
        for _ in 0..3 {
            radius_frame(&mut app, vec![]);
        }
        assert_eq!(
            app.var_retype.as_ref().map(|r| r.text.as_str()),
            Some("--radius/md * 2"),
            "the radius buffer is kept"
        );
        assert!(app.var_complete.is_some(), "and its list stays up");
    }

    /// **A number typed back over a `--` on a rect with *some* corners bound
    /// keeps a corner written its own drawn value bound** (§15 D1044): the
    /// buffer writes as the plain field does, through `keep_corner_bindings`, and
    /// only a chip's retype detaches every corner first.
    ///
    /// Flip, run: detaching first whatever the field (the first cut) fails
    /// *"the bound corner written 6 stays bound"*.
    #[test]
    fn a_number_typed_back_keeps_a_partly_bound_rects_binding() {
        let (ctx, mut app, rect, _, radius) = fixture();
        crate::theme::install(&ctx);
        let ops =
            ondin_core::bind_corners_ops(&app.session.doc, rect, [Some(radius), None, None, None]);
        assert!(app.commit_edit(Transaction(ops)));
        let k = FieldKey::Radius { id: rect };
        assert_eq!(
            app.field_var(k),
            None,
            "the fixture is partly bound, no chip"
        );
        app.var_retype = Some(Retype {
            key: k,
            text: "6".into(),
            focus: true,
        });
        for events in [vec![], vec![], vec![key(egui::Key::Enter)]] {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                if let Some(rt) = app.var_retype.clone() {
                    app.radius_retype(ui, k, rt, 160.0);
                }
            });
        }
        assert!(
            app.var_retype.is_none(),
            "Enter wrote and closed the buffer"
        );
        assert_eq!(
            app.session.doc.get(rect).unwrap().corner_vars()[0],
            Some(radius),
            "the bound corner written 6 stays bound"
        );
    }

    /// **One corner binds, detaches and is typed over alone** (§15 D1049): the
    /// bottom right (`CornerVars` slot 2) bound to *radius/md* leaves the other
    /// three literal; its detach leaves 6, what it drew, with the top left — bound
    /// too by then — still bound; and a number typed over its chip writes that
    /// corner and no other.
    ///
    /// Flips, run: binding every slot in `bind_tx`'s `Corner` arm fails *"only the
    /// bottom right is bound"*; `detach_corners_ops` in `detach_tx`'s `Corner` arm
    /// fails *"the top left stays bound"* — and was green until the top left was
    /// bound, a detach of four unbound-but-one corners writing what each draws,
    /// which for a literal corner is its literal.
    #[test]
    fn one_corner_binds_detaches_and_retypes_alone() {
        let (ctx, mut app, rect, _, radius) = fixture();
        crate::theme::install(&ctx);
        let br = FieldKey::Corner { id: rect, slot: 2 };
        app.bind_field(br, radius);
        let vars = app.session.doc.get(rect).unwrap().corner_vars();
        assert_eq!(
            vars,
            [None, None, Some(radius), None],
            "only the bottom right is bound"
        );
        assert_eq!(app.field_var(br), Some(radius));
        assert_eq!(app.field_var(FieldKey::Corner { id: rect, slot: 3 }), None);
        let radii = |app: &OndinApp| match app.session.doc.get(rect).unwrap().kind() {
            NodeKind::Rect { corner_radii, .. } => *corner_radii,
            _ => unreachable!(),
        };
        let tl = FieldKey::Corner { id: rect, slot: 0 };
        app.bind_field(tl, radius);
        app.detach_field(br);
        assert_eq!(app.field_var(br), None);
        assert_eq!(app.field_var(tl), Some(radius), "the top left stays bound");
        let r = radii(&app);
        assert_eq!(
            (r.top_right, r.bottom_right, r.bottom_left),
            (2.0, 6.0, 2.0),
            "the corner keeps what it drew, the others their own"
        );
        app.detach_field(tl);

        app.bind_field(br, radius);
        app.var_retype = Some(Retype {
            key: br,
            text: "9".into(),
            focus: true,
        });
        for events in [vec![], vec![], vec![key(egui::Key::Enter)]] {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                if let Some(rt) = app.var_retype.clone() {
                    app.radius_retype(ui, br, rt, 80.0);
                }
            });
        }
        let r = radii(&app);
        assert_eq!(
            (r.top_left, r.top_right, r.bottom_right, r.bottom_left),
            // The top left kept the 6 it drew when it was detached above.
            (6.0, 2.0, 9.0, 2.0),
            "the number typed lands on that corner alone"
        );
        assert_eq!(app.field_var(br), None, "and detaches it");
    }

    /// The field's refused tail is laid out at 40%, the rest as `TextEdit` would
    /// have drawn it (B4).
    #[test]
    fn a_refused_tail_is_laid_out_dim() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let _ = ctx.run_ui(Default::default(), |ui| {
            let font = egui::FontId::proportional(12.0);
            let g = refusal_galley(ui, "--radius/md * 2", Some(11), &font);
            let sections = &g.job.sections;
            assert_eq!(sections.len(), 2);
            let r = &sections[1].byte_range;
            assert_eq!((r.start.0, r.end.0), (11, 15), "the dim section is ` * 2`");
            assert_eq!(sections[1].format.color, color::text_a(102));
            assert_ne!(sections[0].format.color, color::text_a(102));
            let plain = refusal_galley(ui, "--radius/md", None, &font);
            assert_eq!(plain.job.sections.len(), 1);
        });
    }

    /// Long names elide from the left by whole groups, then the leaf (A5).
    #[test]
    fn a_long_name_elides_from_the_left_by_groups() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let font = egui::FontId::proportional(11.5);
        let _ = ctx.run_ui(Default::default(), |ui| {
            let full = "layout/sidebar/width-collapsed";
            assert_eq!(elide_groups(ui, full, &font, 1000.0), full);
            let w = ui.fonts_mut(|f| {
                f.layout_no_wrap("…/width-collapsed".into(), font.clone(), color::TEXT)
                    .size()
                    .x
            });
            assert_eq!(elide_groups(ui, full, &font, w + 1.0), "…/width-collapsed");
            assert!(elide_groups(ui, full, &font, 30.0).ends_with('…'));
        });
    }

    /// **Every face of the colour popup is one height** (E1: *"the popup keeps
    /// its height on every tab, so nothing jumps under the pointer"*; §15 D1057),
    /// measured as the bounds of everything the popup paints, shadow included,
    /// on each tab in turn.
    ///
    /// Flip, run: the gradient plane back at 84 fails on *Linear*, 8 points
    /// taller than *Solid* — the height D1043 left.
    #[test]
    fn every_face_of_the_colour_popup_is_one_height() {
        let (ctx, mut app, rect, _, _) = fixture();
        crate::theme::install(&ctx);
        app.session.selection.set(vec![rect]);
        app.picker = Some(crate::panels::picker::Picker::new(
            rect,
            PaintSlot::Fill(0),
            egui::pos2(400.0, 100.0),
        ));
        let h = |ctx: &egui::Context, app: &mut OndinApp| {
            let s = picker_frame(ctx, app, vec![]);
            s.iter()
                .map(|c| c.shape.visual_bounding_rect())
                .filter(|r| r.is_finite())
                .fold(egui::Rect::NOTHING, |a, b| a.union(b))
        };
        for _ in 0..3 {
            picker_frame(&ctx, &mut app, vec![]);
        }
        let solid = h(&ctx, &mut app);
        for t in ["Linear", "Radial", "Variables", "Solid"] {
            click_text(&ctx, &mut app, t);
            for _ in 0..3 {
                picker_frame(&ctx, &mut app, vec![]);
            }
            assert_eq!(h(&ctx, &mut app), solid, "{t} is the Solid face's height");
        }
    }
}
