//! *Select matching layers*: the same layer in each copy of a repeated block.
//!
//! A layer's place is its chain of steps from just below its top-level layer (a
//! page frame or a loose layer) down to itself. Each step is the node's kind and
//! name. An ancestor's name is read with a copy's number dropped ("Card 2" is
//! "Card"), since duplicating a block renumbers its root (`naming::copy_name`).
//! The layer's own name is read whole: a copy's inner layers keep their names,
//! and in a design whose layers all carry generated names, "Rectangle 2" and
//! "Rectangle 60" are two different layers. Two layers match when their chains
//! agree step for step and the layers hold the same ordinal among their siblings
//! of that kind and whole name. The ordinal is not asked of the ancestors, whose
//! copies may sit anywhere; it also keeps a layer's own siblings out.
//!
//! Component links match too, whatever the names and order: two layers whose
//! chains of `Node::link` end at the same layer are the same layer of one main,
//! so an instance that renamed or reordered its layers still pairs with the main
//! and every other instance on the page.

use crate::document::Document;
use crate::id::NodeId;
use crate::naming::numbered;
use crate::node::NodeKind;
use std::mem::{Discriminant, discriminant};

/// One step of a layer's place: its kind and a name (see the module doc for which).
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
        .map(|(id, place, ordinal)| (place, ordinal, origin(doc, *id)))
        .collect();
    places
        .iter()
        .filter(|(id, place, ordinal)| {
            wanted.iter().any(|(p, o, _)| *p == place && *o == ordinal) || {
                let at = origin(doc, *id);
                wanted.iter().any(|(.., w)| *w == at)
            }
        })
        .map(|(id, ..)| *id)
        .collect()
}

/// The far end of `id`'s chain of component links: the main layer it was copied
/// from, through any nested instance, or `id` itself when it is not linked.
fn origin(doc: &Document, id: NodeId) -> NodeId {
    let mut at = id;
    // Bounded, since a link cycle would be a corrupt document, not a reason to hang.
    for _ in 0..64 {
        match doc.get(at).and_then(|n| n.link()) {
            Some(next) if doc.get(next).is_some() => at = next,
            _ => break,
        }
    }
    at
}

/// Push the place and ordinal of every layer below `parent` onto `out`, `path`
/// being `parent`'s own steps by stem (empty for a top-level layer, which has no
/// place).
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
        let kind = discriminant(c.kind());
        let whole = (kind, name);
        let ordinal = seen.iter().filter(|s| **s == whole).count();
        seen.push(whole);
        let mut place = path.clone();
        place.push(whole);
        out.push((child, place, ordinal));
        path.push((kind, numbered(name).map_or(name, |(stem, _)| stem)));
        walk(doc, child, path, out);
        path.pop();
    }
}
