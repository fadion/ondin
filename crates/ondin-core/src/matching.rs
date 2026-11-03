//! *Select matching layers*: the same layer in each copy of a repeated block.
//!
//! A layer's place is its chain of steps from just below its top-level layer (a
//! page frame or a loose layer) down to itself. Each step is the node's kind and
//! its name with a copy's number dropped ("Card 2" is "Card"), since a duplicate
//! increments the number (`naming::copy_name`). Two layers match when their
//! chains agree step for step, and the layers themselves hold the same ordinal
//! among their siblings of that kind and name: a card's two generated texts are
//! both "Text", and the first pairs with each card's first, as a component's
//! layers pair with its main's. The ordinal is not asked of the ancestors, whose
//! copies may sit anywhere; it also keeps a layer's own siblings out.

use crate::document::Document;
use crate::id::NodeId;
use crate::naming::numbered;
use crate::node::NodeKind;
use std::mem::{Discriminant, discriminant};

/// One step of a layer's place: its kind and its name without a copy's number.
type Step<'a> = (Discriminant<NodeKind>, &'a str);

/// Every layer on `page` that matches one of `sources`, in document order, the
/// sources included. A source that is a top-level layer, or not on `page`, has
/// no place and matches nothing.
pub fn matching_layers(doc: &Document, page: NodeId, sources: &[NodeId]) -> Vec<NodeId> {
    let mut places: Vec<(NodeId, Vec<Step<'_>>, usize)> = Vec::new();
    if let Some(root) = doc.get(page) {
        for &top in root.children() {
            walk(doc, top, &mut Vec::new(), &mut places);
        }
    }
    let wanted: Vec<_> = places
        .iter()
        .filter(|(id, ..)| sources.contains(id))
        .map(|(_, place, ordinal)| (place, ordinal))
        .collect();
    places
        .iter()
        .filter(|(_, place, ordinal)| wanted.contains(&(place, ordinal)))
        .map(|(id, ..)| *id)
        .collect()
}

/// Push the place and ordinal of every layer below `parent` onto `out`, `path`
/// being `parent`'s own steps (empty for a top-level layer, which has no place).
fn walk<'a>(
    doc: &'a Document,
    parent: NodeId,
    path: &mut Vec<Step<'a>>,
    out: &mut Vec<(NodeId, Vec<Step<'a>>, usize)>,
) {
    let Some(node) = doc.get(parent) else { return };
    let mut seen: Vec<Step<'a>> = Vec::new();
    for &child in node.children() {
        let Some(c) = doc.get(child) else { continue };
        let name = c.name();
        let step = (
            discriminant(c.kind()),
            numbered(name).map_or(name, |(stem, _)| stem),
        );
        let ordinal = seen.iter().filter(|s| **s == step).count();
        seen.push(step);
        path.push(step);
        out.push((child, path.clone(), ordinal));
        walk(doc, child, path, out);
        path.pop();
    }
}
