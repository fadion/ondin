//! Pages: a document's separate canvases (§5.5).
//!
//! A page is a parentless `Root` node listed in [`Document::pages`]. Its layers
//! are that root's subtree; its name is the root's name and its default mode the
//! root's [`Node::mode`]. What is not a node field lives in [`Page`]: the ground
//! and the draft flag.
//!
//! Everything else — components, styles, variables, images, the kit vault —
//! belongs to the document and is shared by every page. Every page has the same
//! origin, so a layer moved between pages keeps its coordinates.
//!
//! [`Document::pages`]: crate::document::Document::pages
//! [`Node::mode`]: crate::node::Node

use crate::document::Document;
use crate::guide::{Guide, GuideId};
use crate::id::{IdSource, NodeId};
use crate::op::{Operation, Transaction};
use peniko::Color;

/// The name a new document's one page carries, and the stem new pages number
/// from (`Page 2`, `Page 3`, …).
pub const PAGE_STEM: &str = "Page";

/// The page-table half of a page: what is not a field of its root node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Page {
    /// The page's root node.
    pub id: NodeId,
    /// The colour behind and around the page's frames, or `None` for no colour
    /// of its own — drawn on the viewer's default (the app's theme;
    /// [`crate::DEFAULT_CANVAS_BACKGROUND`] headless). Opaque when set: nothing
    /// behind the canvas could show through.
    pub ground: Option<Color>,
    /// Left out of everything that gathers from every page — kit publishing,
    /// exporting all pages, presenting on past the current page, the library
    /// thumbnail — and kept by everything aimed at it directly.
    pub draft: bool,
}

/// `Page N` for the lowest `N ≥ 1` no name in `taken` already uses.
pub fn free_name<'a>(taken: impl IntoIterator<Item = &'a str>) -> String {
    let taken: Vec<&str> = taken.into_iter().collect();
    (1..)
        .map(|n| format!("{PAGE_STEM} {n}"))
        .find(|name| !taken.contains(&name.as_str()))
        .expect("an unbounded range finds a free number")
}

/// The names `doc`'s pages carry, in order.
fn names(doc: &Document) -> Vec<String> {
    doc.page_ids()
        .filter_map(|p| doc.get(p).map(|n| n.name().to_owned()))
        .collect()
}

/// *New page*: a page at `index` in the order (clamped to the end), named `name`
/// or else the first free `Page N`, on `ground`. Answers the new page.
pub fn add(
    doc: &Document,
    ids: &mut IdSource,
    index: usize,
    name: Option<String>,
    ground: Option<Color>,
) -> (Transaction, NodeId) {
    let id = ids.mint();
    let name = name.unwrap_or_else(|| free_name(names(doc).iter().map(String::as_str)));
    let tx = Transaction(vec![Operation::AddPage {
        id,
        index: index.min(doc.pages().len()),
        name,
        ground,
        draft: false,
        modes: Vec::new(),
    }]);
    (tx, id)
}

/// *Duplicate page*: a copy of `page` directly below it, named `… copy` (`… copy 2`
/// when that is taken), with its ground, draft flag and modes, its layers copied
/// as *Duplicate* copies them and its guides moved onto the copies. Answers the
/// new page; `None` when `page` is no page.
pub fn duplicate(
    doc: &Document,
    ids: &mut IdSource,
    page: NodeId,
) -> Option<(Transaction, NodeId)> {
    let (index, info, root) = (doc.page_index(page)?, doc.page(page)?, doc.get(page)?);
    let id = ids.mint();
    let taken = names(doc);
    let stem = format!("{} copy", root.name());
    let name = std::iter::once(stem.clone())
        .chain((2..).map(|n| format!("{stem} {n}")))
        .find(|n| !taken.contains(n))
        .expect("an unbounded range finds a free name");
    let mut ops = vec![Operation::AddPage {
        id,
        index: index + 1,
        name,
        ground: info.ground,
        draft: info.draft,
        modes: root.modes().to_vec(),
    }];
    let templates: Vec<Vec<crate::node::Node>> = root
        .children()
        .iter()
        .filter_map(|c| doc.capture_subtree(*c))
        .collect();
    let placements: Vec<crate::build::Placement> = templates
        .iter()
        .enumerate()
        .map(|(i, t)| crate::build::Placement {
            nodes: t.clone(),
            parent: id,
            index: Some(i),
        })
        .collect();
    let (copies, _) = crate::build::insert_subtrees(doc, ids, &placements, Default::default());
    // Old id → new, read off the inserts, which keep each template's order.
    let mut map = rustc_hash::FxHashMap::default();
    map.insert(page, id);
    let inserted = copies.0.iter().filter_map(|op| match op {
        Operation::InsertSubtree { nodes, .. } => Some(nodes),
        _ => None,
    });
    for (template, nodes) in templates.iter().zip(inserted) {
        for (from, to) in template.iter().zip(nodes) {
            map.insert(from.id(), to.id());
        }
    }
    ops.extend(copies.0);
    let guides: Vec<Guide> = doc
        .guides()
        .iter()
        .filter(|g| doc.page_of(g.owner) == Some(page))
        .copied()
        .collect();
    for g in guides {
        if let Some(owner) = map.get(&g.owner) {
            ops.push(Operation::AddGuide {
                guide: Guide {
                    id: GuideId(ids.mint()),
                    owner: *owner,
                    ..g
                },
            });
        }
    }
    Some((Transaction(ops), id))
}

/// *Delete page*: its guides, then its layers as *Delete* deletes them (a main
/// among them detaching its instances on other pages), then the page. `None` when
/// `page` is no page or the document's last.
pub fn delete(doc: &Document, page: NodeId) -> Option<Transaction> {
    doc.page_index(page)?;
    if doc.pages().len() == 1 {
        return None;
    }
    let layers = doc.get(page)?.children().to_vec();
    let mut ops: Vec<Operation> = doc
        .guides()
        .iter()
        .filter(|g| g.owner == page)
        .map(|g| Operation::RemoveGuide { id: g.id })
        .collect();
    ops.extend(crate::build::delete(doc, &layers).0);
    ops.push(Operation::RemovePage { id: page });
    Some(Transaction(ops))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `free_name` fills the lowest gap rather than counting past the highest.
    #[test]
    fn free_name_takes_the_lowest_unused_number() {
        assert_eq!(free_name([]), "Page 1");
        assert_eq!(free_name(["Page 1", "Page 3"]), "Page 2");
        assert_eq!(free_name(["Landing", "Page 1", "Page 2"]), "Page 3");
    }
}
