//! Gradients follow their layer's box.
//!
//! A gradient's points are stored in the layer's own pixels, so a box that changes
//! size would leave them where they were drawn. They are stored against the
//! layer's [`paint_box`], and two doors keep them on the shape: a write that
//! changes it carries its gradients with it ([`keep_gradients`], run where commits
//! and previews enter), and a box that layout or a bound size draws at another
//! size draws its gradients mapped onto it ([`drawn_paint`]). Both compose the map into
//! `GradientBrush::transform`, which is exact for every kind.

use crate::document::Document;
use crate::id::NodeId;
use crate::image::Brush;
use crate::item::Keyed;
use crate::node::{NodeKind, Paint};
use crate::op::{GeometryPatch, Operation, Transaction};
use crate::style::StyleSlot;
use crate::variable::{BrushSource, SizeVars};
use kurbo::{Affine, Size};
use rustc_hash::FxHashMap;
use std::borrow::Cow;

/// `brush` carried by `by`, an affine of the layer's space. Anything but a gradient
/// comes back unchanged.
pub fn carry_brush(brush: &Brush, by: Affine) -> Brush {
    match brush {
        Brush::Gradient(g) => {
            let mut g = g.clone();
            g.transform = by * g.transform;
            Brush::Gradient(g)
        }
        _ => brush.clone(),
    }
}

/// [`carry_brush`] on a brush source; `None` when it is no literal gradient.
fn carry_source(src: &BrushSource, by: Affine) -> Option<BrushSource> {
    match src {
        BrushSource::Literal(b @ Brush::Gradient(_)) => {
            Some(BrushSource::Literal(carry_brush(b, by)))
        }
        _ => None,
    }
}

/// The map a box-sized layer's gradients ride when its box goes from `from` to
/// `to`: a scale about the box's origin, where every box-sized kind sits. `None`
/// for no change, or a box too small to scale from or to — a zero side would
/// write a singular transform no handle can repair.
pub fn box_map(from: Size, to: Size) -> Option<Affine> {
    if from == to
        || [from.width, from.height, to.width, to.height]
            .iter()
            .any(|s| *s <= 0.0)
    {
        return None;
    }
    let by = Affine::scale_non_uniform(to.width / from.width, to.height / from.height);
    by.as_coeffs().iter().all(|c| c.is_finite()).then_some(by)
}

/// The box a layer's gradients are stored against: `kind`'s, with each axis
/// `vars` binds read as its variable's number in the default mode. A bound axis's
/// literal is a placeholder 0, and reading every mode against the default one
/// lets a mode that draws the variable larger stretch the gradient with it.
pub fn paint_box(doc: &Document, vars: SizeVars, kind: &NodeKind) -> Option<Size> {
    kind.box_size().map(|s| bound_axes(doc, vars, s))
}

/// `size` with each axis `vars` binds read as its variable's default-mode number.
fn bound_axes(doc: &Document, vars: SizeVars, size: Size) -> Size {
    let read = |var: Option<crate::variable::VarId>, literal: f64| match var
        .and_then(|v| doc.variables().resolve_opt(v, None))
    {
        Some(crate::variable::Value::Number(x)) => x.max(0.0),
        _ => literal,
    };
    Size::new(read(vars[0], size.width), read(vars[1], size.height))
}

/// `paint` with its gradients mapped from the box `stored` (a [`paint_box`]) onto
/// the one `drawn` describes: what layout, or a bound size reading another mode,
/// does to its paint. Borrowed when the boxes agree, a kind has no box, or
/// nothing is a gradient.
pub fn drawn_paint<'a>(
    paint: Cow<'a, Paint>,
    stored: Option<Size>,
    drawn: &NodeKind,
) -> Cow<'a, Paint> {
    let (Some(from), Some(to)) = (stored, drawn.box_size()) else {
        return paint;
    };
    let Some(by) = box_map(from, to) else {
        return paint;
    };
    let gradient = |s: &BrushSource| matches!(s, BrushSource::Literal(Brush::Gradient(_)));
    if !paint.fills.iter().any(|f| gradient(&f.brush))
        && !paint.strokes.iter().any(|s| gradient(&s.brush))
    {
        return paint;
    }
    let mut out = paint.into_owned();
    for f in out.fills.iter_mut() {
        if let Some(b) = carry_source(&f.brush, by) {
            f.brush = b;
        }
    }
    for s in out.strokes.iter_mut() {
        if let Some(b) = carry_source(&s.brush, by) {
            s.brush = b;
        }
    }
    Cow::Owned(out)
}

/// `id`'s committed paint as it draws: bindings resolved (`Document::resolved_paint`)
/// and gradients mapped onto the box layout gives it ([`drawn_paint`]). The
/// exporters' door, so a file says what the canvas shows.
pub fn node_paint<'a>(
    doc: &'a Document,
    res: &crate::resolve::Resolved,
    id: NodeId,
) -> Option<Cow<'a, Paint>> {
    let node = doc.get(id)?;
    let drawn = res.used_kind(doc, id).unwrap_or(node.kind());
    Some(drawn_paint(
        doc.resolved_paint(id, node.paint()),
        paint_box(doc, node.size_vars(), node.kind()),
        drawn,
    ))
}

/// Carry `id`'s gradients by `by` within `ops`, a transaction being built.
///
/// A fill or stroke list the ops already set is edited in place, but only where a
/// brush is still the one the layer holds: that list was built from the layer's
/// paint (a photographic scale's widths), while a brush the ops changed was
/// written for the result. A list the ops leave alone is set with its gradients
/// carried, if it has any.
pub fn carry_into(doc: &Document, id: NodeId, by: Affine, ops: &mut Vec<Operation>) {
    carry_slots(doc, id, by, ops, [true, true]);
}

/// [`carry_into`] on the fill list, the stroke list, or both, as `slots` says.
fn carry_slots(doc: &Document, id: NodeId, by: Affine, ops: &mut Vec<Operation>, slots: [bool; 2]) {
    let Some(node) = doc.get(id) else { return };
    let paint = node.paint();
    let held = |list: &[Keyed<crate::node::Fill>], item: &Keyed<crate::node::Fill>| {
        list.iter()
            .any(|f| f.id == item.id && f.brush == item.brush)
    };
    let held_stroke = |list: &[Keyed<crate::node::Stroke>], item: &Keyed<crate::node::Stroke>| {
        list.iter()
            .any(|s| s.id == item.id && s.brush == item.brush)
    };
    let (mut fills_set, mut strokes_set) = (false, false);
    for op in ops.iter_mut() {
        match op {
            Operation::SetFills { id: i, fills } if *i == id && slots[0] => {
                fills_set = true;
                for f in fills.iter_mut() {
                    if held(&paint.fills, f)
                        && let Some(b) = carry_source(&f.brush, by)
                    {
                        f.brush = b;
                    }
                }
            }
            Operation::SetStrokes { id: i, strokes } if *i == id && slots[1] => {
                strokes_set = true;
                for s in strokes.iter_mut() {
                    if held_stroke(&paint.strokes, s)
                        && let Some(b) = carry_source(&s.brush, by)
                    {
                        s.brush = b;
                    }
                }
            }
            _ => {}
        }
    }
    if slots[0]
        && !fills_set
        && paint
            .fills
            .iter()
            .any(|f| carry_source(&f.brush, by).is_some())
    {
        let fills = paint
            .fills
            .iter()
            .map(|f| {
                f.map(|v| crate::node::Fill {
                    brush: carry_source(&v.brush, by).unwrap_or_else(|| v.brush.clone()),
                    ..v.clone()
                })
            })
            .collect();
        ops.push(Operation::SetFills { id, fills });
    }
    if slots[1]
        && !strokes_set
        && paint
            .strokes
            .iter()
            .any(|s| carry_source(&s.brush, by).is_some())
    {
        let strokes = paint
            .strokes
            .iter()
            .map(|s| {
                s.map(|v| crate::node::Stroke {
                    brush: carry_source(&v.brush, by).unwrap_or_else(|| v.brush.clone()),
                    ..v.clone()
                })
            })
            .collect();
        ops.push(Operation::SetStrokes { id, strokes });
    }
}

/// `tx` with the gradients of every box-sized layer whose [`paint_box`] it changes
/// carried onto the new box ([`carry_into`]).
///
/// Both boxes read the layer's size bindings, the new one as the transaction
/// leaves them, so binding or detaching an axis carries only by what the box
/// draws. A layer the transaction creates is left alone, and so is a path or a
/// line: a path's box also changes when a point is edited, which is no resize, so
/// the resize tools carry those themselves. A list bound to a style is left to its
/// style. Several size writes to one layer compose.
///
/// Running it again over its own output carries nothing twice: a list it wrote
/// no longer holds the layer's brush, which is what [`carry_into`] edits.
pub fn keep_gradients(doc: &Document, tx: Transaction) -> Transaction {
    let mut sizes: FxHashMap<NodeId, Size> = FxHashMap::default();
    let mut vars: FxHashMap<NodeId, SizeVars> = FxHashMap::default();
    for op in &tx.0 {
        match op {
            Operation::SetGeometry {
                id,
                geometry: GeometryPatch::Size(to),
            } => {
                sizes.insert(*id, *to);
            }
            Operation::SetSizeVars { id, vars: v } => {
                vars.insert(*id, *v);
            }
            _ => {}
        }
    }
    let mut ids: Vec<(NodeId, Affine)> = sizes
        .into_iter()
        .filter_map(|(id, to)| {
            let node = doc.get(id)?;
            let from = paint_box(doc, node.size_vars(), node.kind())?;
            let after = vars.get(&id).copied().unwrap_or(node.size_vars());
            box_map(from, bound_axes(doc, after, to)).map(|by| (id, by))
        })
        .collect();
    if ids.is_empty() {
        return tx;
    }
    ids.sort_by_key(|(id, _)| *id);
    let mut ops = tx.0;
    for (id, by) in ids {
        let styles = doc.get(id).map(|n| n.styles()).unwrap_or_default();
        let slots = [
            styles.get(StyleSlot::Fills).is_none(),
            styles.get(StyleSlot::Strokes).is_none(),
        ];
        carry_slots(doc, id, by, &mut ops, slots);
    }
    Transaction(ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::IdSource;
    use crate::image::GradientBrush;
    use crate::node::Fill;
    use kurbo::Point;
    use peniko::Gradient;

    fn gradient() -> Brush {
        Brush::Gradient(GradientBrush::from(Gradient::new_linear(
            Point::ZERO,
            Point::new(100.0, 50.0),
        )))
    }

    /// A document holding one 100×50 rect filled with `brush`.
    fn rect_with(brush: Brush) -> (Document, NodeId) {
        let mut ids = IdSource::new(0x5EED);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 50.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id,
                fills: crate::keyed_by_position([Fill {
                    brush: brush.into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("seed");
        (doc, id)
    }

    fn fill_of(tx: &Transaction, id: NodeId) -> Option<Brush> {
        tx.0.iter().find_map(|op| match op {
            Operation::SetFills { id: i, fills } if *i == id => fills[0].brush.literal().cloned(),
            _ => None,
        })
    }

    /// A size write carries the gradient by the box's scale.
    ///
    /// flip: `keep_gradients` returning `tx` unchanged → fails at the `SetFills`
    /// lookup.
    #[test]
    fn a_resize_carries_the_gradient_onto_the_new_box() {
        let (doc, id) = rect_with(gradient());
        let tx = keep_gradients(
            &doc,
            Transaction(vec![Operation::SetGeometry {
                id,
                geometry: GeometryPatch::Size(Size::new(200.0, 25.0)),
            }]),
        );
        let Some(Brush::Gradient(g)) = fill_of(&tx, id) else {
            panic!("the fill was carried: {tx:?}")
        };
        assert_eq!(g.transform, Affine::scale_non_uniform(2.0, 0.5));
    }

    /// A box sized to nothing on either side maps nothing: a zero scale would be
    /// a singular transform.
    #[test]
    fn a_zero_box_maps_nothing() {
        let wide = Size::new(100.0, 50.0);
        assert_eq!(box_map(wide, Size::new(0.0, 50.0)), None);
        assert_eq!(box_map(Size::new(100.0, 0.0), wide), None);
        assert!(
            box_map(wide, Size::new(50.0, 50.0)).is_some(),
            "the control"
        );
    }

    /// A solid has nothing to carry, so the transaction stays as it came.
    #[test]
    fn a_solid_layer_gains_no_paint_write() {
        let (doc, id) = rect_with(Brush::Solid(peniko::Color::BLACK));
        let tx = Transaction(vec![Operation::SetGeometry {
            id,
            geometry: GeometryPatch::Size(Size::new(200.0, 25.0)),
        }]);
        assert_eq!(keep_gradients(&doc, tx.clone()), tx);
    }

    /// A fill list the transaction sets is edited in place where it still holds
    /// the layer's brush, and left alone where it holds a new one.
    #[test]
    fn a_fill_list_already_set_is_carried_only_where_unchanged() {
        let (doc, id) = rect_with(gradient());
        let held = doc.get(id).unwrap().paint().fills.clone();
        let tx = keep_gradients(
            &doc,
            Transaction(vec![
                Operation::SetFills {
                    id,
                    fills: held.clone(),
                },
                Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Size(Size::new(200.0, 50.0)),
                },
            ]),
        );
        assert_eq!(tx.0.len(), 2, "no second SetFills");
        let Some(Brush::Gradient(g)) = fill_of(&tx, id) else {
            panic!("gradient")
        };
        assert_eq!(g.transform, Affine::scale_non_uniform(2.0, 1.0));

        let mut fresh = held;
        fresh[0].brush = carry_brush(&gradient(), Affine::translate((5.0, 5.0))).into();
        let tx = keep_gradients(
            &doc,
            Transaction(vec![
                Operation::SetFills {
                    id,
                    fills: fresh.clone(),
                },
                Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Size(Size::new(200.0, 50.0)),
                },
            ]),
        );
        assert_eq!(fill_of(&tx, id), fresh[0].brush.literal().cloned());
    }

    /// Layout drawing a box wider than it is stored draws the gradient across it.
    #[test]
    fn a_stretched_box_draws_its_gradient_stretched() {
        let (doc, id) = rect_with(gradient());
        let node = doc.get(id).unwrap();
        let wide = NodeKind::Rect {
            size: Size::new(300.0, 50.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let drawn = drawn_paint(Cow::Borrowed(node.paint()), node.kind().box_size(), &wide);
        let Some(Brush::Gradient(g)) = drawn.fills[0].brush.literal() else {
            panic!("gradient")
        };
        assert_eq!(g.transform, Affine::scale_non_uniform(3.0, 1.0));
        assert!(matches!(
            drawn_paint(
                Cow::Borrowed(node.paint()),
                node.kind().box_size(),
                node.kind()
            ),
            Cow::Borrowed(_)
        ));
    }
}
