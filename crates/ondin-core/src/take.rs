//! What an import takes from a document: a count of what it holds, offered as
//! the import's choices, and the trim that leaves out what was not chosen.
//!
//! Every step goes through [`Document::apply`] with the builders the editor's own
//! verbs use — *Delete page*, *Detach instance*, deleting a style, a variable or a
//! mode — so a trimmed document passes every rule a loaded one does, and what is
//! left out leaves the layers drawn as they were.

use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::image::{ImageId, ImageSource};
use crate::node::NodeKind;
use crate::op::{OpError, Operation, Transaction};
use rustc_hash::FxHashSet;

/// What one page holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageCensus {
    pub id: NodeId,
    pub name: String,
    /// The frames placed straight on the page.
    pub frames: usize,
}

/// What a document holds, counted as an import offers it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Census {
    pub pages: Vec<PageCensus>,
    /// Every page's [`PageCensus::frames`], summed.
    pub frames: usize,
    /// Main components on the pages; a kit's vendored mains are not counted.
    pub components: usize,
    pub styles: usize,
    pub variables: usize,
    pub modes: usize,
    pub pictures: usize,
    /// The embedded pictures' bytes; a linked picture has none in the document.
    pub picture_bytes: u64,
}

/// Count what `doc` holds.
pub fn census(doc: &Document) -> Census {
    let nodes = doc.node_map();
    let pages: Vec<PageCensus> = doc
        .pages()
        .iter()
        .filter_map(|p| nodes.get(&p.id))
        .map(|root| PageCensus {
            id: root.id,
            name: root.name.clone(),
            frames: root
                .children
                .iter()
                .filter(|c| {
                    nodes
                        .get(c)
                        .is_some_and(|n| matches!(n.kind, NodeKind::Artboard { .. }))
                })
                .count(),
        })
        .collect();
    // A picture nothing shows is dropped by every copy, so it is not offered.
    let shown = images_shown(doc);
    let pictures: Vec<_> = doc.images().filter(|(id, _)| shown.contains(*id)).collect();
    Census {
        frames: pages.iter().map(|p| p.frames).sum(),
        pages,
        components: nodes
            .values()
            .filter(|n| n.component && !doc.in_vault(n.id))
            .count(),
        styles: doc.styles().iter().count(),
        variables: doc.variables().iter().count(),
        modes: doc.variables().modes().count(),
        picture_bytes: pictures
            .iter()
            .map(|(_, e)| match &e.source {
                ImageSource::Embedded(bytes) => bytes.len() as u64,
                ImageSource::Linked(_) => 0,
            })
            .sum(),
        pictures: pictures.len(),
    }
}

/// What to keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keep {
    /// The pages kept, by root id. A page not named goes; at least one must stay.
    pub pages: Vec<NodeId>,
    /// Off: every instance becomes plain layers and every main a plain frame or
    /// group, and a kit's vendored mains go.
    pub components: bool,
    /// Off: every style goes, each use keeping the values it shows.
    pub styles: bool,
    /// Off: every variable and mode goes, each bound value becoming the value it
    /// draws in its own mode.
    pub variables: bool,
    /// Off: the picture table is emptied. Image fills stay, drawn as missing.
    pub pictures: bool,
}

impl Keep {
    /// Everything in `doc`.
    pub fn all(doc: &Document) -> Self {
        Self {
            pages: doc.page_ids().collect(),
            components: true,
            styles: true,
            variables: true,
            pictures: true,
        }
    }

    /// Everything in `doc` but its pictures.
    pub fn all_but_pictures(doc: &Document) -> Self {
        Self {
            pictures: false,
            ..Self::all(doc)
        }
    }
}

/// Why a trim was refused.
#[derive(Debug, thiserror::Error)]
pub enum TakeError {
    #[error("at least one page has to be taken")]
    NoPages,
    #[error(transparent)]
    Op(#[from] OpError),
}

/// Trim `doc` to `keep`. The pictures no kept layer or style shows any more are
/// dropped with the pages that showed them. On an error `doc` is as far as the
/// last step that went through.
pub fn take(doc: &mut Document, keep: &Keep) -> Result<(), TakeError> {
    if !doc.page_ids().any(|p| keep.pages.contains(&p)) {
        return Err(TakeError::NoPages);
    }
    // Minting stays clear of the ids the document already holds; the passes below
    // mint only when a style or a main rewrites a copy.
    let mut ids = IdSource::new(0x7a6b);
    crate::document::reserve_existing_ids(doc, &mut ids);
    let gone: Vec<NodeId> = doc.page_ids().filter(|p| !keep.pages.contains(p)).collect();
    for page in gone {
        commit(doc, page_removal(doc, page), &mut ids)?;
    }
    if !keep.components {
        commit(doc, unmade_components(doc), &mut ids)?;
    }
    if !keep.variables {
        let vars: Vec<_> = doc.variables().iter().map(|v| v.id).collect();
        commit(doc, crate::variable::remove_ops(doc, &vars), &mut ids)?;
        let modes: Vec<_> = doc.variables().modes().map(|m| m.id).collect();
        for mode in modes {
            commit(doc, crate::variable::remove_mode_ops(doc, mode), &mut ids)?;
        }
        // Back to the one collection a new document has.
        let t = doc.variables();
        let mut ops: Vec<Operation> = t.collections()[1..]
            .iter()
            .map(|c| Operation::RemoveCollection { id: c.id })
            .collect();
        if t.first_collection().name != crate::variable::FIRST_COLLECTION_NAME {
            ops.push(Operation::SetCollectionName {
                id: t.first_collection().id,
                name: crate::variable::FIRST_COLLECTION_NAME.into(),
            });
        }
        commit(doc, ops, &mut ids)?;
        let kits: Vec<String> = doc.variables().kits().keys().cloned().collect();
        let ops = kits
            .into_iter()
            .map(|kit| Operation::SetKitModes { kit, modes: None })
            .collect();
        commit(doc, ops, &mut ids)?;
    }
    if !keep.styles {
        let styles: Vec<_> = doc.styles().iter().map(|s| s.id).collect();
        commit(doc, crate::style::remove_ops(doc, &styles), &mut ids)?;
    }
    let shown = images_shown(doc);
    let ops = doc
        .images()
        .filter(|(id, _)| !keep.pictures || !shown.contains(*id))
        .map(|(id, _)| Operation::RemoveImage { id: id.clone() })
        .collect();
    commit(doc, ops, &mut ids)?;
    Ok(())
}

/// Apply `ops` with what the editor's commit owes them.
fn commit(doc: &mut Document, ops: Vec<Operation>, ids: &mut IdSource) -> Result<(), OpError> {
    if ops.is_empty() {
        return Ok(());
    }
    let mut tx = Transaction(ops);
    crate::propagate::owed(doc, &mut tx, ids);
    doc.apply(&tx).map(|_| ())
}

/// *Delete page*: the page's guides and its layers' guides, its layers — a main
/// among them detaching its instances elsewhere — then the page.
fn page_removal(doc: &Document, page: NodeId) -> Vec<Operation> {
    let layers: Vec<NodeId> = doc
        .get(page)
        .map(|n| n.children().to_vec())
        .unwrap_or_default();
    let mut ops: Vec<Operation> = doc
        .guides()
        .iter()
        .filter(|g| g.owner == page)
        .map(|g| Operation::RemoveGuide { id: g.id })
        .collect();
    ops.extend(crate::build::guides_of(doc, &layers));
    ops.extend(layers.iter().map(|id| Operation::DeleteNode { id: *id }));
    ops.extend(crate::component::relink_for_delete(doc, &layers));
    ops.push(Operation::RemovePage { id: page });
    ops
}

/// Every link, swap, main, set, variant and property cut, so each instance is
/// plain layers drawn as it was; the vault's vendored mains go with it.
///
/// One transaction, because the rules are checked after its last op and a main
/// unmade before its instances are cut would leave them dangling.
fn unmade_components(doc: &Document) -> Vec<Operation> {
    let mut ops = Vec::new();
    let vault = doc.vault();
    let mut ids: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| !doc.in_vault(n.id))
        .map(|n| n.id)
        .collect();
    // Deterministic, so the same document trims to the same file.
    ids.sort();
    for id in ids {
        let Some(n) = doc.get(id) else { continue };
        if n.link.is_some() {
            ops.push(Operation::SetLink { id, link: None });
        }
        if n.swap.is_some() {
            ops.push(Operation::SetSwap { id, swap: None });
        }
        if n.component {
            ops.push(Operation::SetComponent {
                id,
                component: false,
            });
        }
        if n.set.is_some() {
            ops.push(Operation::SetVariantSet { id, set: None });
        }
        if !n.variant.is_empty() {
            ops.push(Operation::SetVariant { id, values: vec![] });
        }
        if !n.props.is_empty() {
            ops.push(Operation::SetProperties { id, props: vec![] });
        }
    }
    if let Some(vault) = vault {
        let held = doc
            .get(vault)
            .map(|n| n.children().to_vec())
            .unwrap_or_default();
        ops.extend(held.into_iter().map(|id| Operation::DeleteNode { id }));
        ops.push(Operation::CloseVault { id: vault });
    }
    ops
}

/// Every picture a layer (the vault's included) or a style shows.
fn images_shown(doc: &Document) -> FxHashSet<ImageId> {
    let nodes: Vec<crate::Node> = doc.node_map().values().cloned().collect();
    let mut out: FxHashSet<ImageId> = crate::build::image_ids_in(&nodes).into_iter().collect();
    for style in doc.styles().iter() {
        let brushes: Vec<&crate::variable::BrushSource> = match &style.value {
            crate::style::StyleValue::Fills(f) => f.iter().map(|f| &f.brush).collect(),
            crate::style::StyleValue::Strokes(s) => s.iter().map(|s| &s.brush).collect(),
            _ => Vec::new(),
        };
        out.extend(brushes.into_iter().filter_map(|b| match b.literal() {
            Some(crate::Brush::Image(b)) => Some(b.image.id.clone()),
            _ => None,
        }));
    }
    out
}
