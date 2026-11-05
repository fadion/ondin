//! Colour review: the near-identical colours of a scope, grouped by OKLab
//! distance, and the transaction that merges a group into one colour.
//!
//! **Every raw colour counts**: solid fills and strokes, gradient stops, shadow
//! colours, and a text layer's own colour and its runs'. A fill or stroke bound to
//! a colour variable is a member too, as that variable; it is never merged away.
//! Only fills and strokes bind, so a merge into a variable binds those and writes
//! the variable's value into the other places ([`Merged::unbound`]).
//!
//! **A group grows around its anchor**, the most-used colour not already taken,
//! and every member is within the limit of the anchor itself, so A≈B≈C cannot
//! chain. Alpha is compared apart: two members always share their alpha byte.

use crate::Brush;
use crate::build::subtree_nodes;
use crate::document::Document;
use crate::effect::EffectKind;
use crate::id::NodeId;
use crate::node::{Fill, NodeKind, Stroke};
use crate::op::{Operation, Transaction};
use crate::typography::{CharAttr, Span, Spans};
use crate::variable::{BrushSource, Cell, ModeId, Value, VarId, VarKind, Variable};
use peniko::Color;
use peniko::color::{DynamicColor, Oklab, Srgb};

/// How far apart two colours may be and still be one group, in OKLab distance ×100.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Threshold {
    Tight,
    #[default]
    Close,
    Loose,
}

impl Threshold {
    pub const ALL: [Threshold; 3] = [Threshold::Tight, Threshold::Close, Threshold::Loose];

    /// The largest [`delta`] a member may sit from its anchor.
    pub fn limit(self) -> f64 {
        match self {
            Threshold::Tight => 1.0,
            Threshold::Close => 2.0,
            Threshold::Loose => 4.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Threshold::Tight => "Tight",
            Threshold::Close => "Close",
            Threshold::Loose => "Loose",
        }
    }
}

/// The OKLab distance between two colours' RGB, ×100; alpha is not compared.
pub fn delta(a: Color, b: Color) -> f64 {
    let (p, q) = (a.convert::<Oklab>(), b.convert::<Oklab>());
    (0..3)
        .map(|k| f64::from(p.components[k] - q.components[k]).powi(2))
        .sum::<f64>()
        .sqrt()
        * 100.0
}

/// One distinct colour of a scope: a raw RGBA, or a colour variable as the fills
/// and strokes bound to it draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Swatch {
    /// As drawn in the default mode, a bound one's binding alpha applied.
    pub color: Color,
    /// The variable a bound member is; `None` for a raw colour.
    pub var: Option<VarId>,
    /// Every place carrying it.
    pub uses: usize,
    /// The distinct layers carrying it, in document order.
    pub nodes: Vec<NodeId>,
}

impl Swatch {
    pub fn is_raw(&self) -> bool {
        self.var.is_none()
    }

    fn key(&self) -> Key {
        match self.var {
            None => Key::Raw(bytes(self.color)),
            Some(var) => Key::Bound(var, bytes(self.color)[3]),
        }
    }
}

/// What a place is matched on: a raw colour by its RGBA bytes, a bound paint by
/// its variable and its binding's alpha byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Raw([u8; 4]),
    Bound(VarId, u8),
}

fn bytes(c: Color) -> [u8; 4] {
    c.to_rgba8().to_u8_array()
}

fn of_bytes([r, g, b, a]: [u8; 4]) -> Color {
    Color::from_rgba8(r, g, b, a)
}

fn stop_color(c: &DynamicColor) -> Color {
    c.to_alpha_color::<Srgb>()
}

/// The roots a review over `scope` walks: the selection, or every page.
/// Never the vault: a vendored main is not the document's to recolour.
pub fn roots(doc: &Document, selection: Option<&[NodeId]>) -> Vec<NodeId> {
    match selection {
        Some(ids) => ids.to_vec(),
        None => doc.pages().iter().map(|p| p.id).collect(),
    }
}

/// Every distinct colour inside the subtrees rooted at `roots`, most-used first.
///
/// Hidden paints count, as in Group Colors: a merge that skipped them would
/// bring the old colour back when one is switched on.
pub fn census(doc: &Document, roots: &[NodeId]) -> Vec<Swatch> {
    let mut out: Vec<(Key, Swatch)> = Vec::new();
    for id in subtree_nodes(doc, roots) {
        each_place(doc, id, |key| {
            let swatch = match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, s)) => s,
                None => {
                    let Some(color) = color_of(doc, key) else {
                        return;
                    };
                    out.push((
                        key,
                        Swatch {
                            color,
                            var: match key {
                                Key::Raw(_) => None,
                                Key::Bound(v, _) => Some(v),
                            },
                            uses: 0,
                            nodes: Vec::new(),
                        },
                    ));
                    &mut out.last_mut().expect("just pushed").1
                }
            };
            swatch.uses += 1;
            if swatch.nodes.last() != Some(&id) {
                swatch.nodes.push(id);
            }
        });
    }
    let mut out: Vec<Swatch> = out.into_iter().map(|(_, s)| s).collect();
    // Stable, so equally-used colours keep document order between frames.
    out.sort_by_key(|s| std::cmp::Reverse(s.uses));
    out
}

/// What a place keyed `key` draws, in the default mode.
fn color_of(doc: &Document, key: Key) -> Option<Color> {
    match key {
        Key::Raw(b) => Some(of_bytes(b)),
        Key::Bound(var, alpha) => match doc.variables().resolve_opt(var, None)? {
            Value::Color(c) => {
                let a = c.components[3] * f32::from(alpha) / 255.0;
                Some(of_bytes(bytes(c.with_alpha(a))))
            }
            _ => None,
        },
    }
}

/// Call `f` with the key of every colour place on `id`.
fn each_place(doc: &Document, id: NodeId, mut f: impl FnMut(Key)) {
    let Some(node) = doc.get(id) else { return };
    let paint = node.paint();
    let brushes = paint
        .fills
        .iter()
        .map(|f| &f.brush)
        .chain(paint.strokes.iter().map(|s| &s.brush));
    for brush in brushes {
        match brush {
            BrushSource::Bound { var, alpha } => {
                f(Key::Bound(
                    *var,
                    (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
                ));
            }
            BrushSource::Literal(Brush::Solid(c)) => f(Key::Raw(bytes(*c))),
            BrushSource::Literal(Brush::Gradient(g)) => {
                for s in g.gradient.stops.iter() {
                    f(Key::Raw(bytes(stop_color(&s.color))));
                }
            }
            BrushSource::Literal(Brush::Image(_)) => {}
        }
    }
    for e in node.effects() {
        if let EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) = &e.kind {
            f(Key::Raw(bytes(s.color)));
        }
    }
    if let NodeKind::Text { style, spans, .. } = node.kind() {
        if let Some(c) = style.color {
            f(Key::Raw(bytes(c)));
        }
        for span in spans.as_slice() {
            if let CharAttr::Color(Some(c)) = span.attr {
                f(Key::Raw(bytes(c)));
            }
        }
    }
}

/// A group of near-identical colours: indices into the census it was built
/// from, the anchor first and the rest nearest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub members: Vec<usize>,
}

impl Group {
    /// The member a merge goes into unless the user picks another: the
    /// most-used bound colour, else the anchor. A position in `members`.
    pub fn default_target(&self, census: &[Swatch]) -> usize {
        self.members
            .iter()
            .position(|&m| !census[m].is_raw())
            .unwrap_or(0)
    }

    /// The members a merge into `target` writes away: the raw ones but the target.
    /// A bound colour is never merged away.
    pub fn merged(&self, census: &[Swatch], target: usize) -> Vec<usize> {
        self.members
            .iter()
            .enumerate()
            .filter(|&(i, &m)| i != target && census[m].is_raw())
            .map(|(_, &m)| m)
            .collect()
    }

    pub fn uses(&self, census: &[Swatch]) -> usize {
        self.members.iter().map(|&m| census[m].uses).sum()
    }
}

/// The groups at `threshold`, most-used first. A group shows only when it has
/// something to merge: two or more members, at least one of them raw.
pub fn groups(census: &[Swatch], threshold: Threshold) -> Vec<Group> {
    let limit = threshold.limit();
    let mut taken = vec![false; census.len()];
    let mut out = Vec::new();
    // `census` is most-used first, so each anchor is the most-used colour left.
    for anchor in 0..census.len() {
        if taken[anchor] {
            continue;
        }
        taken[anchor] = true;
        let a = &census[anchor];
        let mut near: Vec<(usize, f64)> = (anchor + 1..census.len())
            .filter(|&m| !taken[m])
            .filter(|&m| bytes(census[m].color)[3] == bytes(a.color)[3])
            .map(|m| (m, delta(a.color, census[m].color)))
            .filter(|&(_, d)| d <= limit)
            .collect();
        for &(m, _) in &near {
            taken[m] = true;
        }
        near.sort_by(|x, y| x.1.total_cmp(&y.1));
        let members: Vec<usize> = std::iter::once(anchor)
            .chain(near.into_iter().map(|(m, _)| m))
            .collect();
        if members.len() >= 2 && members.iter().any(|&m| census[m].is_raw()) {
            out.push(Group { members });
        }
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.uses(census)));
    out
}

/// The document colour a sampled pixel snaps to: the nearest opaque colour of
/// the census within [`Threshold::Tight`], a bound one winning a tie.
pub fn snap(census: &[Swatch], pixel: Color) -> Option<&Swatch> {
    census
        .iter()
        .filter(|s| bytes(s.color)[3] == 255)
        .map(|s| (s, delta(s.color, pixel)))
        .filter(|&(_, d)| d <= Threshold::Tight.limit())
        .min_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then_with(|| a.0.is_raw().cmp(&b.0.is_raw()))
        })
        .map(|(s, _)| s)
}

/// What a merge wrote, for the status line.
#[derive(Clone, Debug, Default)]
pub struct Merged {
    pub tx: Transaction,
    /// How many places it changed.
    pub uses: usize,
    /// On how many layers.
    pub layers: usize,
    /// Places on locked layers it left as they were.
    pub locked: usize,
    /// Places it wrote the variable's value into because they cannot bind:
    /// gradient stops, shadow colours, text colours.
    pub unbound: usize,
}

/// What a merged place becomes.
#[derive(Clone, Copy)]
enum Into {
    Raw(Color),
    /// Bind fills and strokes, at the alpha that draws theirs over the variable's
    /// own `own`; write `value` where a place cannot bind.
    Var {
        var: VarId,
        value: Color,
        own: Color,
    },
}

/// Merge `group`'s members into its member `target` over the subtrees rooted at
/// `roots`, as one transaction. Into a raw colour, the others take it; into a
/// bound one, their fills and strokes bind to its variable.
pub fn merge(
    doc: &Document,
    roots: &[NodeId],
    census: &[Swatch],
    group: &Group,
    target: usize,
) -> Merged {
    let t = &census[group.members[target]];
    let into = match t.var {
        None => Into::Raw(t.color),
        Some(var) => Into::Var {
            var,
            value: t.color,
            own: match doc.variables().resolve_opt(var, None) {
                Some(Value::Color(c)) => c,
                _ => t.color,
            },
        },
    };
    let from: Vec<Key> = group
        .merged(census, target)
        .into_iter()
        .map(|m| census[m].key())
        .collect();
    rewrite(doc, roots, &from, into)
}

/// The variable [`merge_into_new`] creates, its ids minted by the caller.
pub struct NewVar<'a> {
    pub id: VarId,
    pub name: &'a str,
    /// The mode a document with none gains, since a variable needs a cell per mode.
    pub first_mode: ModeId,
}

/// Merge every raw member of `group` into a new colour variable valued
/// `target`'s colour in every mode: create it and bind their users to it, as
/// one transaction.
pub fn merge_into_new(
    doc: &Document,
    roots: &[NodeId],
    census: &[Swatch],
    group: &Group,
    target: usize,
    new: NewVar<'_>,
) -> Merged {
    let NewVar {
        id: var,
        name,
        first_mode,
    } = new;
    let value = census[group.members[target]].color;
    let t = doc.variables();
    let (mut ops, modes) = crate::variable::columns_for_new(t, t.first_collection().id, first_mode);
    ops.push(Operation::AddVariable {
        var: Variable {
            id: var,
            name: name.to_owned(),
            kind: VarKind::Color,
            cells: modes.into_iter().map(|m| (m, Cell::Color(value))).collect(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    });
    let from: Vec<Key> = group
        .members
        .iter()
        .filter(|&&m| census[m].is_raw())
        .map(|&m| census[m].key())
        .collect();
    let mut merged = rewrite(
        doc,
        roots,
        &from,
        Into::Var {
            var,
            value,
            own: value,
        },
    );
    // Every use on a locked layer: nothing to bind, so no variable either.
    if merged.uses == 0 {
        return merged;
    }
    ops.append(&mut merged.tx.0);
    merged.tx = Transaction(ops);
    merged
}

/// Rewrite every place keyed one of `from` into `into`, leaving locked layers.
fn rewrite(doc: &Document, roots: &[NodeId], from: &[Key], into: Into) -> Merged {
    let mut out = Merged::default();
    let hit = |c: Color| from.contains(&Key::Raw(bytes(c)));
    for id in subtree_nodes(doc, roots) {
        let Some(node) = doc.get(id) else { continue };
        let mut uses = 0;
        let mut unbound = 0;
        let mut ops = Vec::new();
        // A place that cannot bind takes the value; one that can, the binding.
        let mut literal = |c: Color| -> Option<Color> {
            if !hit(c) {
                return None;
            }
            uses += 1;
            Some(match into {
                Into::Raw(t) => t,
                Into::Var { value, .. } => {
                    unbound += 1;
                    value
                }
            })
        };
        // A field the target's scope refuses takes its colour, unbound, and so
        // does every field of a layer where the target draws another colour than
        // its default mode's, which the census matched. A variable the merge
        // makes is not held yet, and binds every field.
        let here = match into {
            Into::Var { var, own, .. } => doc
                .variables()
                .get(var)
                .is_none_or(|_| doc.resolve_var(id, var) == Some(Value::Color(own))),
            Into::Raw(_) => true,
        };
        let binds = |var: VarId, f: crate::variable::Field| {
            here && doc.variables().get(var).is_none_or(|v| v.binds(f))
        };
        let brush = |b: &BrushSource,
                     literal: &mut dyn FnMut(Color) -> Option<Color>,
                     f: crate::variable::Field| match b {
            BrushSource::Literal(Brush::Solid(c)) => {
                let c = *c;
                let swapped = literal(c)?;
                Some(match into {
                    Into::Var { var, own, .. } if binds(var, f) => BrushSource::Bound {
                        var,
                        alpha: bind_alpha(c, own),
                    },
                    Into::Raw(_) | Into::Var { .. } => BrushSource::from(swapped),
                })
            }
            BrushSource::Literal(Brush::Gradient(g)) => {
                let mut g = g.clone();
                let mut any = false;
                for s in g.gradient.stops.iter_mut() {
                    if let Some(c) = literal(stop_color(&s.color)) {
                        s.color = DynamicColor::from_alpha_color(c);
                        any = true;
                    }
                }
                any.then(|| BrushSource::Literal(Brush::Gradient(g)))
            }
            _ => None,
        };
        let paint = node.paint();
        let mut solids_bound = 0;
        let mut fills_changed = false;
        let fills: Vec<_> = paint
            .fills
            .iter()
            .map(
                |f| match brush(&f.brush, &mut literal, crate::variable::Field::Fill) {
                    Some(b) => {
                        fills_changed = true;
                        solids_bound += usize::from(b.is_bound());
                        f.map(|f| Fill {
                            brush: b.clone(),
                            visible: f.visible,
                        })
                    }
                    None => f.clone(),
                },
            )
            .collect();
        let mut strokes_changed = false;
        let strokes: Vec<_> = paint
            .strokes
            .iter()
            .map(
                |s| match brush(&s.brush, &mut literal, crate::variable::Field::Stroke) {
                    Some(b) => {
                        strokes_changed = true;
                        solids_bound += usize::from(b.is_bound());
                        s.map(|s| Stroke {
                            brush: b.clone(),
                            ..s.clone()
                        })
                    }
                    None => s.clone(),
                },
            )
            .collect();
        if fills_changed {
            ops.push(Operation::SetFills { id, fills });
        }
        if strokes_changed {
            ops.push(Operation::SetStrokes { id, strokes });
        }
        let mut effects_changed = false;
        let effects: Vec<_> = node
            .effects()
            .iter()
            .map(|e| {
                let mut e = e.clone();
                if let EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) = &mut e.kind
                    && let Some(c) = literal(s.color)
                {
                    s.color = c;
                    effects_changed = true;
                }
                e
            })
            .collect();
        if effects_changed {
            ops.push(Operation::SetEffects { id, effects });
        }
        if let NodeKind::Text { style, spans, .. } = node.kind() {
            let mut style = (**style).clone();
            let mut changed = false;
            if let Some(c) = style.color.and_then(&mut literal) {
                style.color = Some(c);
                changed = true;
            }
            let parts: Vec<_> = spans
                .as_slice()
                .iter()
                .map(|s| match s.attr {
                    CharAttr::Color(Some(c)) => match literal(c) {
                        Some(c) => {
                            changed = true;
                            Span {
                                attr: CharAttr::Color(Some(c)),
                                ..s.clone()
                            }
                        }
                        None => s.clone(),
                    },
                    _ => s.clone(),
                })
                .collect();
            if changed {
                let spans = Spans::from_parts(parts, &style);
                ops.push(Operation::SetTextStyle {
                    id,
                    style,
                    spans: Some(spans),
                });
            }
        }
        if ops.is_empty() {
            continue;
        }
        // A solid fill or stroke bound rather than given the value.
        if let Into::Var { .. } = into {
            unbound -= solids_bound;
        }
        if crate::query::is_effectively_locked(doc, id) {
            out.locked += uses;
            continue;
        }
        out.uses += uses;
        out.unbound += unbound;
        out.layers += 1;
        out.tx.0.append(&mut ops);
    }
    out
}

/// The binding alpha that draws `c`'s alpha over a variable valued `value`.
fn bind_alpha(c: Color, value: Color) -> f32 {
    let v = value.components[3];
    if v <= 0.0 {
        return 1.0;
    }
    (c.components[3] / v).clamp(0.0, 1.0)
}
