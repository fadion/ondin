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
//! Left out: fields already bound, the vault, kit instances and the fields a
//! copy swapped to a vault main takes from it (their mains belong to the kit,
//! and its next update would drop the binding), visibility,
//! and the resting values every layer has — an opacity of 100% and a zero
//! radius, gap, padding, limit or effect number — which would list every
//! layer in the document. A copy that draws the value because its main does is
//! offered once, as the main's field, with the copies counted (a swapped copy
//! follows its swap but for its size and limits); a copy that
//! overrides it is its own row, and so is one in modes where the main's `%`
//! would draw it otherwise. A main is bound only with each such row, and is
//! dead, with its reason, where a copy following it would change and is no row
//! of its own. A chart frame's padding and flow are its regeneration's and are
//! never offered.

use super::binding::{COLUMN_GAP, FieldKey, PAD_BOTTOM, PAD_LEFT, PAD_RIGHT, PAD_TOP, ROW_GAP};
use super::paint::PaintSlot;
use crate::app::OndinApp;
use ondin_core::peniko::Color;
use ondin_core::variable::{Cell, Field, Scope, Value, VarId};
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
    /// The copies' own rows this main's bind would change unless they are
    /// bound with it: a main is bound only together with each of them.
    pub needs: Vec<FieldKey>,
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
    // A chart's layout is its regeneration's: its frame's padding and flow
    // are never bound by hand.
    if doc.resolved_display(id).is_some() && n.chart().is_none() {
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

/// Whether a field's drawn `cell` is the variable's `value`; a colour is
/// matched by [`draws`], only paints taking one.
fn same(cell: &Cell, value: &Value) -> bool {
    match (cell, value) {
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

/// Whether `key` is a field a swapped copy keeps from its slot rather than its
/// swap: its size, its size limits and its visibility, whose binds write
/// `SetSizeVars`, `SetLayoutItem` and `SetVisibleVar` (`swap::is_slot_field`).
fn slot_field(key: FieldKey) -> bool {
    matches!(
        key,
        FieldKey::Size { .. } | FieldKey::Limit { .. } | FieldKey::Visible { .. }
    )
}

/// The node copy `n` takes `key` from as propagation sends it: its link for a
/// slot field, else its swap where it has one.
fn copied_from(n: &ondin_core::Node, key: FieldKey) -> Option<NodeId> {
    match slot_field(key) {
        true => n.link(),
        false => n.swap().or(n.link()),
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

/// Which of `uses` start ticked in the list, and are what the offer line's
/// one-click bind takes: those that can be bound, of a family `scope` already
/// takes, so neither binds a locked layer or widens the scope unasked. A main
/// starts ticked only with every copy row it needs (`Use::needs`), as the
/// bind takes it only with them, so the offer counts what it binds.
pub(crate) fn ticked_by_default(uses: &[Use], scope: Option<Scope>) -> Vec<bool> {
    let own: Vec<bool> = uses
        .iter()
        .map(|u| u.why_not.is_none() && scope.is_none_or(|s| s.contains(u.field)))
        .collect();
    let ticked = |k: &FieldKey| {
        uses.iter()
            .zip(&own)
            .any(|(other, t)| *t && other.key == *k)
    };
    uses.iter()
        .zip(&own)
        .map(|(u, t)| *t && u.needs.iter().all(ticked))
        .collect()
}

/// Tick or untick row `i` of the list, keeping the ticks whole under
/// `Use::needs` as the kit update review does: ticking a main ticks the copy
/// rows it needs, and unticking a copy row unticks the mains that need it.
/// Dead rows are left as they are.
pub(crate) fn toggle(uses: &[Use], on: &mut [bool], i: usize) {
    let to = !on[i];
    on[i] = to;
    let mut stack = vec![i];
    while let Some(j) = stack.pop() {
        let linked = |u: &Use| match to {
            true => uses[j].needs.contains(&u.key),
            false => u.needs.contains(&uses[j].key),
        };
        for (k, u) in uses.iter().enumerate() {
            if on[k] != to && u.why_not.is_none() && linked(u) {
                on[k] = to;
                stack.push(k);
            }
        }
    }
}

/// The fields a bind of the ticked rows binds, as [`OndinApp::bind_other_uses`]
/// takes them: the live ticked rows, less a main any of whose needed copy rows
/// is not among them — the popover's *Bind N* counts these.
pub(crate) fn to_bind(uses: &[Use], on: &[bool]) -> Vec<FieldKey> {
    let mut keys: Vec<FieldKey> = uses
        .iter()
        .zip(on)
        .filter(|(u, on)| **on && u.why_not.is_none())
        .map(|(u, _)| u.key)
        .collect();
    // Again until nothing drops, as a dropped row may be another's need.
    loop {
        let kept: Vec<FieldKey> = (keys.iter().copied())
            .filter(|k| {
                uses.iter()
                    .find(|u| u.key == *k)
                    .is_none_or(|u| u.needs.iter().all(|n| keys.contains(n)))
            })
            .collect();
        if kept.len() == keys.len() {
            return keys;
        }
        keys = kept;
    }
}

/// The offer line under a variable *Extract into variable…* just made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Offer {
    pub var: VarId,
    /// The fields its one-click bind takes ([`ticked_by_default`]).
    pub ready: usize,
    /// Every field drawing the value, the list's rows.
    pub total: usize,
}

impl OndinApp {
    /// The offer line for `var`, or `None` when no other field draws its value.
    pub(crate) fn offer_of(&self, var: VarId) -> Option<Offer> {
        let uses = self.other_uses(var);
        let scope = self.session.doc.variables().get(var).and_then(|v| v.scope);
        let ready = ticked_by_default(&uses, scope)
            .into_iter()
            .filter(|t| *t)
            .count();
        (!uses.is_empty()).then_some(Offer {
            var,
            ready,
            total: uses.len(),
        })
    }

    /// Bind `var` to the fields the list would start with ticked: the offer
    /// line's one click.
    pub(crate) fn bind_ready_uses(&mut self, var: VarId) {
        let uses = self.other_uses(var);
        let scope = self.session.doc.variables().get(var).and_then(|v| v.scope);
        let keys: Vec<FieldKey> = uses
            .iter()
            .zip(ticked_by_default(&uses, scope))
            .filter(|(_, t)| *t)
            .map(|(u, _)| u.key)
            .collect();
        self.bind_other_uses(var, &keys);
    }

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
        let mut found: Vec<(FieldKey, Field, Option<f32>)> = Vec::new();
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
        for &id in &ids {
            let Some(n) = doc.get(id) else { continue };
            // A chart's parts are written only by its regeneration.
            if n.link().is_some_and(|l| doc.in_vault(l))
                || ondin_core::chart::owner(doc, id).is_some()
            {
                continue;
            }
            let Some(value) = doc.resolve_var(id, var) else {
                continue;
            };
            for key in keys_of(doc, n) {
                // A copy swapped to a vault main takes this field from the kit.
                if copied_from(n, key).is_some_and(|s| doc.in_vault(s)) {
                    continue;
                }
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
                // The `%` a paint binds at, read in this layer's modes.
                let alpha = match &value {
                    Value::Color(v) => drawn_solid(doc, key).and_then(|c| share(c, *v)),
                    _ => None,
                };
                found.push((key, field, alpha));
            }
        }
        // A copy whose main's same field is offered and can be bound follows
        // it; under a locked or styled main it is offered as its own, and so
        // is one in other modes that the main's `%` would not draw the same.
        let found_at = |k: FieldKey| found.iter().find(|(f, ..)| *f == k).map(|x| x.2);
        let is_found = |k: FieldKey, alpha: Option<f32>| found_at(k).is_some_and(|a| a == alpha);
        let dead = |k: FieldKey| {
            locked(k.at()) || doc.get(k.at()).and_then(|n| style_over(n, k)).is_some()
        };
        let source = |k: FieldKey| {
            let alpha = found_at(k).flatten();
            doc.get(k.at())
                .and_then(|n| copied_from(n, k))
                .map(|l| on(k, l))
                .filter(|s| is_found(*s, alpha) && !dead(*s))
        };
        // Each layer's copies one step down, as propagation sends a field: a
        // swapped copy takes its slot fields from its link and the rest from
        // its swap. A main's binding reaches every copy that follows the
        // field, found or not.
        let mut copies: [std::collections::HashMap<NodeId, Vec<NodeId>>; 2] = Default::default();
        for &id in &ids {
            let Some(n) = doc.get(id) else { continue };
            if let Some(l) = n.link() {
                copies[1].entry(l).or_default().push(id);
            }
            if let Some(s) = n.swap().or(n.link()) {
                copies[0].entry(s).or_default().push(id);
            }
        }
        let copies_of = |k: FieldKey, at: NodeId| copies[usize::from(slot_field(k))].get(&at);
        // The copies following `key` that binding it would change: a copy
        // that does not draw the variable at the main's `%` in its own modes.
        // `Err` when one is no row of its own, else those rows, which must be
        // bound with it, each at its own `%`.
        let changed_copies = |key: FieldKey| -> Result<Vec<FieldKey>, ()> {
            let Some(cell) = self.field_value(key) else {
                return Ok(Vec::new());
            };
            let alpha = found_at(key).flatten();
            let mut stack = copies_of(key, key.at()).cloned().unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            let mut rows = Vec::new();
            while let Some(c) = stack.pop() {
                if !seen.insert(c) {
                    continue;
                }
                let ck = on(key, c);
                if self.field_var(ck).is_some() || self.field_value(ck).as_ref() != Some(&cell) {
                    continue;
                }
                if !is_found(ck, alpha) {
                    match found_at(ck).is_some() {
                        true => rows.push(ck),
                        false => return Err(()),
                    }
                }
                stack.extend(copies_of(key, c).into_iter().flatten().copied());
            }
            Ok(rows)
        };
        let mut out: Vec<Use> = Vec::new();
        for &(key, field, _) in &found {
            if source(key).is_some() {
                continue;
            }
            // The copies that follow it, through copies of copies.
            let follows = found
                .iter()
                .filter(|(k, ..)| {
                    std::iter::successors(source(*k), |s| source(*s)).any(|s| s == key)
                })
                .count();
            let id = key.at();
            let n = doc.get(id);
            let changed = changed_copies(key);
            let moves_copy =
                !locked(id) && n.and_then(|n| style_over(n, key)).is_none() && changed.is_err();
            let why_not = if locked(id) {
                Some(format!(
                    "{} is locked — unlock it to bind",
                    crate::private::layer(doc, id)
                ))
            } else if moves_copy {
                Some(format!(
                    "An instance of {} is in another mode — binding would change it",
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
                (Some(_), _) if moves_copy => "instance in another mode".to_string(),
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
                needs: changed.unwrap_or_default(),
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
        // A main whose bind would change a copy's row left unticked stays as it
        // is: nothing on screen changes.
        let uses = self.other_uses(var);
        let on: Vec<bool> = uses.iter().map(|u| keys.contains(&u.key)).collect();
        let keys = &to_bind(&uses, &on)[..];
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
        let mut bound: Vec<Field> = Vec::new();
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
            bound.push(k.field());
            n += 1;
        }
        // The scope widens only by the families a bind landed in.
        if let Some(scope) = v.scope
            && widened.iter().any(|f| !bound.contains(f))
        {
            widened.retain(|f| bound.contains(f));
            let next = widened.iter().fold(scope, |s, f| s.with(*f, true));
            match next == scope {
                true => {
                    ops.remove(0);
                }
                false => {
                    ops[0] = Operation::SetVariable {
                        var: ondin_core::Variable {
                            scope: Some(next),
                            ..v.clone()
                        },
                    }
                }
            }
        }
        if n == 0 {
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
            Some(super::Offer {
                var,
                ready: 2,
                total: 4
            }),
            "B, M, L and S's stroke; B and M ready, L being locked and S out of scope"
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

    /// **A copy in other modes is its own row when the main's `%` would not draw
    /// it the same**: `overlay` is black 40% in Light and 60% in Dark; a 40%
    /// black main in Light binds at 100%, its copy in Dark needs 67%, so the
    /// copy is offered apart and both draw 40% once bound.
    ///
    /// flip: `other_uses`'s `source` ignoring the alpha (`is_found` by key
    /// alone) → fails at "the copy is its own row".
    #[test]
    fn a_copy_needing_another_share_is_its_own_row() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let (var, m, inst) = overlay_main_with_a_dark_copy(&ctx, &mut app);
        let uses = app.other_uses(var);
        let mut at: Vec<NodeId> = uses.iter().map(|u| u.key.at()).collect();
        at.sort();
        let mut want = vec![m, inst];
        want.sort();
        assert_eq!(at, want, "the copy is its own row");
        let keys: Vec<FieldKey> = uses.iter().map(|u| u.key).collect();
        app.bind_other_uses(var, &keys);
        let doc = &app.session.doc;
        for id in [m, inst] {
            let brush = &doc.get(id).unwrap().paint().fills[0].brush;
            assert_eq!(brush.var(), Some(var));
            assert_eq!(
                doc.resolve_brush(id, brush).into_owned().to_owned(),
                ondin_core::Brush::Solid(Color::BLACK.with_alpha(0.4)),
                "{id:?} draws as before"
            );
        }
    }

    /// **The popover's ticks stay whole**: ticking the main ticks the copy row
    /// it needs, unticking that copy unticks the main, and *Bind N* counts
    /// only what the bind binds — a main ticked without its copy is not.
    ///
    /// flip: `toggle` not following `needs` → fails at "the copy comes with
    /// it"; `to_bind` keeping a main without its needs → fails at "counts
    /// nothing".
    #[test]
    fn a_main_and_the_copy_it_needs_tick_together() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let (var, m, inst) = overlay_main_with_a_dark_copy(&ctx, &mut app);
        let uses = app.other_uses(var);
        let row = |id: NodeId| uses.iter().position(|u| u.key.at() == id).unwrap();
        let (mi, ci) = (row(m), row(inst));
        assert_eq!(uses[mi].needs, vec![uses[ci].key], "the fixture: M needs C");
        let mut on = vec![false; uses.len()];
        toggle(&uses, &mut on, mi);
        assert!(on[ci], "the copy comes with it");
        toggle(&uses, &mut on, ci);
        assert!(!on[mi], "unticking the copy unticks the main");
        on[mi] = true;
        assert!(to_bind(&uses, &on).is_empty(), "counts nothing");
        app.bind_other_uses(var, &[uses[mi].key]);
        let brush = &app.session.doc.get(m).unwrap().paint().fills[0].brush;
        assert_eq!(brush.var(), None, "and binds nothing");
    }

    /// `overlay`, black 40% in Light and 60% in Dark; a main M filled a 40%
    /// black in Light, and its instance pinned to Dark: `(var, M, instance)`.
    fn overlay_main_with_a_dark_copy(
        ctx: &egui::Context,
        app: &mut OndinApp,
    ) -> (VarId, NodeId, NodeId) {
        use ondin_core::variable::{Mode, VarKind, Variable};
        let _ = ctx.run_ui(Default::default(), |_| {});
        let page = app.session.doc.first_page();
        let m = app.session.ids.mint();
        let (light, dark) = (app.session.ids.mint_mode(), app.session.ids.mint_mode());
        let var = app.session.ids.mint_var();
        let black = |a: f32| Color::BLACK.with_alpha(a);
        let add = |id, name: &str, index| Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id,
                name: name.into(),
            },
            index,
            cells: Vec::new(),
        };
        app.session
            .try_commit(Transaction(vec![
                add(light, "Light", 0),
                add(dark, "Dark", 1),
                Operation::AddVariable {
                    var: Variable {
                        id: var,
                        name: "overlay".into(),
                        kind: VarKind::Color,
                        cells: [
                            (light, Cell::Color(black(0.4))),
                            (dark, Cell::Color(black(0.6))),
                        ]
                        .into(),
                        scope: None,
                        origin: None,
                        forked: None,
                        description: String::new(),
                    },
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
                fill(m, Color::BLACK, 0.4),
                Operation::SetComponent {
                    id: m,
                    component: true,
                },
            ]))
            .expect("the main");
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
        app.session
            .try_commit(Transaction(vec![Operation::SetModes {
                id: inst,
                modes: vec![dark],
            }]))
            .expect("the copy in Dark");
        (var, m, inst)
    }

    /// **A main whose bind would recolour a copy in another mode is not
    /// offered**: `brand` is red in Light and blue in Dark; the main's literal
    /// red draws `brand` in Light, but its instance in Dark draws red too, and
    /// binding the main would turn it blue. The main's row is dead with its
    /// reason, and the one-click bind leaves the instance red.
    ///
    /// flip: `other_uses` ignoring `changed_copies` → fails at "dead, with its
    /// reason".
    #[test]
    fn a_main_whose_copy_would_change_is_not_bound() {
        use ondin_core::variable::{Mode, VarKind, Variable};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let page = app.session.doc.first_page();
        let m = app.session.ids.mint();
        let (light, dark) = (app.session.ids.mint_mode(), app.session.ids.mint_mode());
        let var = app.session.ids.mint_var();
        let add = |id, name: &str, index| Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id,
                name: name.into(),
            },
            index,
            cells: Vec::new(),
        };
        app.session
            .try_commit(Transaction(vec![
                add(light, "Light", 0),
                add(dark, "Dark", 1),
                Operation::AddVariable {
                    var: Variable {
                        id: var,
                        name: "brand".into(),
                        kind: VarKind::Color,
                        cells: [(light, Cell::Color(RED)), (dark, Cell::Color(BLUE))].into(),
                        scope: None,
                        origin: None,
                        forked: None,
                        description: String::new(),
                    },
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
                fill(m, RED, 1.0),
                Operation::SetComponent {
                    id: m,
                    component: true,
                },
            ]))
            .expect("the main");
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
        app.session
            .try_commit(Transaction(vec![Operation::SetModes {
                id: inst,
                modes: vec![dark],
            }]))
            .expect("the copy in Dark");
        let uses = app.other_uses(var);
        let [u] = &uses[..] else {
            panic!("the main's row alone: {uses:?}")
        };
        assert_eq!(u.key.at(), m);
        assert!(u.why_not.is_some(), "dead, with its reason");
        app.bind_ready_uses(var);
        let doc = &app.session.doc;
        let brush = &doc.get(inst).unwrap().paint().fills[0].brush;
        assert_eq!(
            doc.resolve_brush(inst, brush).into_owned().to_owned(),
            ondin_core::Brush::Solid(RED),
            "the instance stays red"
        );
    }

    /// **A swapped copy follows its swap's fill**: Holder holds an instance n
    /// of Star; Holder's instance h, in Dark, has its copy r of n swapped to M,
    /// so r draws M's red. Binding M to `brand` would turn r blue, so M's row
    /// is dead and the one-click bind leaves r red.
    ///
    /// flip: `other_uses`'s content copies keyed by the link alone (n, not M)
    /// → fails at "dead, with its reason".
    #[test]
    fn a_main_whose_swapped_copy_would_change_is_not_bound() {
        use ondin_core::variable::{Mode, VarKind, Variable};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let page = app.session.doc.first_page();
        let [m, star, holder] = [(); 3].map(|_| app.session.ids.mint());
        let (light, dark) = (app.session.ids.mint_mode(), app.session.ids.mint_mode());
        let var = app.session.ids.mint_var();
        let add = |id, name: &str, index| Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id,
                name: name.into(),
            },
            index,
            cells: Vec::new(),
        };
        let frame = |id, name: &str| Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        };
        let main = |id| Operation::SetComponent {
            id,
            component: true,
        };
        app.session
            .try_commit(Transaction(vec![
                add(light, "Light", 0),
                add(dark, "Dark", 1),
                Operation::AddVariable {
                    var: Variable {
                        id: var,
                        name: "brand".into(),
                        kind: VarKind::Color,
                        cells: [(light, Cell::Color(RED)), (dark, Cell::Color(BLUE))].into(),
                        scope: None,
                        origin: None,
                        forked: None,
                        description: String::new(),
                    },
                },
                frame(m, "M"),
                fill(m, RED, 1.0),
                main(m),
                frame(star, "Star"),
                main(star),
                frame(holder, "Holder"),
            ]))
            .expect("the mains");
        let place = |app: &mut OndinApp, main: NodeId, parent: NodeId| {
            let doc = &app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut app.session.ids,
                &[ondin_core::Placement {
                    nodes: doc.capture_subtree(main).unwrap(),
                    parent,
                    index: None,
                }],
                Default::default(),
            );
            assert!(app.commit_edit(tx));
            made[0]
        };
        let n = place(&mut app, star, holder);
        app.session
            .try_commit(Transaction(vec![main(holder)]))
            .expect("Holder a main");
        let h = place(&mut app, holder, page);
        app.session
            .try_commit(Transaction(vec![Operation::SetModes {
                id: h,
                modes: vec![dark],
            }]))
            .expect("h in Dark");
        let r = app.session.doc.get(h).unwrap().children()[0];
        assert_eq!(
            app.session.doc.get(r).unwrap().link(),
            Some(n),
            "the fixture: r"
        );
        let swap = ondin_core::swap::swap(&app.session.doc, r, m).expect("r swaps to M");
        assert!(app.commit_edit(swap));
        let red = |app: &OndinApp| {
            let doc = &app.session.doc;
            let brush = &doc.get(r).unwrap().paint().fills[0].brush;
            doc.resolve_brush(r, brush).into_owned() == ondin_core::Brush::Solid(RED)
        };
        assert!(red(&app), "the fixture: r draws M's red");
        let uses = app.other_uses(var);
        let [u] = &uses[..] else {
            panic!("M's row alone: {uses:?}")
        };
        assert_eq!(u.key.at(), m);
        assert!(u.why_not.is_some(), "dead, with its reason");
        app.bind_ready_uses(var);
        assert!(red(&app), "r stays red");
    }

    /// **A copy swapped to a vault main takes its fill from the kit**: Holder
    /// holds an instance n of Star; Holder's instance h has its copy r of n
    /// swapped to M, a main in the vault, so r draws M's red. r's fill is not
    /// offered, as M's is not.
    ///
    /// flip: `other_uses` without its per-key vault test → fails at "nothing
    /// offered", r's fill listed.
    #[test]
    fn a_copy_swapped_to_a_vault_main_is_not_offered() {
        use ondin_core::variable::{Mode, VarKind, Variable};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let page = app.session.doc.first_page();
        let [vault, m, star, holder] = [(); 4].map(|_| app.session.ids.mint());
        let light = app.session.ids.mint_mode();
        let var = app.session.ids.mint_var();
        let frame = |id, parent, name: &str| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        };
        let main = |id| Operation::SetComponent {
            id,
            component: true,
        };
        app.session
            .try_commit(Transaction(vec![
                Operation::AddMode {
                    collection: ondin_core::variable::CollectionId::FIRST,
                    mode: Mode {
                        id: light,
                        name: "Light".into(),
                    },
                    index: 0,
                    cells: Vec::new(),
                },
                Operation::AddVariable {
                    var: Variable {
                        id: var,
                        name: "brand".into(),
                        kind: VarKind::Color,
                        cells: [(light, Cell::Color(RED))].into(),
                        scope: None,
                        origin: None,
                        forked: None,
                        description: String::new(),
                    },
                },
                Operation::OpenVault { id: vault },
                frame(m, vault, "M"),
                fill(m, RED, 1.0),
                main(m),
                Operation::SetOrigin {
                    id: m,
                    origin: Some(ondin_core::kit::Origin {
                        kit: "0".repeat(32),
                        node: m,
                        version: 1,
                        name: Some("Kit".into()),
                    }),
                },
                frame(star, page, "Star"),
                main(star),
                frame(holder, page, "Holder"),
            ]))
            .expect("the mains");
        let place = |app: &mut OndinApp, main: NodeId, parent: NodeId| {
            let doc = &app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut app.session.ids,
                &[ondin_core::Placement {
                    nodes: doc.capture_subtree(main).unwrap(),
                    parent,
                    index: None,
                }],
                Default::default(),
            );
            assert!(app.commit_edit(tx));
            made[0]
        };
        let n = place(&mut app, star, holder);
        app.session
            .try_commit(Transaction(vec![main(holder)]))
            .expect("Holder a main");
        let h = place(&mut app, holder, page);
        let r = app.session.doc.get(h).unwrap().children()[0];
        assert_eq!(
            app.session.doc.get(r).unwrap().link(),
            Some(n),
            "the fixture: r"
        );
        let swap = ondin_core::swap::swap(&app.session.doc, r, m).expect("r swaps to M");
        assert!(app.commit_edit(swap));
        let doc = &app.session.doc;
        let brush = &doc.get(r).unwrap().paint().fills[0].brush;
        assert_eq!(
            doc.resolve_brush(r, brush).into_owned(),
            ondin_core::Brush::Solid(RED),
            "the fixture: r draws M's red"
        );
        let uses = app.other_uses(var);
        assert!(uses.is_empty(), "nothing offered: {uses:?}");
    }

    /// **A chart frame's padding is its regeneration's**: a chart's generated
    /// layout offers no Spacing or Flow key.
    ///
    /// flip: `keys_of` without its chart test → its Spacing and Flow keys.
    #[test]
    fn a_chart_frames_layout_is_not_offered() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        use ondin_core::chart::{Chart, ChartKind};
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(0.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        let doc = &app.session.doc;
        let mut stack = vec![doc.first_page()];
        let mut chart = None;
        while let Some(id) = stack.pop() {
            let n = doc.get(id).unwrap();
            if n.chart().is_some() {
                chart = Some(id);
                break;
            }
            stack.extend(n.children().iter().copied());
        }
        let chart = chart.expect("the chart");
        assert!(
            app.session.doc.resolved_display(chart).is_some(),
            "the fixture: a chart frame lays out"
        );
        let n = app.session.doc.get(chart).unwrap();
        let keys = keys_of(&app.session.doc, n);
        assert!(
            !keys
                .iter()
                .any(|k| matches!(k, FieldKey::Spacing { .. } | FieldKey::Flow { .. })),
            "{keys:?}"
        );
    }

    #[test]
    fn and_list_joins_the_last_with_and() {
        assert_eq!(and_list(&["Fill"]), "Fill");
        assert_eq!(and_list(&["Fill", "Stroke"]), "Fill and Stroke");
        assert_eq!(and_list(&["A", "B", "C"]), "A, B and C");
    }
}
