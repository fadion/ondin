//! Kits — the provenance of a component vendored from another document, and the
//! **vault** that holds it (§5.3e, §15 D1010, D1011, D1012).
//!
//! Using a component from kit L in document D copies L's published main into D,
//! and the copy lives in D's **vault**: a second root, parentless, of kind
//! [`NodeKind::Root`], beside the document's own ([`Document::vault`]). Every
//! node in it carries an [`Origin`] — which kit, which node of that kit's
//! published state, which version — and no node outside it carries one.
//!
//! ⚠️ **A second root, not a container under the first, and that is the whole of
//! §15 D1012.** D1011 (9) put the vendored mains in a hidden container *in* the
//! tree and named the cost — a closed list of tree readers that must each skip
//! it, none of which fails to compile when it forgets, so a reader that forgets
//! draws, exports or selects a hidden main. Beside the root, every walk that
//! starts at [`Document::root`] — the render, the resolve, the exports, the
//! layers panel, the dashboard cover, zoom-to-fit — skips the vault without being
//! told, and a reader that forgets fails *safe*. What D1011 (9) chose it for
//! holds unchanged: the component passes ask by id across the whole node map
//! (`crate::component`), so they see a vendored main as they see any other.
//!
//! The cost moved to a short list. In this crate: the loader's reachability
//! check walks from both roots; [`check`] holds the vault's rules after every
//! transaction and on load; and `Resolved::update` drops a vault node a
//! transaction dirtied rather than resolving it, since nothing resolves the vault
//! in the first place (`Resolved::rebuild` walks from the root). Outside it: the
//! app's `Selection::retain_existing` and `OndinApp::select_main` keep a vault id
//! out of the selection, and `ondin-render`'s `RenderOverrides::absorb` refuses
//! the vault's two operations. One reader takes the whole node map on purpose and
//! so sees the vault: `swap::options`, whose list of mains a kit's belong in.

use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::node::{Node, NodeKind};
use crate::op::{Operation, Transaction};
use rustc_hash::{FxHashMap, FxHashSet};

/// Where a vendored node came from (§5.3e): the kit's `meta.id`, the node in that
/// kit's published state, and the version it was published at.
///
/// **On every node of the vault and on nothing else** ([`check`]). Per node rather
/// than once per main, so any node answers the question by itself — an update
/// matches nodes by `(kit, node)` across a re-publish, and a node of a main the
/// kit vendored from a third kit names that third kit, not the one it arrived
/// through.
///
/// `node` is an id **in the kit's document**, never one in this document: each
/// vendored node is minted its own id here (§5.2), because two documents
/// vendoring one kit would otherwise hold the same ids and collide when one is
/// pasted into the other (§15 D1010).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
    /// The kit's `meta.id` — 32 lowercase hex characters
    /// ([`crate::DocumentMeta::is_wellformed_id`]).
    pub kit: String,
    /// The node this one was copied from, in the kit's published state.
    pub node: NodeId,
    /// The kit's version this copy was published at.
    pub version: u32,
    /// The kit's name when this copy was vendored — **on a vault main's top
    /// node only**, `None` below it (§15 D1014). What the picker calls a kit the
    /// library can no longer find (3I): the id alone names nothing a person
    /// recognises, and the kit is exactly what is missing. A label, so not part
    /// of [`Self::key`].
    pub name: Option<String>,
}

impl Origin {
    /// The identity an update matches on: which kit, which node there. The
    /// version is not part of it — a newer publish of the same node is the same
    /// node, which is the whole of an update.
    pub fn key(&self) -> (&str, NodeId) {
        (&self.kit, self.node)
    }
}

/// Which vault rule a document broke. Carried by `OpError::Vault` and turned into
/// the loader's message, so both doors say the same thing — `LinkRule`'s pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultRule {
    /// The vault is missing, is not of kind `Root`, has a parent, or is the
    /// document's own root.
    Shape,
    /// A node outside the vault carries an origin.
    OutsideVault,
    /// A node inside the vault carries no origin.
    NoOrigin,
    /// A child of the vault that is neither a main nor a component set.
    NotAMain,
    /// An origin whose kit is not a well-formed document id.
    KitId,
    /// The vault was opened twice, or closed while it held something or was not
    /// open.
    State,
}

impl std::fmt::Display for VaultRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Shape => "is not a parentless Root beside the document's root",
            Self::OutsideVault => "carries a kit origin outside the vault",
            Self::NoOrigin => "is in the vault without a kit origin",
            Self::NotAMain => "is in the vault and is neither a main nor a component set",
            Self::KitId => "names a kit whose id is malformed",
            Self::State => "opens a vault that is open, or closes one that is not empty",
        })
    }
}

/// The vault's rules (§15 D1012), over the whole node map — run after the last op
/// of every transaction and on load, beside `crate::component::check`:
///
/// - the vault, when there is one, is a parentless `Root` that is not `root`;
/// - every node under it carries an [`Origin`] whose kit is a well-formed id, and
///   no other node carries one;
/// - each of its children is a main or a component set — what a kit publishes.
///
/// **No rule says a vendored main is read-only**, because nothing here can tell an
/// edit from an update: both are transactions writing its fields. Read-only is
/// the app's — a vault node is never resolved, so it is never drawn, picked or
/// selected, and no edit reaches it except the ones a kit verb builds.
pub fn check(
    nodes: &FxHashMap<NodeId, Node>,
    root: NodeId,
    vault: Option<NodeId>,
) -> Result<(), (NodeId, VaultRule)> {
    let Some(v) = vault else {
        return match nodes.values().find(|n| n.origin.is_some()) {
            Some(n) => Err((n.id, VaultRule::OutsideVault)),
            None => Ok(()),
        };
    };
    let Some(vn) = nodes.get(&v) else {
        return Err((v, VaultRule::Shape));
    };
    if v == root || vn.parent.is_some() || !matches!(vn.kind, NodeKind::Root) {
        return Err((v, VaultRule::Shape));
    }
    if vn.origin.is_some() {
        return Err((v, VaultRule::OutsideVault));
    }
    let mut inside: FxHashSet<NodeId> = FxHashSet::default();
    let mut stack: Vec<NodeId> = vn.children.clone();
    for c in &vn.children {
        let Some(child) = nodes.get(c) else {
            continue;
        };
        if !child.component && child.set.is_none() {
            return Err((*c, VaultRule::NotAMain));
        }
    }
    while let Some(id) = stack.pop() {
        if !inside.insert(id) {
            continue;
        }
        let Some(n) = nodes.get(&id) else {
            continue;
        };
        match &n.origin {
            None => return Err((id, VaultRule::NoOrigin)),
            Some(o) if !crate::meta::DocumentMeta::is_wellformed_id(&o.kit) => {
                return Err((id, VaultRule::KitId));
            }
            Some(_) => {}
        }
        stack.extend(n.children.iter().copied());
    }
    match nodes
        .values()
        .find(|n| n.origin.is_some() && !inside.contains(&n.id))
    {
        Some(n) => Err((n.id, VaultRule::OutsideVault)),
        None => Ok(()),
    }
}

/// A kit's **published state** (§5.3e, §15 D1011 (2)): the frozen copy consumers
/// vendor from, never the working document.
///
/// **It is a [`Document`]**, with the kit's own ids: its root holds every main
/// and component set the kit published, and its vault the kit mains *they* were
/// built from — a main of this kit that nests an instance of another kit's. So
/// the loader, `component::check` and [`check`] hold it to every rule a document
/// is held to, without a second set of rules for a second shape. The kit's ids
/// are kept, not minted, because they are what a consumer's [`Origin::node`]
/// names: the same main keeps its id across publishes, which is how an update
/// finds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Published {
    doc: Document,
    /// For each published main or set, the name of the top-level frame it sat
    /// in when it was published — the picker's grouping, since Ondin has no
    /// pages (§15 D1011 (4)). Absent for one outside any frame.
    groups: Vec<(NodeId, String)>,
    /// The kit's typed name at the publish, which each vendored main records
    /// ([`Origin::name`]) so a consumer can still say which kit it lost (3I).
    name: Option<String>,
}

impl Published {
    /// Assemble a published state read from a file. `doc` has been through the
    /// loader; a group naming a node that is not published is dropped rather than
    /// refused, since it is a label.
    pub(crate) fn from_parts(
        doc: Document,
        groups: Vec<(NodeId, String)>,
        name: Option<String>,
    ) -> Self {
        let tops: FxHashSet<NodeId> = doc
            .get(doc.root())
            .map(|r| r.children.iter().copied().collect())
            .unwrap_or_default();
        let groups = groups
            .into_iter()
            .filter(|(t, _)| tops.contains(t))
            .collect();
        Self { doc, groups, name }
    }

    /// The kit's name at the publish, when it had one.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The frozen document: published mains and sets under its root, their kit
    /// dependencies in its vault.
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// The published mains and component sets, in the kit's document order.
    pub fn tops(&self) -> &[NodeId] {
        self.doc
            .get(self.doc.root())
            .map(|r| r.children.as_slice())
            .unwrap_or(&[])
    }

    /// The top-level frame `top` sat in when it was published (§15 D1011 (4)).
    pub fn group(&self, top: NodeId) -> Option<&str> {
        self.groups
            .iter()
            .find(|(t, _)| *t == top)
            .map(|(_, g)| g.as_str())
    }

    /// Every `(top, frame name)` pair, for the file.
    pub(crate) fn groups(&self) -> &[(NodeId, String)] {
        &self.groups
    }
}

/// The mains and sets a publish freezes, in document order: every set, and every
/// main that is not a set's variant, anywhere in the tree under the root.
fn published_tops(doc: &Document) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        if id != doc.root() && (n.set.is_some() || n.component) {
            out.push(id);
            // A set's variants are published with it; a main holds no main.
            continue;
        }
        stack.extend(n.children.iter().rev().copied());
    }
    out
}

/// The top-level frame a published main or set sits in, when it is not itself
/// one — the root's child above it.
fn top_frame_name(doc: &Document, id: NodeId) -> Option<String> {
    let mut at = id;
    let mut above = doc.get(id)?.parent;
    while let Some(p) = above {
        if p == doc.root() {
            let n = doc.get(at)?;
            return (at != id && matches!(n.kind, NodeKind::Artboard { .. }))
                .then(|| n.name.clone());
        }
        at = p;
        above = doc.get(p)?.parent;
    }
    None
}

/// Every node a set of captured nodes refers to outside itself — links, swaps
/// and bound layers — which a frozen or vendored copy must carry along.
fn refs_out<'a>(nodes: impl IntoIterator<Item = &'a Node>) -> Vec<NodeId> {
    let nodes: Vec<&Node> = nodes.into_iter().collect();
    let inside: FxHashSet<NodeId> = nodes.iter().map(|n| n.id).collect();
    let mut out = Vec::new();
    for n in nodes {
        let bound = n.props.iter().flat_map(|p| p.value.bound.iter().copied());
        for r in n.link.into_iter().chain(n.swap).chain(bound) {
            if !inside.contains(&r) {
                out.push(r);
            }
        }
    }
    out
}

/// The child of a root — the document's or the vault — that `id` sits under.
fn top_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut at = id;
    for _ in 0..=doc.node_map().len() {
        let p = doc.get(at)?.parent?;
        if p == doc.root() || Some(p) == doc.vault() {
            return Some(at);
        }
        at = p;
    }
    None
}

/// Freeze `doc`'s mains and sets as its published state (§5.3e) — what *Publish*
/// stores beside the working document. `None` when the document holds no main.
///
/// Each published main or set is copied whole, ids kept, under the frozen
/// document's root; every main of `doc`'s own vault one of them reaches —
/// through a link, a swap, or a nested instance of a nested instance — is
/// copied whole under the frozen vault with its origin, so a consumer vendors a
/// transitive main from the kit it came from rather than from this one.
pub fn freeze(doc: &Document) -> Option<Published> {
    let tops = published_tops(doc);
    if tops.is_empty() {
        return None;
    }
    let mut root = Document::new(doc.root())
        .node_map()
        .get(&doc.root())
        .cloned()
        .expect("a new document holds its root");
    let mut nodes: FxHashMap<NodeId, Node> = FxHashMap::default();
    let mut groups = Vec::new();
    for t in &tops {
        let mut captured = doc.capture_subtree(*t)?;
        captured[0].parent = Some(doc.root());
        root.children.push(*t);
        if let Some(g) = top_frame_name(doc, *t) {
            groups.push((*t, g));
        }
        nodes.extend(captured.into_iter().map(|n| (n.id, n)));
    }

    // The vault mains the published ones reach, transitively.
    let mut vault_tops: Vec<NodeId> = Vec::new();
    let mut pending = refs_out(nodes.values());
    let mut seen: FxHashSet<NodeId> = FxHashSet::default();
    while let Some(r) = pending.pop() {
        if nodes.contains_key(&r) || !doc.in_vault(r) {
            continue;
        }
        let Some(t) = top_of(doc, r) else { continue };
        if !seen.insert(t) {
            continue;
        }
        let captured = doc.capture_subtree(t)?;
        pending.extend(refs_out(&captured));
        vault_tops.push(t);
        nodes.extend(captured.into_iter().map(|n| (n.id, n)));
    }
    let vault = if vault_tops.is_empty() {
        None
    } else {
        let v = doc.vault()?;
        let mut vn = doc.get(v)?.clone();
        // In the vault's own order, so a re-publish freezes the same bytes.
        vn.children.retain(|c| vault_tops.contains(c));
        nodes.insert(v, vn);
        Some(v)
    };
    nodes.insert(root.id, root);

    let images: FxHashMap<_, _> =
        crate::build::image_ids_in(&nodes.values().cloned().collect::<Vec<_>>())
            .into_iter()
            .filter_map(|id| doc.image(&id).map(|e| (id, e.clone())))
            .collect();
    let frozen = Document::from_parts(
        doc.schema_version(),
        nodes,
        doc.root(),
        vault,
        doc.canvas_background(),
        Vec::new(),
        images,
        Default::default(),
        None,
    );
    debug_assert!(
        crate::component::check(frozen.node_map()).is_ok()
            && check(frozen.node_map(), frozen.root(), frozen.vault()).is_ok(),
        "a frozen kit breaks a rule its working document keeps"
    );
    Some(Published {
        doc: frozen,
        groups,
        name: doc.meta().name.clone(),
    })
}

/// What a publish would change (§5.3e, 2A–2C): the kit's mains and sets against
/// what it last published, by id — added, changed, removed. Never published,
/// every one is added.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Published now and not before, in the kit's document order.
    pub added: Vec<NodeId>,
    /// Published both times, and different.
    pub changed: Vec<NodeId>,
    /// Published before and not now — ids in the last published state, whose
    /// consumers keep them as local copies (D1011 (3)).
    pub removed: Vec<NodeId>,
    /// The kit's name changed since its last publish — what a consumer calls it
    /// once the kit is lost (3I), so worth a publish though no component changed.
    pub renamed: bool,
}

impl Changes {
    /// How many components a publish would carry — the top bar's *Publish N*.
    pub fn count(&self) -> usize {
        self.added.len() + self.changed.len() + self.removed.len()
    }

    /// Whether a publish would carry anything: a component, or the kit's name.
    pub fn any(&self) -> bool {
        self.count() > 0 || self.renamed
    }
}

/// `doc`'s changes since its last publish (§5.3e's change count).
///
/// **Frozen state against frozen state**: the working document is frozen as a
/// publish would freeze it ([`freeze`]) and each published main or set compared,
/// with the kit mains it reaches, to the last publish's. **A main's own place is
/// not compared** — its transform, and its parent in the kit — because a
/// consumer's copy sits in a vault where neither means anything; moving a main
/// across the kit's canvas is not a change anyone downstream receives.
pub fn changes(doc: &Document) -> Changes {
    let now = freeze(doc);
    let before = doc.published();
    let tops_now: Vec<NodeId> = now.as_ref().map(|p| p.tops().to_vec()).unwrap_or_default();
    let tops_before: Vec<NodeId> = before.map(|p| p.tops().to_vec()).unwrap_or_default();
    let mut out = Changes::default();
    for t in &tops_now {
        if !tops_before.contains(t) {
            out.added.push(*t);
        } else if let (Some(n), Some(b)) = (&now, before)
            && (reach(n.document(), *t) != reach(b.document(), *t)
                // Its top-level frame is a consumer's picker grouping (D1011
                // (4)), so moving it into another frame is a change downstream
                // though no node is (`arch-scribe`'s reading of the first cut).
                || n.group(*t) != b.group(*t))
        {
            out.changed.push(*t);
        }
    }
    out.removed = tops_before
        .into_iter()
        .filter(|t| !tops_now.contains(t))
        .collect();
    out.renamed = before.is_some_and(|b| b.name() != doc.meta().name.as_deref());
    out
}

/// Every node `top` shows in a frozen document — its subtree and, transitively,
/// the vault mains it reaches — by id, with **every reached top's own place
/// blanked**, not only `top`'s ([`changes`]).
fn reach(frozen: &Document, top: NodeId) -> std::collections::BTreeMap<NodeId, Node> {
    let mut out = std::collections::BTreeMap::new();
    let mut pending = vec![top];
    while let Some(t) = pending.pop() {
        let Some(captured) = frozen.capture_subtree(t) else {
            continue;
        };
        for r in refs_out(&captured) {
            if let Some(rt) = top_of(frozen, r)
                && !out.contains_key(&rt)
            {
                pending.push(rt);
            }
        }
        // Every top's place, not only `top`'s: a main nesting another reaches it,
        // and moving the nested one across the canvas changes neither for a
        // consumer — the first cut blanked `top` alone and counted Card changed
        // when Icon moved.
        for mut n in captured {
            if n.id == t {
                n.transform = kurbo::Affine::IDENTITY;
                n.parent = None;
            }
            out.insert(n.id, n);
        }
    }
    out
}

/// Publish `doc` as a kit (§5.3e, 2B): freeze it, bump its mark's version, stamp
/// `date` and `note`, and keep the frozen copy beside the working document. The
/// new version, or `None` when the document is not a kit or has nothing to
/// publish.
///
/// **Not undoable** (§15 D1011 (2)), and not an operation: like *Save*, it is an
/// outward act the history cannot take back from documents that already read it.
/// `date` is the caller's clock — a publish is stamped by the user's act, never
/// by a save (`crate::meta`'s rule about clocks).
pub fn publish(doc: &mut Document, note: Option<String>, date: u64) -> Option<u32> {
    let mut meta = doc.meta().clone();
    let mark = meta.kit.as_mut()?;
    // A kit that deleted its last main still has something to publish — that it
    // publishes nothing now, so its consumers keep their copies as local mains
    // (D1011 (3)). Only a kit that never published and holds no main has none.
    let frozen = match freeze(doc) {
        Some(f) => f,
        None if doc.published().is_some() => empty_published(doc),
        None => return None,
    };
    mark.version += 1;
    mark.date = Some(date);
    mark.note = note.filter(|n| !n.trim().is_empty());
    let version = mark.version;
    doc.set_meta(meta);
    doc.set_published(Some(frozen));
    Some(version)
}

/// A published state holding nothing: the kit's root and no main — what a kit
/// that removed its last component publishes.
fn empty_published(doc: &Document) -> Published {
    let mut nodes = FxHashMap::default();
    let root = Document::new(doc.root())
        .node_map()
        .get(&doc.root())
        .cloned()
        .expect("a new document holds its root");
    nodes.insert(root.id, root);
    Published {
        doc: Document::from_parts(
            doc.schema_version(),
            nodes,
            doc.root(),
            None,
            doc.canvas_background(),
            Vec::new(),
            FxHashMap::default(),
            Default::default(),
            None,
        ),
        groups: Vec::new(),
        name: doc.meta().name.clone(),
    }
}

/// Why a kit insert could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KitError {
    /// The kit id is not a well-formed document id.
    KitId,
    /// The node named is not a published main or a variant of a published set.
    NotPublished(NodeId),
    /// The published state refers to a node it does not hold.
    Incomplete(NodeId),
    /// The document holds this node's main at an older version that lacks it —
    /// a variant or a layer a later publish added. Accepting the kit's update
    /// comes first.
    NeedsUpdate(NodeId),
}

/// The identity of a node of `src` — a kit's published document — as a vendored
/// copy records it: its own origin when it sits in the kit's vault (a main the
/// kit vendored from a third), else the kit and its id.
fn source_origin(src: &Document, kit: &str, version: u32, n: &Node) -> Origin {
    match (&n.origin, src.in_vault(n.id)) {
        (Some(o), true) => o.clone(),
        _ => Origin {
            kit: kit.to_string(),
            node: n.id,
            version,
            name: None,
        },
    }
}

/// The ops that vendor `main` — a main or a variant of `kit`'s published state —
/// into `doc`'s vault, with every kit main it reaches (§5.3e), and the id in
/// `doc` that `main`'s copy has. Nothing is vendored twice: a main `doc` already
/// holds, matched by its [`Origin::key`], is reused, whatever version it is at —
/// bringing it up to date is an update's job, not an insert's.
///
/// A variant is vendored with its whole set. Every node is minted a fresh id
/// here (§15 D1010); links, swaps and bound layers that pointed across mains in
/// the kit are pointed at the copies.
pub fn vendor(
    doc: &Document,
    ids: &mut IdSource,
    kit: &str,
    version: u32,
    published: &Published,
    main: NodeId,
) -> Result<(Vec<Operation>, NodeId), KitError> {
    let (src, name) = (published.document(), published.name());
    if !crate::meta::DocumentMeta::is_wellformed_id(kit) {
        return Err(KitError::KitId);
    }
    let is_main = src.get(main).is_some_and(|n| n.component);
    let first = top_of(src, main)
        .filter(|_| is_main)
        .ok_or(KitError::NotPublished(main))?;

    let mut held = held_index(doc);
    let key = |n: &Node| {
        let o = source_origin(src, kit, version, n);
        (o.kit, o.node)
    };

    let mut copies: Vec<Vec<Node>> = Vec::new();
    let mut queue = vec![first];
    let mut queued: FxHashSet<NodeId> = queue.iter().copied().collect();
    while let Some(t) = queue.pop() {
        let top = src.get(t).ok_or(KitError::Incomplete(t))?;
        if find(doc, &held, &key(top)).is_some() {
            continue;
        }
        let template = src.capture_subtree(t).ok_or(KitError::Incomplete(t))?;
        for r in refs_out(&template) {
            let rt = top_of(src, r).ok_or(KitError::Incomplete(r))?;
            if queued.insert(rt) {
                queue.push(rt);
            }
        }
        let (mut copy, _) =
            crate::document::remap_subtree(&template, ids).ok_or(KitError::Incomplete(t))?;
        for (c, s) in copy.iter_mut().zip(&template) {
            c.origin = Some(source_origin(src, kit, version, s));
            held.insert(key(s), c.id);
        }
        // The kit's name on the top node, for when the kit cannot be found — only
        // on a main **of this kit**: a third kit's main keeps the name it arrived
        // with, or none, rather than taking the name of the kit it came through
        // (`D1014`'s fix (c)).
        if let Some(o) = copy.first_mut().and_then(|c| c.origin.as_mut())
            && o.name.is_none()
            && o.kit == kit
        {
            o.name = name.map(str::to_string);
        }
        copies.push(copy);
    }

    // Point what crossed mains in the kit at the copies — `remap_subtree` left
    // every reference outside its own template as the kit's id.
    //
    // A node whose main `doc` holds at an older version, without that node — a
    // variant or a layer a later publish added — has nowhere to land: that is
    // an update owed, named as one rather than as a broken kit.
    let resolve = |r: NodeId| -> Result<NodeId, KitError> {
        let n = src.get(r).ok_or(KitError::Incomplete(r))?;
        if let Some(found) = find(doc, &held, &key(n)) {
            return Ok(found);
        }
        let top = top_of(src, r).and_then(|t| src.get(t));
        match top.and_then(|t| find(doc, &held, &key(t))) {
            Some(_) => Err(KitError::NeedsUpdate(r)),
            None => Err(KitError::Incomplete(r)),
        }
    };
    // Dependencies first, so each insert's links already have somewhere to land
    // when the transaction is read in order — the checks run after the last op
    // either way.
    copies.reverse();
    for copy in &mut copies {
        let inside: FxHashSet<NodeId> = copy.iter().map(|n| n.id).collect();
        for n in copy.iter_mut() {
            if let Some(l) = n.link
                && !inside.contains(&l)
            {
                n.link = Some(resolve(l)?);
            }
            if let Some(s) = n.swap
                && !inside.contains(&s)
            {
                n.swap = Some(resolve(s)?);
            }
            for p in &mut n.props {
                for b in &mut p.value.bound {
                    if !inside.contains(b) {
                        *b = resolve(*b)?;
                    }
                }
            }
        }
    }
    let shown = resolve(main)?;
    let mut images: Vec<(crate::ImageId, crate::ImageEntry)> = Vec::new();
    for copy in &copies {
        for id in crate::build::image_ids_in(copy) {
            if let Some(e) = src.image(&id)
                && !images.iter().any(|(i, _)| *i == id)
            {
                images.push((id, e.clone()));
            }
        }
    }
    let mut out = crate::build::missing_image_ops(doc, &images);
    if copies.is_empty() {
        return Ok((out, shown));
    }
    // Opened only when something goes in — a kit inserting its own main into
    // itself vendors nothing ([`find`]) and must not leave an empty vault.
    let vault = match doc.vault() {
        Some(v) => v,
        None => {
            let v = ids.mint();
            out.push(Operation::OpenVault { id: v });
            v
        }
    };
    let base = doc.get(vault).map_or(0, |v| v.children.len());
    for (i, nodes) in copies.into_iter().enumerate() {
        out.push(Operation::InsertSubtree {
            nodes,
            parent: vault,
            index: base + i,
        });
    }
    Ok((out, shown))
}

/// Every vault node `doc` holds, by its origin's identity.
fn held_index(doc: &Document) -> FxHashMap<(String, NodeId), NodeId> {
    let mut held = FxHashMap::default();
    if let Some(v) = doc.vault() {
        for n in doc.node_map().values() {
            if let Some(o) = &n.origin
                && n.id != v
            {
                held.insert((o.kit.clone(), o.node), n.id);
            }
        }
    }
    held
}

/// The node in `doc` a vendored identity lands on: a held vault copy, or —
/// when `doc` **is** that kit — the live node itself.
///
/// The second arm is what keeps a kit from vendoring itself: a kit instance
/// pasted back into its kit, or its own main inserted from the picker, would
/// otherwise land a frozen copy of the kit's own main in its own vault, and the
/// instance would stop following the main it was made from (`arch-scribe`'s
/// reading of the first build). An origin's `node` is the kit's own id
/// ([`Origin`]), so in the kit it names the live main directly.
fn find(
    doc: &Document,
    held: &FxHashMap<(String, NodeId), NodeId>,
    key: &(String, NodeId),
) -> Option<NodeId> {
    if let Some(h) = held.get(key) {
        return Some(*h);
    }
    let (kit, node) = key;
    (doc.meta().id.as_deref() == Some(kit.as_str()) && doc.contains(*node) && !doc.in_vault(*node))
        .then_some(*node)
}

/// The vault mains a copy of `subtrees` reaches — through a link, a swap or a
/// bound layer, transitively — captured whole with their origins, for the
/// clipboard to **carry** (§15 D1012, D1010's premise (d)).
///
/// A pasted kit instance then lands in another document still an instance:
/// [`adopt`] vendors what it carries there, where §15 D979 (d)'s rule alone —
/// a link whose source the target lacks is dropped — would detach it. Empty for
/// a copy that reaches no vault, which is nearly every copy.
pub fn carried(doc: &Document, subtrees: &[Vec<Node>]) -> Vec<Vec<Node>> {
    let mut out: Vec<Vec<Node>> = Vec::new();
    let mut seen: FxHashSet<NodeId> = FxHashSet::default();
    let mut pending = refs_out(subtrees.iter().flatten());
    while let Some(r) = pending.pop() {
        if !doc.in_vault(r) {
            continue;
        }
        let Some(t) = top_of(doc, r) else { continue };
        if !seen.insert(t) {
            continue;
        }
        let Some(captured) = doc.capture_subtree(t) else {
            continue;
        };
        pending.extend(refs_out(&captured));
        out.push(captured);
    }
    out
}

/// Bring carried vault mains into `doc` (§15 D1012): the ops that vendor each
/// one `doc` does not already hold, and the map from every carried id to the
/// id it has in `doc` — the held copy's, matched by [`Origin::key`], or a fresh
/// one. A paste back into the document it came from maps every id to itself and
/// emits nothing.
///
/// A carried node with no origin, or whose top `doc` holds without it (another
/// version, which grew or lost a layer), is left out of the map, and whatever
/// linked to it lands detached by the ordinary rule.
pub fn adopt(
    doc: &Document,
    ids: &mut IdSource,
    kits: &[Vec<Node>],
) -> (Vec<Operation>, FxHashMap<NodeId, NodeId>) {
    let held = held_index(doc);
    let key = |n: &Node| n.origin.as_ref().map(|o| (o.kit.clone(), o.node));
    let landed = |n: &Node| key(n).and_then(|k| find(doc, &held, &k));
    let mut map: FxHashMap<NodeId, NodeId> = FxHashMap::default();
    let mut fresh: Vec<Vec<Node>> = Vec::new();
    for template in kits {
        let Some(top) = template.first() else {
            continue;
        };
        if landed(top).is_some() {
            for n in template {
                if let Some(h) = landed(n) {
                    map.insert(n.id, h);
                }
            }
            continue;
        }
        if template.iter().any(|n| n.origin.is_none()) {
            continue;
        }
        let Some((mut copy, _)) = crate::document::remap_subtree(template, ids) else {
            continue;
        };
        for (c, t) in copy.iter_mut().zip(template) {
            c.origin = t.origin.clone();
            map.insert(t.id, c.id);
        }
        fresh.push(copy);
    }
    for copy in &mut fresh {
        relink(copy, &map);
    }
    let mut ops = Vec::new();
    if fresh.is_empty() {
        return (ops, map);
    }
    let vault = match doc.vault() {
        Some(v) => v,
        None => {
            let v = ids.mint();
            ops.push(Operation::OpenVault { id: v });
            v
        }
    };
    let base = doc.get(vault).map_or(0, |v| v.children.len());
    for (i, nodes) in fresh.into_iter().enumerate() {
        ops.push(Operation::InsertSubtree {
            nodes,
            parent: vault,
            index: base + i,
        });
    }
    (ops, map)
}

/// `build::insert_subtrees` for a paste carrying kit mains (§15 D1012): adopt
/// them ([`adopt`]), point the pasted subtrees at the adopted copies, and insert
/// against the document the adoption leaves — so `component::settle_copy` finds
/// each link's source present and keeps it. One transaction, the vendoring
/// first. With nothing carried this is `insert_subtrees` exactly.
pub fn insert_carrying(
    doc: &Document,
    ids: &mut IdSource,
    kits: &[Vec<Node>],
    placements: &[crate::build::Placement],
    offset: kurbo::Vec2,
) -> (Transaction, Vec<NodeId>) {
    let (mut ops, map) = adopt(doc, ids, kits);
    if ops.is_empty() && map.values().zip(map.keys()).all(|(a, b)| a == b) {
        return crate::build::insert_subtrees(doc, ids, placements, offset);
    }
    let mut scratch = doc.clone();
    if scratch.apply_unchecked(&Transaction(ops.clone())).is_err() {
        return crate::build::insert_subtrees(doc, ids, placements, offset);
    }
    let placements: Vec<crate::build::Placement> = placements
        .iter()
        .map(|p| {
            let mut nodes = p.nodes.clone();
            relink(&mut nodes, &map);
            crate::build::Placement {
                nodes,
                parent: p.parent,
                index: p.index,
            }
        })
        .collect();
    let (tx, created) = crate::build::insert_subtrees(&scratch, ids, &placements, offset);
    ops.extend(tx.0);
    (Transaction(ops), created)
}

/// Point every link, swap and bound layer in `nodes` that `map` knows at its
/// new id — [`adopt`]'s map applied to a pasted subtree, before it is inserted.
pub fn relink(nodes: &mut [Node], map: &FxHashMap<NodeId, NodeId>) {
    let to = |id: &mut NodeId| {
        if let Some(n) = map.get(id) {
            *id = *n;
        }
    };
    for n in nodes {
        if let Some(l) = &mut n.link {
            to(l);
        }
        if let Some(s) = &mut n.swap {
            to(s);
        }
        for p in &mut n.props {
            p.value.bound.iter_mut().for_each(to);
        }
    }
}

/// What [`update`] built: the transaction, and which vendored mains it rewrote
/// and which it made local.
#[derive(Clone, Debug)]
pub struct Update {
    /// One transaction for the whole kit — undone in one step (5B).
    pub tx: Transaction,
    /// The vault mains rewritten to the new publish, by their id here.
    pub updated: Vec<NodeId>,
    /// The vault mains the kit no longer publishes, revealed as ordinary mains
    /// of the document (D1011 (3)). Placing them is the caller's: the vault is
    /// never resolved, so nothing here knows the document's content bounds.
    pub made_local: Vec<NodeId>,
}

/// Bring every main this document vendored from `kit` up to `published`, at
/// `version` (§5.3e, 5B; §15 D1016) — **a diff plus an ordinary commit**.
///
/// Each vendored main is rewritten in place to match its node in the new
/// publish: a field that differs is set, a layer the publish added is inserted, a
/// layer it dropped is deleted, the children are put in its order, and every
/// node's origin moves to `version`. The commit's main-edit passes then carry the
/// change to the instances exactly as a hand edit of a main would — overrides
/// surviving by value comparison, instance-side structure by §5.3d's rules — so
/// nothing about instances is decided here. A main the new publish reaches and
/// this document does not hold yet is vendored first. **A main the kit no longer
/// publishes is made local**: moved out of the vault as an ordinary main, its
/// origins cleared — no instance is promoted (D1011 (3)).
///
/// Built against a scratch document that each op is applied to as it is made,
/// so every op is read against the state the ones before it leave, and an op
/// that would change nothing is dropped.
pub fn update(
    doc: &Document,
    ids: &mut IdSource,
    kit: &str,
    version: u32,
    published: &Published,
) -> Result<Update, KitError> {
    update_all(doc, ids, &[(kit, version, published)])
}

/// [`update`] for several kits as **one** transaction — *Accept all* (5B), one
/// undo step. Built on one scratch, each kit against the state the ones before
/// it leave, so the commit's propagation runs once over the whole of it.
pub fn update_all(
    doc: &Document,
    ids: &mut IdSource,
    kits: &[(&str, u32, &Published)],
) -> Result<Update, KitError> {
    let Some(&(kit0, version0, published0)) = kits.first() else {
        return Ok(Update {
            tx: Transaction(Vec::new()),
            updated: Vec::new(),
            made_local: Vec::new(),
        });
    };
    let mut b = Builder {
        scratch: doc.clone(),
        ops: Vec::new(),
        kit: kit0,
        version: version0,
        published: published0,
    };
    let mut updated = Vec::new();
    let mut made_local = Vec::new();
    for &(kit, version, published) in kits {
        (b.kit, b.version, b.published) = (kit, version, published);
        let src = published.document();
        let tops: Vec<NodeId> = b
            .scratch
            .vault()
            .and_then(|v| b.scratch.get(v))
            .map(|v| v.children.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|t| {
                b.scratch
                    .get(*t)
                    .and_then(|n| n.origin.as_ref())
                    .is_some_and(|o| o.kit == kit)
            })
            .collect();
        for t in tops {
            let Some(x) = b
                .scratch
                .get(t)
                .and_then(|n| n.origin.as_ref())
                .map(|o| o.node)
            else {
                continue;
            };
            let still = src.get(x).is_some() && top_of(src, x) == Some(x) && !src.in_vault(x);
            if !still {
                made_local.push(t);
                continue;
            }
            b.vendor_reached(ids, x)?;
            b.sync(ids, t, x)?;
            updated.push(t);
        }
    }
    for op in make_local(&b.scratch, &made_local).0 {
        b.push(op)?;
    }
    Ok(Update {
        tx: Transaction(b.ops),
        updated,
        made_local,
    })
}

/// ***Make local*** (§5.3e, 4D/4E, 6B; §15 D1011 (3), D1016): reveal each of
/// `mains` — children of the vault — as an ordinary main of the document, last
/// under its root, every origin in it cleared. It no longer follows its kit.
///
/// 🚨 **No instance is promoted**, which is the whole of D1011 (3): promoting one
/// would make its overrides the main's values and re-base every other instance
/// against it. Revealing the copy leaves every instance linked to the same main
/// with every override as it was — and is undone in one step. A kit main this
/// one nests stays vendored; it is still that kit's. Placing the revealed main
/// is the caller's (the vault is never resolved). A node that is not a vault
/// child is skipped.
pub fn make_local(doc: &Document, mains: &[NodeId]) -> Transaction {
    let mut ops = Vec::new();
    let Some(vault) = doc.vault() else {
        return Transaction(ops);
    };
    let root = doc.root();
    let mut end = doc.get(root).map_or(0, |r| r.children.len());
    for m in mains {
        if doc.get(*m).and_then(|n| n.parent) != Some(vault) {
            continue;
        }
        for n in doc.capture_subtree(*m).unwrap_or_default() {
            ops.push(Operation::SetOrigin {
                id: n.id,
                origin: None,
            });
        }
        ops.push(Operation::Reparent {
            id: *m,
            new_parent: root,
            index: end,
        });
        end += 1;
    }
    Transaction(ops)
}

/// [`update`]'s state: the scratch document every op is applied to as it is
/// made, and the ops so far.
struct Builder<'a> {
    scratch: Document,
    ops: Vec<Operation>,
    kit: &'a str,
    version: u32,
    published: &'a Published,
}

impl Builder<'_> {
    /// Apply `op` to the scratch and keep it — unless it would change nothing.
    fn push(&mut self, op: Operation) -> Result<(), KitError> {
        if op.changes_nothing(&self.scratch) {
            return Ok(());
        }
        let at = op.overwrites().unwrap_or_else(|| self.scratch.root());
        self.scratch
            .apply_unchecked(&Transaction(vec![op.clone()]))
            .map_err(|_| KitError::Incomplete(at))?;
        self.ops.push(op);
        Ok(())
    }

    /// The scratch's node for a node of the published state, by identity.
    fn here(&self, x: NodeId) -> Option<NodeId> {
        let src = self.published.document();
        let o = source_origin(src, self.kit, self.version, src.get(x)?);
        find(&self.scratch, &held_index(&self.scratch), &(o.kit, o.node))
    }

    /// Vendor every main published `x` reaches that the scratch does not hold —
    /// a nested instance of a main this document has never used.
    fn vendor_reached(&mut self, ids: &mut IdSource, x: NodeId) -> Result<(), KitError> {
        let src = self.published.document();
        let captured = src.capture_subtree(x).ok_or(KitError::Incomplete(x))?;
        for r in refs_out(&captured) {
            let rt = top_of(src, r).ok_or(KitError::Incomplete(r))?;
            if self.here(rt).is_some() {
                continue;
            }
            // `vendor` takes a main; a set is vendored through any variant of it.
            let main = if src.get(rt).is_some_and(|n| n.component) {
                rt
            } else {
                src.get(rt)
                    .and_then(|n| {
                        n.children
                            .iter()
                            .copied()
                            .find(|c| src.get(*c).is_some_and(|c| c.component))
                    })
                    .ok_or(KitError::Incomplete(rt))?
            };
            let (ops, _) = vendor(
                &self.scratch,
                ids,
                self.kit,
                self.version,
                self.published,
                main,
            )?;
            for op in ops {
                self.push(op)?;
            }
        }
        Ok(())
    }

    /// Rewrite the scratch's node `d` to match the published node `x`, children
    /// first, then its own fields.
    fn sync(&mut self, ids: &mut IdSource, d: NodeId, x: NodeId) -> Result<(), KitError> {
        let src = self.published.document();
        let pn = src.get(x).ok_or(KitError::Incomplete(x))?.clone();
        let dn = self.scratch.get(d).ok_or(KitError::Incomplete(d))?.clone();

        // Children: match by identity, delete the dropped, insert the new, order.
        let key_of = |n: &Node| n.origin.as_ref().map(|o| o.node);
        let mut placed: Vec<NodeId> = Vec::with_capacity(pn.children.len());
        for dc in &dn.children {
            let kept = self
                .scratch
                .get(*dc)
                .and_then(key_of)
                .is_some_and(|k| pn.children.contains(&k));
            if !kept {
                self.push(Operation::DeleteNode { id: *dc })?;
            }
        }
        for (i, pc) in pn.children.iter().enumerate() {
            let held = self
                .scratch
                .get(d)
                .map(|n| n.children.clone())
                .unwrap_or_default()
                .into_iter()
                .find(|c| self.scratch.get(*c).and_then(key_of) == Some(*pc));
            // A layer whose kind changed between publishes cannot be rewritten
            // field by field — no operation turns a rectangle into an ellipse —
            // so it is replaced.
            let same_kind = |c: NodeId| {
                let (Some(a), Some(b)) = (self.scratch.get(c), src.get(*pc)) else {
                    return false;
                };
                std::mem::discriminant(&a.kind) == std::mem::discriminant(&b.kind)
            };
            let at = match held {
                Some(c) if same_kind(c) => {
                    self.sync(ids, c, *pc)?;
                    c
                }
                Some(c) => {
                    self.push(Operation::DeleteNode { id: c })?;
                    self.insert_new(ids, d, i, *pc)?
                }
                None => self.insert_new(ids, d, i, *pc)?,
            };
            placed.push(at);
        }
        for (i, c) in placed.iter().enumerate() {
            self.push(Operation::Reorder { id: *c, index: i })?;
        }

        // Then the node's own fields — after its children, which a component
        // property's bound layers may be among.
        let mut want = pn.clone();
        want.id = d;
        for op in crate::reset::state_ops(&want) {
            self.push(op)?;
        }
        let resolve = |b: &Builder, r: NodeId| b.here(r);
        let link = match pn.link {
            Some(l) => Some(resolve(self, l).ok_or(KitError::Incomplete(l))?),
            None => None,
        };
        let swap = match pn.swap {
            Some(s) => Some(resolve(self, s).ok_or(KitError::Incomplete(s))?),
            None => None,
        };
        let mut props = pn.props.clone();
        for p in &mut props {
            for bnd in &mut p.value.bound {
                *bnd = resolve(self, *bnd).ok_or(KitError::Incomplete(*bnd))?;
            }
        }
        let origin = Some(Origin {
            name: dn.origin.as_ref().and_then(|o| o.name.clone()),
            ..source_origin(src, self.kit, self.version, &pn)
        });
        for op in [
            Operation::SetLocked {
                id: d,
                locked: pn.locked,
            },
            Operation::SetProportionsLocked {
                id: d,
                locked: pn.proportions_locked,
            },
            Operation::SetComponent {
                id: d,
                component: pn.component,
            },
            Operation::SetLink { id: d, link },
            Operation::SetSwap { id: d, swap },
            Operation::SetVariantSet {
                id: d,
                set: pn.set.clone(),
            },
            Operation::SetVariant {
                id: d,
                values: pn.variant.clone(),
            },
            Operation::SetProperties { id: d, props },
            Operation::SetOrigin { id: d, origin },
        ] {
            self.push(op)?;
        }
        Ok(())
    }

    /// Insert published `x`'s subtree as a new child of the scratch's `parent` at
    /// `index`, minted fresh, with origins, its outside references pointed at the
    /// scratch's nodes. Answers the new node's id.
    fn insert_new(
        &mut self,
        ids: &mut IdSource,
        parent: NodeId,
        index: usize,
        x: NodeId,
    ) -> Result<NodeId, KitError> {
        let src = self.published.document();
        let template = src.capture_subtree(x).ok_or(KitError::Incomplete(x))?;
        let (mut copy, root) =
            crate::document::remap_subtree(&template, ids).ok_or(KitError::Incomplete(x))?;
        let inside: FxHashSet<NodeId> = copy.iter().map(|n| n.id).collect();
        for (c, t) in copy.iter_mut().zip(&template) {
            c.origin = Some(source_origin(src, self.kit, self.version, t));
        }
        for c in &mut copy {
            if let Some(l) = c.link
                && !inside.contains(&l)
            {
                c.link = Some(self.here(l).ok_or(KitError::Incomplete(l))?);
            }
            if let Some(s) = c.swap
                && !inside.contains(&s)
            {
                c.swap = Some(self.here(s).ok_or(KitError::Incomplete(s))?);
            }
            for p in &mut c.props {
                for bnd in &mut p.value.bound {
                    if !inside.contains(bnd) {
                        *bnd = self.here(*bnd).ok_or(KitError::Incomplete(*bnd))?;
                    }
                }
            }
        }
        let index = index.min(self.scratch.get(parent).map_or(0, |p| p.children.len()));
        self.push(Operation::InsertSubtree {
            nodes: copy,
            parent,
            index,
        })?;
        Ok(root)
    }
}

/// The variant a set inserts when it is inserted whole (§5.3e's *a set is listed
/// once and inserts its default variant*): **the one placed first in reading
/// order** — topmost, then leftmost, by its position in the set — which is where
/// the eye starts on a grid of variants and the cell Figma treats as the default
/// too. Ties go to the earlier child. `None` for a set with no variant.
///
/// The build's ruling (§15 D1012): §5.3d defines no default, and storing one
/// would be a field and a control for a question reading order already answers.
pub fn default_variant(doc: &Document, set: NodeId) -> Option<NodeId> {
    let s = doc.get(set)?;
    s.children
        .iter()
        .filter_map(|c| doc.get(*c))
        .filter(|n| n.component)
        .min_by(|a, b| {
            let (pa, pb) = (a.transform.translation(), b.transform.translation());
            pa.y.total_cmp(&pb.y).then(pa.x.total_cmp(&pb.x))
        })
        .map(|n| n.id)
}

/// One kit insert as a single transaction (§5.3e): vendor `main` — a published
/// main, a variant, or a set, which inserts its [`default_variant`] — and place
/// an instance of it as the last child of `parent`, at the main's own transform.
/// Returns the transaction and the instance's id.
///
/// Placing it is the caller's: the vault is never resolved, so the main's
/// bounds are not known here, and the app centres the instance once the
/// transaction has been laid out.
pub fn insert(
    doc: &Document,
    ids: &mut IdSource,
    kit: &str,
    version: u32,
    published: &Published,
    main: NodeId,
    parent: NodeId,
) -> Result<(Transaction, NodeId), KitError> {
    let src = published.document();
    let main = match src.get(main) {
        Some(n) if n.set.is_some() => {
            default_variant(src, main).ok_or(KitError::NotPublished(main))?
        }
        Some(_) => main,
        None => return Err(KitError::NotPublished(main)),
    };
    let (mut ops, shown) = vendor(doc, ids, kit, version, published, main)?;

    // The instance is built against the document the vendoring leaves, which
    // holds the main to copy.
    let mut scratch = doc.clone();
    scratch
        .apply_unchecked(&Transaction(ops.clone()))
        .map_err(|_| KitError::Incomplete(main))?;
    let template = scratch
        .capture_subtree(shown)
        .ok_or(KitError::Incomplete(main))?;
    let (tx, created) = crate::build::insert_subtrees_as(
        &scratch,
        ids,
        &[crate::build::Placement {
            nodes: template,
            parent,
            index: None,
        }],
        Default::default(),
        crate::component::MainCopy::Instance,
    );
    let instance = *created.first().ok_or(KitError::Incomplete(main))?;
    ops.extend(tx.0);
    Ok((Transaction(ops), instance))
}
