//! *Bind other uses*: the fields across the document that draw a variable's
//! value without being bound to it, offered after *Extract into variable…* and
//! from a variable's menu, and bound together in one step.
//!
//! **Same value means binding changes nothing on screen**: each field's drawn
//! value is compared with the variable's value in the modes in force at that
//! layer (`Document::resolve_var`). A paint draws the variable's colour times
//! its own `%`, so it matches on the colour, and the bind sets the `%` that
//! keeps its alpha: a 25% black is a 50% black variable at 50% (`share`). A
//! field the bind would not write (`build::bind_field_ops` empty) is not offered.
//!
//! Left out: fields already bound, the vault and kit instances (their mains
//! belong to the kit, and its next update would drop the binding), visibility,
//! and the resting values every layer has — an opacity of 100% and a zero
//! radius, gap, padding, limit or effect number — which would list every
//! layer in the document. A copy that draws the value because its main does is
//! offered once, as the main's field, with the copies counted; a copy that
//! overrides it is its own row.

use super::binding::{COLUMN_GAP, FieldKey, PAD_BOTTOM, PAD_LEFT, PAD_RIGHT, PAD_TOP, ROW_GAP};
use super::paint::PaintSlot;
use crate::app::OndinApp;
use ondin_core::peniko::Color;
use ondin_core::variable::{Cell, Field, Value, VarId};
use ondin_core::{Document, NodeId, NodeKind, Operation, PaintTarget, Transaction};

/// One field that draws the variable's value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Use {
    pub key: FieldKey,
    pub field: Field,
    /// The copies outside the vault that follow this field from their main,
    /// and are bound with it.
    pub follows: usize,
    /// Why the field cannot be bound from the list — a locked layer, a style
    /// the bind would detach — as the row's tooltip says it; `None` when it can.
    pub why_not: Option<String>,
    /// The row's tail: the layer's page, or what a main's row reaches.
    pub tail: String,
}

/// The fields one node may bind, of the families a variable of `kind` takes,
/// whether bound or not.
fn keys_of(doc: &Document, n: &ondin_core::Node) -> Vec<FieldKey> {
    let id = n.id();
    let mut out = Vec::new();
    for i in 0..n.paint().fills.len() {
        out.push(FieldKey::Paint {
            id,
            slot: PaintSlot::Fill(i),
        });
    }
    for (i, s) in n.paint().strokes.iter().enumerate() {
        out.push(FieldKey::Paint {
            id,
            slot: PaintSlot::Stroke(i),
        });
        out.push(FieldKey::StrokeWidth { id, item: s.id });
    }
    if n.kind().corner_radii().is_some() {
        if n.kind().has_four_corners() {
            out.extend((0..4).map(|slot| FieldKey::Corner { id, slot }));
        } else {
            out.push(FieldKey::Radius { id });
        }
    }
    if doc.resolved_display(id).is_some() {
        for slots in [
            COLUMN_GAP, ROW_GAP, PAD_TOP, PAD_RIGHT, PAD_BOTTOM, PAD_LEFT,
        ] {
            out.push(FieldKey::Spacing { id, slots });
        }
        out.extend((0..2).map(|slot| FieldKey::Flow { id, slot }));
    }
    if n.kind().box_size().is_some() {
        out.extend((0..2).map(|axis| FieldKey::Size { id, axis }));
    }
    if matches!(n.kind(), NodeKind::Text { .. }) {
        use ondin_core::variable as v;
        out.push(FieldKey::Content { id });
        out.push(FieldKey::Family { id });
        for slot in [
            v::TEXT_SIZE,
            v::TEXT_WEIGHT,
            v::TEXT_LINE_HEIGHT,
            v::TEXT_LETTER_SPACING,
            v::TEXT_PARAGRAPH_SPACING,
        ] {
            out.push(FieldKey::Type { id, slot });
        }
    }
    if n.parent().is_some() {
        out.extend((0..4).map(|slot| FieldKey::Limit { id, slot }));
    }
    out.push(FieldKey::Opacity { id });
    for e in n.effects() {
        out.extend((0..4).map(|slot| FieldKey::EffectNumber {
            id,
            item: e.id,
            slot,
        }));
    }
    out
}

/// Whether `v` is a resting value of `key` — what nearly every layer holds,
/// and so no sign of a shared value.
fn resting(key: FieldKey, v: &Cell) -> bool {
    let Cell::Number(x) = v else {
        return false;
    };
    match key {
        FieldKey::Opacity { .. } => (*x - 100.0).abs() < 1e-6,
        FieldKey::Radius { .. }
        | FieldKey::Corner { .. }
        | FieldKey::Spacing { .. }
        | FieldKey::Limit { .. }
        | FieldKey::EffectNumber { .. } => x.abs() < 1e-9,
        _ => false,
    }
}

/// Whether `key` holds its drawn number as such: not a percentage limit nor an
/// *Auto* line height, which `field_value` reads as the px they come to and a
/// bind would turn into a fixed px, the same today and not after a resize.
fn literal(doc: &Document, key: FieldKey) -> bool {
    match key {
        FieldKey::Limit { id, slot } => matches!(
            doc.resolved_item(id).limit(slot),
            ondin_core::container::Dimension::Px(_)
        ),
        FieldKey::Type { id, slot } => match doc.resolved_kind(id).as_deref() {
            Some(NodeKind::Text {
                style, paragraph, ..
            }) => ondin_core::variable::text_slot_number(slot, style, paragraph).is_some(),
            _ => false,
        },
        _ => true,
    }
}

/// Whether a field's drawn `cell` is the variable's `value`.
fn same(cell: &Cell, value: &Value) -> bool {
    match (cell, value) {
        (Cell::Color(a), Value::Color(b)) => a.to_rgba8() == b.to_rgba8(),
        (Cell::Number(a), Value::Number(b)) => (a - b).abs() <= 1e-6 * b.abs().max(1.0),
        (Cell::String(a), Value::String(b)) => a == b,
        (Cell::Boolean(a), Value::Boolean(b)) => a == b,
        _ => false,
    }
}

/// The `%` a paint drawing `drawn` binds a colour variable of value `var` at so
/// it draws the same: its alpha over the variable's. `None` when the colours
/// differ, or the paint is more opaque than the variable can draw.
fn share(drawn: Color, var: Color) -> Option<f32> {
    if drawn.with_alpha(1.0).to_rgba8() != var.with_alpha(1.0).to_rgba8() {
        return None;
    }
    let (d, v) = (drawn.components[3], var.components[3]);
    let k = if v > 0.0 { (d / v).min(1.0) } else { 1.0 };
    (var.multiply_alpha(k).to_rgba8() == drawn.to_rgba8()).then_some(k)
}

/// The solid colour paint field `key` draws, its alpha included; `None` for
/// any other field or brush.
fn drawn_solid(doc: &Document, key: FieldKey) -> Option<Color> {
    let FieldKey::Paint { id, slot } = key else {
        return None;
    };
    let (target, index) = super::binding::paint_item(slot)?;
    let p = doc.get(id)?.paint();
    let b = match target {
        PaintTarget::Fill => &p.fills.get(index)?.brush,
        PaintTarget::Stroke => &p.strokes.get(index)?.brush,
    };
    match doc.resolve_brush(id, b).into_owned() {
        ondin_core::Brush::Solid(c) => Some(c),
        _ => None,
    }
}

/// Whether `key`, drawing `cell`, draws the variable's `value`.
fn draws(doc: &Document, key: FieldKey, cell: &Cell, value: &Value) -> bool {
    match (key, value) {
        (FieldKey::Paint { .. }, Value::Color(v)) => {
            drawn_solid(doc, key).and_then(|c| share(c, *v)).is_some()
        }
        _ => same(cell, value),
    }
}

/// `key` on node `id` in place of its own.
fn on(key: FieldKey, id: NodeId) -> FieldKey {
    match key {
        FieldKey::Paint { slot, .. } => FieldKey::Paint { id, slot },
        FieldKey::Radius { .. } => FieldKey::Radius { id },
        FieldKey::Corner { slot, .. } => FieldKey::Corner { id, slot },
        FieldKey::Spacing { slots, .. } => FieldKey::Spacing { id, slots },
        FieldKey::Visible { .. } => FieldKey::Visible { id },
        FieldKey::Size { axis, .. } => FieldKey::Size { id, axis },
        FieldKey::Content { .. } => FieldKey::Content { id },
        FieldKey::Family { .. } => FieldKey::Family { id },
        FieldKey::Type { slot, .. } => FieldKey::Type { id, slot },
        FieldKey::Limit { slot, .. } => FieldKey::Limit { id, slot },
        FieldKey::Opacity { .. } => FieldKey::Opacity { id },
        FieldKey::StrokeWidth { item, .. } => FieldKey::StrokeWidth { id, item },
        FieldKey::EffectNumber { item, slot, .. } => FieldKey::EffectNumber { id, item, slot },
        FieldKey::Flow { slot, .. } => FieldKey::Flow { id, slot },
    }
}

/// The style a bind of `key` would detach: the one applied to the card the
/// field sits in.
fn style_over(n: &ondin_core::Node, key: FieldKey) -> Option<ondin_core::style::StyleId> {
    let s = n.styles();
    match key {
        FieldKey::Paint {
            slot: PaintSlot::Fill(_),
            ..
        } => s.fills,
        FieldKey::Paint {
            slot: PaintSlot::Stroke(_),
            ..
        }
        | FieldKey::StrokeWidth { .. } => s.strokes,
        FieldKey::EffectNumber { .. } => s.effects,
        FieldKey::Content { .. } => None,
        FieldKey::Family { .. } | FieldKey::Type { .. } => s.text,
        FieldKey::Spacing { .. } | FieldKey::Flow { .. } => s.layout,
        _ => None,
    }
}

/// The word a style's slot is called by in a sentence: *the Brand fill style*.
fn slot_word(key: FieldKey) -> &'static str {
    match key {
        FieldKey::Paint {
            slot: PaintSlot::Fill(_),
            ..
        } => "fill",
        FieldKey::Paint { .. } | FieldKey::StrokeWidth { .. } => "stroke",
        FieldKey::EffectNumber { .. } => "effect",
        FieldKey::Spacing { .. } | FieldKey::Flow { .. } => "layout",
        _ => "text",
    }
}

impl OndinApp {
    /// Every field outside `var` that draws its value and could be bound to it,
    /// in document order — see the module's doc for what is left out. A field
    /// of a family the variable's scope leaves out is listed too: binding it
    /// widens the scope.
    pub(crate) fn other_uses(&self, var: VarId) -> Vec<Use> {
        let doc = &self.session.doc;
        let Some(v) = doc.variables().get(var) else {
            return Vec::new();
        };
        let kind = v.kind;
        let scope = v.scope;
        // What may bind at all: never visibility, and a family only a scope
        // naming it takes (a flex word) only when this scope already does.
        let takes = |f: Field| {
            f.kind() == kind
                && f != Field::Visible
                && (!f.needs_scope() || scope.is_some_and(|s| s.contains(f)))
        };
        let locked = |id: NodeId| {
            std::iter::successors(doc.get(id), |n| n.parent().and_then(|p| doc.get(p)))
                .any(|n| n.locked())
        };
        // Every candidate first: copies are folded into their mains after.
        let mut found: Vec<(FieldKey, Field)> = Vec::new();
        // Every page's layers, top to bottom; the vault is no page's.
        let mut ids: Vec<NodeId> = Vec::new();
        for page in doc.page_ids() {
            let mut stack: Vec<NodeId> = doc
                .get(page)
                .map(|p| p.children().iter().rev().copied().collect())
                .unwrap_or_default();
            while let Some(id) = stack.pop() {
                ids.push(id);
                if let Some(n) = doc.get(id) {
                    stack.extend(n.children().iter().rev().copied());
                }
            }
        }
        for id in ids {
            let Some(n) = doc.get(id) else { continue };
            if n.link().is_some_and(|l| doc.in_vault(l)) {
                continue;
            }
            let Some(value) = doc.resolve_var(id, var) else {
                continue;
            };
            for key in keys_of(doc, n) {
                let field = key.field();
                if !takes(field) || self.field_var(key).is_some() {
                    continue;
                }
                let Some(cell) = self.field_value(key) else {
                    continue;
                };
                if resting(key, &cell) || !draws(doc, key, &cell, &value) || !literal(doc, key) {
                    continue;
                }
                let Some(bf) = key.bound() else { continue };
                if ondin_core::build::bind_field_ops(doc, &[id], bf, Some(var))
                    .0
                    .is_empty()
                {
                    continue;
                }
                found.push((key, field));
            }
        }
        // A copy whose main's same field is offered and can be bound follows
        // it; under a locked or styled main it is offered as its own.
        let is_found = |k: FieldKey| found.iter().any(|(f, _)| *f == k);
        let dead = |k: FieldKey| {
            locked(k.at()) || doc.get(k.at()).and_then(|n| style_over(n, k)).is_some()
        };
        let source = |k: FieldKey| {
            doc.get(k.at())
                .and_then(|n| n.link())
                .map(|l| on(k, l))
                .filter(|s| is_found(*s) && !dead(*s))
        };
        let mut out: Vec<Use> = Vec::new();
        for &(key, field) in &found {
            if source(key).is_some() {
                continue;
            }
            // The copies that follow it, through copies of copies.
            let follows = found
                .iter()
                .filter(|(k, _)| {
                    std::iter::successors(source(*k), |s| source(*s)).any(|s| s == key)
                })
                .count();
            let id = key.at();
            let n = doc.get(id);
            let why_not = if locked(id) {
                Some(format!(
                    "{} is locked — unlock it to bind",
                    crate::private::layer(doc, id)
                ))
            } else {
                n.and_then(|n| style_over(n, key))
                    .and_then(|s| doc.styles().get(s))
                    .map(|s| {
                        format!(
                            "{} uses the {} {} style — binding would detach it",
                            crate::private::layer(doc, id),
                            s.name,
                            slot_word(key)
                        )
                    })
            };
            // The lock first, as `why_not` names it.
            let tail = match (&why_not, n.and_then(|n| style_over(n, key))) {
                (Some(_), _) if locked(id) => "locked".to_string(),
                (Some(_), None) => "locked".to_string(),
                (Some(_), Some(s)) => format!(
                    "{} style {}",
                    slot_word(key),
                    doc.styles().get(s).map_or("", |s| s.name.as_str())
                ),
                (None, _) if follows > 0 => {
                    format!("main · {}", crate::ui::count(follows, "instance"))
                }
                (None, _) => doc
                    .page_of(id)
                    .map(|p| crate::private::page(doc, p).into_owned())
                    .unwrap_or_default(),
            };
            out.push(Use {
                key,
                field,
                follows,
                why_not,
                tail,
            });
        }
        out
    }

    /// Bind each of `keys` to `var` in one step, *Bind variable*, widening the
    /// variable's scope to the families it takes in; the status line says how
    /// many fields and how many layers now use it.
    pub(crate) fn bind_other_uses(&mut self, var: VarId, keys: &[FieldKey]) {
        // Read again: the list may have been open across an edit that changed
        // a field's value, or reordered a layer's fills under its key.
        let still: Vec<FieldKey> = self
            .other_uses(var)
            .into_iter()
            .filter(|u| u.why_not.is_none())
            .map(|u| u.key)
            .collect();
        let keys: Vec<FieldKey> = keys.iter().copied().filter(|k| still.contains(k)).collect();
        let keys = &keys[..];
        let doc = &self.session.doc;
        let Some(v) = doc.variables().get(var).cloned() else {
            return;
        };
        // Built one field at a time on a scratch, each against what the ones
        // before it leave: two fills of one layer are one list, and a second
        // write built against the document would take the first one back.
        let mut scratch = doc.clone();
        let mut ops: Vec<Operation> = Vec::new();
        let mut widened: Vec<Field> = Vec::new();
        if let Some(scope) = v.scope {
            let mut next = scope;
            for k in keys {
                let f = k.field();
                if !next.contains(f) {
                    next = next.with(f, true);
                    widened.push(f);
                }
            }
            if next != scope {
                let op = Operation::SetVariable {
                    var: ondin_core::Variable {
                        scope: Some(next),
                        ..v.clone()
                    },
                };
                if scratch.apply(&Transaction(vec![op.clone()])).is_err() {
                    return;
                }
                ops.push(op);
            }
        }
        // The fields bound, for the status line; copies following a main bound
        // here are counted in its layers, not as fields.
        let mut n = 0;
        for k in keys {
            let Some(bf) = k.bound() else { continue };
            // A paint takes the `%` that keeps it drawing as it did, not its
            // own alpha, which a translucent variable would multiply again.
            let tx = match (*k, scratch.resolve_var(k.at(), var)) {
                (FieldKey::Paint { slot, .. }, Some(Value::Color(v))) => {
                    let Some(alpha) = drawn_solid(&scratch, *k).and_then(|c| share(c, v)) else {
                        continue;
                    };
                    let Some((target, index)) = super::binding::paint_item(slot) else {
                        continue;
                    };
                    ondin_core::set_brush_at_all(&scratch, &[k.at()], target, index, |_, _| {
                        ondin_core::BrushSource::Bound { var, alpha }
                    })
                }
                _ => ondin_core::build::bind_field_ops(&scratch, &[k.at()], bf, Some(var)),
            };
            if tx.0.is_empty() || scratch.apply(&tx).is_err() {
                continue;
            }
            ops.extend(tx.0);
            n += 1;
        }
        if ops.is_empty() {
            return;
        }
        self.session
            .name_step("Bind variable", crate::theme::icon::LINK_SIMPLE);
        if !self.commit_edit(Transaction(ops)) {
            return;
        }
        let layers = ondin_core::variable::users(&self.session.doc, var)
            .into_iter()
            .filter(|u| !self.session.doc.in_vault(*u))
            .count();
        let mut toast = format!(
            "Bound {} to {n} more {} · {} {} it",
            v.name,
            if n == 1 { "field" } else { "fields" },
            crate::ui::count(layers, "layer"),
            if layers == 1 { "uses" } else { "use" }
        );
        if !widened.is_empty()
            && let Some(scope) = self.session.doc.variables().get(var).and_then(|v| v.scope)
        {
            let names: Vec<&str> = scope.fields().map(super::variables::field_label).collect();
            toast += &format!(" · its scope now takes {}", and_list(&names));
        }
        self.session.info(format!("{toast} · Ctrl+Z to undo"));
    }
}

/// *Fill*, *Fill and Stroke*, *Fill, Stroke and Opacity*.
fn and_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{Fill, Stroke, keyed_by_position};

    const BLUE: Color = Color::from_rgba8(47, 111, 216, 255);
    const RED: Color = Color::from_rgba8(200, 30, 30, 255);

    fn rect(id: NodeId, parent: NodeId, name: &str) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        }
    }

    fn fill(id: NodeId, c: Color, alpha: f32) -> Operation {
        Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: c.with_alpha(alpha).into(),
                visible: true,
            }]),
        }
    }

    /// **A field that stopped drawing the value is not bound**: the list may
    /// stay open across an edit, so the bind reads the uses again.
    ///
    /// flip: `bind_other_uses` binding its keys unchecked → fails at "red
    /// stays red".
    #[test]
    fn a_field_recoloured_since_the_list_opened_is_left_alone() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let page = app.session.doc.first_page();
        let [a, b] = [(); 2].map(|_| app.session.ids.mint());
        app.session
            .try_commit(Transaction(vec![
                rect(a, page, "A"),
                fill(a, BLUE, 1.0),
                rect(b, page, "B"),
                fill(b, BLUE, 1.0),
            ]))
            .unwrap();
        let key = FieldKey::Paint {
            id: a,
            slot: PaintSlot::Fill(0),
        };
        app.extract_field(key, app.field_value(key).unwrap());
        let var = app.field_var(key).unwrap();
        let keys: Vec<FieldKey> = app.other_uses(var).iter().map(|u| u.key).collect();
        assert_eq!(keys.len(), 1, "B is offered");
        app.session
            .try_commit(Transaction(vec![fill(b, RED, 1.0)]))
            .unwrap();
        app.bind_other_uses(var, &keys);
        let brush = &app.session.doc.get(b).unwrap().paint().fills[0].brush;
        assert_eq!(brush.var(), None, "red stays red");
    }

    /// **The other uses are the fields drawing the value, and they bind in
    /// one step**: extracting A's blue offers B (a 50% fill of the same blue),
    /// the main M once with its instance following, the locked L disabled, and
    /// S's stroke under its own family. Binding B, M and S binds the instance
    /// with M, widens the scope to Fill and Stroke, and undoes as one.
    ///
    /// flip: `other_uses` dropping its copy-folding (`source` always `None`)
    /// → fails at "B, M, L and S's stroke", the instance counted as a fifth.
    #[test]
    fn other_uses_are_found_and_bound_in_one_step() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let page = app.session.doc.first_page();
        let [a, b, c, l, s, m] = [(); 6].map(|_| app.session.ids.mint());
        let stroke = |c: Color| {
            keyed_by_position([Stroke {
                brush: c.into(),
                width: 1.0,
                ..Stroke::default()
            }])
        };
        app.session
            .try_commit(Transaction(vec![
                rect(a, page, "A"),
                fill(a, BLUE, 1.0),
                rect(b, page, "B"),
                fill(b, BLUE, 0.5),
                rect(c, page, "C"),
                fill(c, RED, 1.0),
                rect(l, page, "L"),
                fill(l, BLUE, 1.0),
                Operation::SetLocked {
                    id: l,
                    locked: true,
                },
                rect(s, page, "S"),
                fill(s, RED, 1.0),
                Operation::SetStrokes {
                    id: s,
                    strokes: stroke(BLUE),
                },
                Operation::CreateNode {
                    id: m,
                    parent: page,
                    index: 0,
                    kind: NodeKind::Artboard {
                        size: Size::new(40.0, 20.0),
                        corner_radii: RoundedRectRadii::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: Some("M".into()),
                },
                fill(m, BLUE, 1.0),
                Operation::SetComponent {
                    id: m,
                    component: true,
                },
            ]))
            .expect("the layers");
        let doc = &app.session.doc;
        let (tx, made) = ondin_core::insert_subtrees(
            doc,
            &mut app.session.ids,
            &[ondin_core::Placement {
                nodes: doc.capture_subtree(m).unwrap(),
                parent: page,
                index: None,
            }],
            Default::default(),
        );
        assert!(app.commit_edit(tx));
        let inst = made[0];
        assert_eq!(app.session.doc.get(inst).unwrap().link(), Some(m));

        let key = FieldKey::Paint {
            id: a,
            slot: PaintSlot::Fill(0),
        };
        let value = app.field_value(key).unwrap();
        app.extract_field(key, value);
        let var = app.field_var(key).expect("A is bound");
        assert_eq!(
            app.var_browser.as_ref().and_then(|b| b.offer),
            Some((var, 4)),
            "B, M, L and S's stroke"
        );
        let uses = app.other_uses(var);
        let row = |id: NodeId| uses.iter().find(|u| u.key.at() == id);
        assert!(row(inst).is_none(), "the instance follows its main");
        assert_eq!(
            row(m).map(|u| (u.follows, u.tail.as_str())),
            Some((1, "main · 1 instance"))
        );
        assert!(
            row(l)
                .unwrap()
                .why_not
                .as_deref()
                .unwrap()
                .contains("locked")
        );
        assert_eq!(row(s).unwrap().field, Field::Stroke);
        assert!(row(c).is_none(), "red is not blue");

        let before = app.session.doc.clone();
        let keys: Vec<FieldKey> = [b, m, s].iter().map(|id| row(*id).unwrap().key).collect();
        app.bind_other_uses(var, &keys);
        let doc = &app.session.doc;
        let bound = |id: NodeId, stroke: bool| {
            let p = doc.get(id).unwrap().paint();
            match stroke {
                true => p.strokes[0].brush.var(),
                false => p.fills[0].brush.var(),
            }
        };
        assert_eq!(bound(b, false), Some(var));
        assert_eq!(bound(m, false), Some(var));
        assert_eq!(bound(inst, false), Some(var), "with its main");
        assert_eq!(bound(s, true), Some(var));
        assert_eq!(bound(l, false), None);
        let scope = doc.variables().get(var).unwrap().scope.unwrap();
        assert!(scope.contains(Field::Fill) && scope.contains(Field::Stroke));
        assert_eq!(
            doc.resolve_brush(b, &doc.get(b).unwrap().paint().fills[0].brush)
                .into_owned(),
            ondin_core::Brush::Solid(BLUE.with_alpha(0.5)),
            "B draws as before"
        );
        let toast = app.session.status().text.clone();
        assert!(
            toast.contains("its scope now takes Fill and Stroke"),
            "{toast}"
        );
        app.undo();
        assert_eq!(app.session.doc, before, "one undo");
    }

    /// **A translucent colour variable matches the paints it can draw**: a 50%
    /// blue variable offers a 50% blue fill (bound at 100%) and a 25% one (at
    /// 50%), not an opaque blue, and both draw as before once bound.
    ///
    /// flip: `bind_other_uses` keeping each fill's own alpha (`bind_field_ops`)
    /// → fails at "A draws as before".
    #[test]
    fn a_translucent_variable_binds_the_fills_it_draws() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let page = app.session.doc.first_page();
        let [a, b, c] = [(); 3].map(|_| app.session.ids.mint());
        app.session
            .try_commit(Transaction(vec![
                rect(a, page, "A"),
                fill(a, BLUE, 0.5),
                rect(b, page, "B"),
                fill(b, BLUE, 0.25),
                rect(c, page, "C"),
                fill(c, BLUE, 1.0),
            ]))
            .unwrap();
        let key = |id| FieldKey::Paint {
            id,
            slot: PaintSlot::Fill(0),
        };
        // Extract C's opaque blue, make the variable 50% blue, and give C its
        // own opaque blue back.
        app.extract_field(key(c), app.field_value(key(c)).unwrap());
        let var = app.field_var(key(c)).unwrap();
        let mut v = app.session.doc.variables().get(var).unwrap().clone();
        for cell in v.cells.values_mut() {
            *cell = Cell::Color(BLUE.with_alpha(0.5));
        }
        app.session
            .try_commit(Transaction(vec![
                Operation::SetVariable { var: v },
                Operation::SetFills {
                    id: c,
                    fills: keyed_by_position([Fill {
                        brush: BLUE.into(),
                        visible: true,
                    }]),
                },
            ]))
            .unwrap();
        let keys: Vec<FieldKey> = app.other_uses(var).iter().map(|u| u.key).collect();
        let mut at: Vec<NodeId> = keys.iter().map(|k| k.at()).collect();
        at.sort();
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(at, want, "A and B, not opaque C");
        app.bind_other_uses(var, &keys);
        let doc = &app.session.doc;
        let drawn = |id: NodeId| {
            doc.resolve_brush(id, &doc.get(id).unwrap().paint().fills[0].brush)
                .into_owned()
        };
        assert_eq!(doc.get(a).unwrap().paint().fills[0].brush.var(), Some(var));
        assert_eq!(
            drawn(a),
            ondin_core::Brush::Solid(BLUE.with_alpha(0.5)),
            "A draws as before"
        );
        assert_eq!(
            drawn(b),
            ondin_core::Brush::Solid(BLUE.with_alpha(0.25)),
            "B draws as before"
        );
    }

    #[test]
    fn and_list_joins_the_last_with_and() {
        assert_eq!(and_list(&["Fill"]), "Fill");
        assert_eq!(and_list(&["Fill", "Stroke"]), "Fill and Stroke");
        assert_eq!(and_list(&["A", "B", "C"]), "A, B and C");
    }
}
