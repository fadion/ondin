//! Kits — the provenance of a component vendored from another document, and the
//! **vault** that holds it (§5.3e, §15 D1010, D1011, D1012).
//!
//! Using a component from kit L in document D copies L's published main into D,
//! and the copy lives in D's **vault**: a second root, parentless, of kind
//! [`NodeKind::Root`], beside the document's own ([`Document::vault`]). Every
//! node in it carries an [`Origin`] — which kit, which node of that kit's
//! published state, which version — and no node outside it carries one.
//!
//! ⚠️ **A root of its own, not a container on a page, and that is the whole of
//! §15 D1012.** D1011 (9) put the vendored mains in a hidden container *in* the
//! tree and named the cost — a closed list of tree readers that must each skip
//! it, none of which fails to compile when it forgets, so a reader that forgets
//! draws, exports or selects a hidden main. Beside the pages, every walk that
//! starts at a page ([`Document::pages`]) — the render, the resolve, the exports,
//! the layers panel, the dashboard cover, zoom-to-fit — skips the vault without
//! being told, and a reader that forgets fails *safe*. What D1011 (9) chose it for
//! holds unchanged: the component passes ask by id across the whole node map
//! (`crate::component`), so they see a vendored main as they see any other.
//!
//! The cost moved to a short list. In this crate: the loader's reachability
//! check walks from every root; [`check`] holds the vault's rules after every
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
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    /// A node inside the vault links, swaps or binds to a node outside it (§15
    /// D1036 (1)).
    LinksOut,
}

impl std::fmt::Display for VaultRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Shape => "is not a parentless Root beside the document's pages",
            Self::OutsideVault => "carries a kit origin outside the vault",
            Self::NoOrigin => "is in the vault without a kit origin",
            Self::NotAMain => "is in the vault and is neither a main nor a component set",
            Self::KitId => "names a kit whose id is malformed",
            Self::State => "opens a vault that is open, or closes one that is not empty",
            Self::LinksOut => "is in the vault and refers to a node outside it",
        })
    }
}

/// The vault's rules (§15 D1012), over the whole node map — run after the last op
/// of every transaction and on load, beside `crate::component::check`:
///
/// - the vault, when there is one, is a parentless `Root` (that it is not also a
///   page is the page table's rule, held by the loader and `OpenVault`);
/// - every node under it carries an [`Origin`] whose kit is a well-formed id, and
///   no other node carries one;
/// - each of its children is a main or a component set — what a kit publishes;
/// - **nothing in it links, swaps or binds to a node outside it** (§15 D1036
///   (1)): a frozen copy that pointed at a live node of the document was written
///   by every edit of that node, and reverted by the next update. A reference to
///   a node the document lacks is `crate::component::check`'s to refuse.
///
/// **No rule says a vendored main is read-only**, because nothing here can tell an
/// edit from an update: both are transactions writing its fields. Read-only is
/// the app's — a vault node is never resolved, so it is never drawn, picked or
/// selected, and no edit reaches it except the ones a kit verb builds.
pub fn check(
    nodes: &FxHashMap<NodeId, Node>,
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
    if vn.parent.is_some() || !matches!(vn.kind, NodeKind::Root) {
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
    if let Some(n) = nodes
        .values()
        .find(|n| n.origin.is_some() && !inside.contains(&n.id))
    {
        return Err((n.id, VaultRule::OutsideVault));
    }
    for id in &inside {
        let n = &nodes[id];
        let bound = n.props.iter().flat_map(|p| p.value.bound.iter());
        if n.link
            .iter()
            .chain(&n.swap)
            .chain(bound)
            .any(|r| nodes.contains_key(r) && !inside.contains(r))
        {
            return Err((*id, VaultRule::LinksOut));
        }
    }
    Ok(())
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
            .get(doc.first_page())
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

    /// The frozen document: published mains and sets on its one page, their kit
    /// dependencies in its vault.
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// The published mains and component sets, in the kit's document order.
    pub fn tops(&self) -> &[NodeId] {
        self.doc
            .get(self.doc.first_page())
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
/// main that is not a set's variant, on every page that is not a draft — and
/// the ones on draft pages those reach ([`draft_tops`]).
fn published_tops(doc: &Document) -> Vec<NodeId> {
    let mut tops = tops_on(doc, doc.gathered_pages());
    tops.extend(draft_tops(doc, &tops));
    crate::build::all_in_document_order(doc, &tops)
}

/// The outermost sets and mains under each of `roots`, in document order.
fn tops_on(doc: &Document, roots: impl Iterator<Item = NodeId>) -> Vec<NodeId> {
    let mut out = Vec::new();
    for root in roots {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(n) = doc.get(id) else { continue };
            if id != root && (n.set.is_some() || n.component) {
                out.push(id);
                // A set's variants are published with it; a main holds no main.
                continue;
            }
            stack.extend(n.children.iter().rev().copied());
        }
    }
    out
}

/// The sets and mains on draft pages that a publish carries anyway, because a
/// published one uses them ([`draft_tops`]) — what the publish message names.
pub fn carried_from_drafts(doc: &Document) -> Vec<NodeId> {
    let tops = tops_on(doc, doc.gathered_pages());
    draft_tops(doc, &tops)
}

/// The sets and mains on draft pages that `tops` reach — by a link, a swap or a
/// bound property, transitively — and so are published with them: a draft keeps
/// its own components out of a kit, never a piece the kit's components use.
fn draft_tops(doc: &Document, tops: &[NodeId]) -> Vec<NodeId> {
    let drafts: FxHashSet<NodeId> =
        tops_on(doc, doc.pages().iter().filter(|p| p.draft).map(|p| p.id))
            .into_iter()
            .collect();
    if drafts.is_empty() {
        return Vec::new();
    }
    let reach = |t: NodeId| refs_out(&doc.capture_subtree(t).unwrap_or_default());
    let mut pending: Vec<NodeId> = tops.iter().flat_map(|t| reach(*t)).collect();
    let mut out = Vec::new();
    while let Some(r) = pending.pop() {
        let mut at = Some(r);
        while let Some(a) = at.filter(|a| !drafts.contains(a)) {
            at = doc.get(a).and_then(|n| n.parent);
        }
        let Some(t) = at else { continue };
        if out.contains(&t) {
            continue;
        }
        pending.extend(reach(t));
        out.push(t);
    }
    out
}

/// The top-level frame a published main or set sits in, when it is not itself
/// one — the page's child above it.
fn top_frame_name(doc: &Document, id: NodeId) -> Option<String> {
    let mut at = id;
    let mut above = doc.get(id)?.parent;
    while let Some(p) = above {
        if doc.is_page(p) {
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

/// The child of a root — a page or the vault — that `id` sits under.
fn top_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut at = id;
    for _ in 0..=doc.node_map().len() {
        let p = doc.get(at)?.parent?;
        if doc.is_page(p) || Some(p) == doc.vault() {
            return Some(at);
        }
        at = p;
    }
    None
}

/// Freeze `doc`'s mains and sets as its published state (§5.3e) — what *Publish*
/// stores beside the working document. `None` when the document holds no main.
///
/// Each published main or set is copied whole, ids kept, onto the frozen
/// document's one page; every main of `doc`'s own vault one of them reaches —
/// through a link, a swap, or a nested instance of a nested instance — is
/// copied whole under the frozen vault with its origin, so a consumer vendors a
/// transitive main from the kit it came from rather than from this one.
pub fn freeze(doc: &Document) -> Option<Published> {
    let tops = published_tops(doc);
    if tops.is_empty() {
        return None;
    }
    let (mut root, page) = frozen_page(doc);
    let mut nodes: FxHashMap<NodeId, Node> = FxHashMap::default();
    let mut groups = Vec::new();
    for t in &tops {
        let mut captured = doc.capture_subtree(*t)?;
        captured[0].parent = Some(root.id);
        root.children.push(*t);
        if let Some(g) = top_frame_name(doc, *t) {
            groups.push((*t, g));
        }
        // Bindings kept: the frozen copy carries the table they read (§15 D1064,
        // below), as it carries the images its fills key into — **all but the
        // top's own visibility**, frozen to what it shows (D1064's amendment): an
        // instance root keeps its own visibility, so nothing downstream reads
        // that binding, and keeping it put its variable in the frozen table,
        // where a cell edit counted as a change ([`Changes::tokens`]) with no
        // component changed.
        top_visibility_frozen(doc, &mut captured);
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
        let mut captured = doc.capture_subtree(t)?;
        top_visibility_frozen(doc, &mut captured);
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
    let (variables, styles) = frozen_tables(doc, nodes.values());
    let frozen = Document::from_parts(
        doc.schema_version(),
        nodes,
        vec![page],
        vault,
        Vec::new(),
        images,
        variables,
        styles,
        Default::default(),
        None,
    );
    debug_assert!(
        crate::component::check(frozen.node_map()).is_ok()
            && check(frozen.node_map(), frozen.vault()).is_ok(),
        "a frozen kit breaks a rule its working document keeps"
    );
    Some(Published {
        doc: frozen,
        groups,
        name: doc.meta().name.clone(),
    })
}

/// A frozen top's visibility binding, and every main root's in it, detached to
/// what it shows (§15 D1064's amendment, the edge its first fix left): a
/// published main's or set's own visibility is its place on the kit's page,
/// and a variant's its place in its set — an instance root keeps its own
/// (`propagate::is_placement`), a variant switch or a swap the slot's
/// (§15 D983) — so no instance downstream receives it ([`reach`] blanks it)
/// and the variable it reads must not travel for it.
fn top_visibility_frozen(doc: &Document, captured: &mut [Node]) {
    for (i, n) in captured.iter_mut().enumerate() {
        if (i == 0 || n.component) && n.visible_var.is_some() {
            n.visible = doc.shown(n.id);
            n.visible_var = None;
        }
    }
}

/// The variables and styles a frozen kit carries (§15 D1064): **what its nodes
/// read**, closed over styles and aliases (`crate::tokens::reached`) — the kit's
/// own and the third kits' it vendored, those with their origins and their kits'
/// modes — and the kit's whole mode list, which are the kit's modes downstream.
/// No mapping travels: the per-kit mapping is each consumer's own (D1038 (b)).
///
/// A variable the mains do not read stays home, as a main the kit does not
/// publish does: what a consumer holds of a kit is what its mains need.
fn frozen_tables<'a>(
    doc: &Document,
    nodes: impl IntoIterator<Item = &'a Node>,
) -> (crate::variable::Variables, crate::style::Styles) {
    let nodes: Vec<&Node> = nodes.into_iter().collect();
    let t = doc.variables();
    let (vs, ss) = crate::tokens::reached(t, doc.styles(), nodes.iter().copied());
    if vs.is_empty() && ss.is_empty() && nodes.iter().all(|n| n.mode.is_none()) {
        return Default::default();
    }
    let vars: std::collections::BTreeMap<_, _> = vs
        .iter()
        .filter_map(|v| t.get(*v).map(|var| (*v, var.clone())))
        .collect();
    let mut kits = std::collections::BTreeMap::new();
    let kit_modes = nodes
        .iter()
        .filter_map(|n| n.mode)
        .filter_map(|m| t.kit_mode_of(m).map(|(k, _)| k.to_string()));
    for k in vars
        .values()
        .filter_map(|v| v.kit().map(str::to_string))
        .chain(kit_modes)
    {
        if let Some(km) = t.kits().get(&k) {
            kits.entry(k).or_insert_with(|| crate::variable::KitModes {
                map: Default::default(),
                ..km.clone()
            });
        }
    }
    // The kit's own modes whenever anything of its own is read or set — a
    // variable of its own, or a frame set to one of its modes.
    let own = vars.values().any(|v| v.origin.is_none())
        || nodes
            .iter()
            .any(|n| n.mode.is_some_and(|m| t.mode(m).is_some()));
    let variables = crate::variable::Variables {
        modes: if own { t.modes().to_vec() } else { Vec::new() },
        vars,
        kits,
    };
    let styles = crate::style::Styles::from_list(
        ss.iter()
            .filter_map(|s| doc.styles().get(*s).cloned())
            .collect(),
    );
    (variables, styles)
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
    /// The published mains' order changed — the order a consumer's picker lists
    /// them in, so worth a publish as the name is: §15 D1015 counts what changes
    /// downstream (the v0.5.0 release review's `[X2-L1-04]`).
    pub reordered: bool,
    /// The variables or styles the mains read changed — a value, a name, a mode
    /// (§15 D1064): a change every consumer draws though no node of a main moved.
    pub tokens: bool,
}

impl Changes {
    /// How many components a publish would carry — the top bar's *Publish N*.
    pub fn count(&self) -> usize {
        self.added.len() + self.changed.len() + self.removed.len()
    }

    /// Whether a publish would carry anything: a component, the kit's name, the
    /// order of its components, or the variables and styles they read.
    pub fn any(&self) -> bool {
        self.count() > 0 || self.renamed || self.reordered || self.tokens
    }
}

/// `doc`'s changes since its last publish (§5.3e's change count).
///
/// **Frozen state against frozen state**: the working document is frozen as a
/// publish would freeze it ([`freeze`]) and each published main or set compared,
/// with the kit mains it reaches, to the last publish's. **A main's own place is
/// not compared** — its transform, its parent in the kit, its insets, its layout
/// item and its visibility, every field an instance root keeps as its own — nor
/// an origin's version: a consumer's copy sits in a vault where none of them
/// reaches an instance, so moving, pinning or hiding a main on the kit's canvas
/// is not a change anyone downstream receives ([`reach`]). **The mains' order
/// is**, as [`Changes::reordered`]: it is the order a consumer's picker lists
/// them in.
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
    // The order of the mains published both times.
    let kept = |of: &[NodeId], other: &[NodeId]| -> Vec<NodeId> {
        of.iter().copied().filter(|t| other.contains(t)).collect()
    };
    out.reordered = kept(&tops_now, &tops_before) != kept(&tops_before, &tops_now);
    out.removed = tops_before
        .into_iter()
        .filter(|t| !tops_now.contains(t))
        .collect();
    out.renamed = before.is_some_and(|b| b.name() != doc.meta().name.as_deref());
    // A third kit's origin version alone is no change the mains read, as
    // `reach` holds for nodes: accepting its update rewrites every held copy.
    let tables = |p: Option<&Published>| {
        p.map(|p| {
            let (mut vars, mut styles) = (
                p.document().variables().clone(),
                p.document().styles().clone(),
            );
            for o in vars.vars.values_mut().filter_map(|v| v.origin.as_mut()) {
                o.version = 0;
            }
            for o in styles.styles.values_mut().filter_map(|s| s.origin.as_mut()) {
                o.version = 0;
            }
            (vars, styles)
        })
        .unwrap_or_default()
    };
    out.tokens = tables(now.as_ref()) != tables(before);
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
        //
        // 🚨 **And all of a top's place, not only its transform** (the v0.5.0
        // release review's `[X2-L1-03]`): an instance root's insets, its layout
        // item and its visibility are its own as its transform is
        // (`propagate::is_placement`), so hiding a main on the kit's page, or
        // moving one pinned in a frame, reached no consumer's instance — and was
        // counted, an update for nothing. Nor is an origin's **version** a change
        // anyone draws: accepting a third kit's update moves it on every main
        // that nests it, and counted each as changed.
        for mut n in captured {
            if n.id == t {
                n.transform = kurbo::Affine::IDENTITY;
                n.parent = None;
                n.insets = Default::default();
                n.item = Default::default();
                n.visible = true;
                // And its binding, now that a frozen copy keeps bindings (§15
                // D1064; D1059's amendment, item 6): an instance root keeps
                // its own visibility, bound or not.
                n.visible_var = None;
            } else if n.component {
                // A variant's visibility is its place in its set, as a top's on
                // the page (`top_visibility_frozen`).
                n.visible = true;
                n.visible_var = None;
            }
            if let Some(o) = &mut n.origin {
                o.version = 0;
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

/// The frozen document's one page: an empty root under the id of `doc`'s first
/// page, on its ground.
fn frozen_page(doc: &Document) -> (Node, crate::page::Page) {
    let id = doc.first_page();
    let root = crate::document::root_node(id, format!("{} 1", crate::page::PAGE_STEM));
    let page = crate::page::Page {
        id,
        ground: doc.ground(id),
        draft: false,
    };
    (root, page)
}

/// A published state holding nothing: the kit's page and no main — what a kit
/// that removed its last component publishes.
fn empty_published(doc: &Document) -> Published {
    let mut nodes = FxHashMap::default();
    let (root, page) = frozen_page(doc);
    nodes.insert(root.id, root);
    Published {
        doc: Document::from_parts(
            doc.schema_version(),
            nodes,
            vec![page],
            None,
            Vec::new(),
            FxHashMap::default(),
            Default::default(),
            Default::default(),
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
    /// The kit's components use a main of **this** document — the kit being
    /// built on is built on it (§15 D1036 (2)): an icons kit whose components
    /// kit nests its icons, used back in the icons kit. A frozen copy would link
    /// to the live main, which the vault may not (`VaultRule::LinksOut`), so the
    /// insert, the update or the paste is refused, naming the main here.
    Circular(NodeId),
    /// The document holds this kit's variables at **other modes** than the
    /// publish read (§15 D1064) — a mode the kit added or removed since: the
    /// variables held have no cell in a mode they lack, so accepting the kit's
    /// update comes first, as [`Self::NeedsUpdate`] says for a main. Also when
    /// the publish brings a variable or style under a name a held one of the
    /// kit's already has — the held version is behind it the same way.
    Modes(String),
}

/// The main `id` sits in, or `id` itself when it is one — what a
/// [`KitError::Circular`] names.
fn main_holding(doc: &Document, id: NodeId) -> NodeId {
    let mut at = id;
    for _ in 0..=doc.node_map().len() {
        let Some(n) = doc.get(at) else { break };
        if n.component {
            return at;
        }
        match n.parent {
            Some(p) => at = p,
            None => break,
        }
    }
    id
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
    let mut templates: Vec<Node> = Vec::new();
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
        templates.extend(template);
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
    // 🚨 **A frozen copy never lands on a live node** (§15 D1036 (2), the
    // v0.5.0 release review's `[X1-L1-02]`): `find`'s own-kit arm answers a
    // node of `doc` itself when the kit being vendored from was built on this
    // one — its copy then linked out of the vault, and every structural edit of
    // that main was refused for good, naming nothing. Refused here, naming it.
    let into_vault = |r: NodeId| -> Result<NodeId, KitError> {
        let found = resolve(r)?;
        if doc.contains(found) && !doc.in_vault(found) {
            return Err(KitError::Circular(main_holding(doc, found)));
        }
        Ok(found)
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
                n.link = Some(into_vault(l)?);
            }
            if let Some(s) = n.swap
                && !inside.contains(&s)
            {
                n.swap = Some(into_vault(s)?);
            }
            for p in &mut n.props {
                for b in &mut p.value.bound {
                    if !inside.contains(b) {
                        *b = into_vault(*b)?;
                    }
                }
                // A preferred main is pointed at where the document holds it and
                // never vendored for its sake: unheld, it is passed over.
                for q in &mut p.value.preferred {
                    if !inside.contains(q)
                        && let Some(found) = src.get(*q).and_then(|m| find(doc, &held, &key(m)))
                    {
                        *q = found;
                    }
                }
            }
        }
    }
    let shown = resolve(main)?;
    // The variables and styles the copies read, held here and the copies
    // pointed at them (§15 D1064) — a held one reused, whatever its version.
    let tokens = crate::tokens::plan(
        doc,
        ids,
        &crate::tokens::Source {
            vars: src.variables(),
            styles: src.styles(),
            kit: Some((kit, version, name)),
        },
        &templates,
        None,
    )?;
    for copy in &mut copies {
        copy.iter_mut().for_each(|n| tokens.apply(n));
    }
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
    out.extend(tokens.ops);
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
///
/// 🚨 **Except a node a fresh copy reaches** (§15 D1036 (7), the v0.5.0 release
/// review's `[X1-L1-01]`): a kit main carried fresh that nests one `doc` holds
/// at an older version, without the layer the copy links to, was inserted into
/// the vault with the link dangling — no `settle_copy` runs on a vault insert —
/// and the whole paste was refused as a broken link. That is an update owed,
/// [`KitError::NeedsUpdate`] naming the held main, as the picker answers for the
/// same main. And a fresh copy reaching a live main of `doc` — a kit built on
/// this one — is [`KitError::Circular`], as [`vendor`] answers (D1036 (2)).
pub fn adopt(
    doc: &Document,
    ids: &mut IdSource,
    kits: &[Vec<Node>],
) -> Result<(Vec<Operation>, FxHashMap<NodeId, NodeId>), KitError> {
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
        // 🚨 **A kit never vendors itself, by paste either** (§15 D1023, the kits
        // QA review's finding): a main of this very kit that is no longer here —
        // deleted, then an instance copied from a consumer pasted back — is not
        // `landed`, and copying it in opened a vault in the kit holding a copy of
        // its own main, two frozen nodes later carrying one origin key. Left out,
        // the paste lands detached, as a link to any missing source does.
        if top
            .origin
            .as_ref()
            .is_some_and(|o| doc.meta().id.as_deref() == Some(o.kit.as_str()))
        {
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
    // Where each carried node's top is, for a reference the map cannot point.
    let top_of_carried: FxHashMap<NodeId, &Node> = kits
        .iter()
        .filter_map(|t| t.first().map(|top| (t, top)))
        .flat_map(|(t, top)| t.iter().map(move |n| (n.id, top)))
        .collect();
    // A carried node left out above — no origin, or this kit's own main no
    // longer here (D1023) — is a source nothing will hold: what links to it
    // lands detached, the ordinary rule. In a fresh vault copy that has to be
    // done here, as no `settle_copy` runs on a vault insert (the record's
    // reading of `adb8e1f`: the dangling link refused the paste whole).
    let mut cut: FxHashSet<NodeId> = FxHashSet::default();
    for copy in &fresh {
        for r in refs_out(copy.iter()) {
            match map.get(&r) {
                Some(&to) if doc.contains(to) && !doc.in_vault(to) => {
                    return Err(KitError::Circular(main_holding(doc, to)));
                }
                Some(_) => {}
                None => {
                    if let Some(top) = top_of_carried.get(&r) {
                        match landed(top) {
                            Some(held) => return Err(KitError::NeedsUpdate(held)),
                            None => {
                                cut.insert(r);
                            }
                        }
                    }
                }
            }
        }
    }
    for copy in &mut fresh {
        relink(copy, &map);
        for n in copy.iter_mut() {
            if n.link.is_some_and(|l| cut.contains(&l)) {
                n.link = None;
            }
            if n.swap.is_some_and(|s| cut.contains(&s)) {
                n.swap = None;
            }
            for p in &mut n.props {
                p.value.bound.retain(|b| !cut.contains(b));
            }
        }
    }
    let mut ops = Vec::new();
    if fresh.is_empty() {
        return Ok((ops, map));
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
    Ok((ops, map))
}

/// `build::insert_subtrees` for a paste carrying kit mains (§15 D1012): adopt
/// them ([`adopt`]), point the pasted subtrees at the adopted copies, and insert
/// against the document the adoption leaves — so `component::settle_copy` finds
/// each link's source present and keeps it. One transaction, the vendoring
/// first. With nothing carried this is `insert_subtrees` exactly; what
/// [`adopt`] refuses, it refuses.
pub fn insert_carrying(
    doc: &Document,
    ids: &mut IdSource,
    kits: &[Vec<Node>],
    tokens: &crate::tokens::Tokens,
    placements: &[crate::build::Placement],
    offset: kurbo::Vec2,
) -> Result<(Transaction, Vec<NodeId>), KitError> {
    let (mut ops, map) = adopt(doc, ids, kits)?;
    // And the kits' variables and styles the copy reads (§15 D1064), held here
    // first: the adopted copies and the pasted layers are pointed at them.
    let pasted = placements.iter().flat_map(|p| p.nodes.iter());
    let plan = crate::tokens::plan_carried(doc, ids, tokens, kits.iter().flatten().chain(pasted))?;
    for op in &mut ops {
        if let Operation::InsertSubtree { nodes, .. } = op {
            nodes.iter_mut().for_each(|n| plan.apply(n));
        }
    }
    let identity = map.values().zip(map.keys()).all(|(a, b)| a == b)
        && plan.vars.iter().all(|(a, b)| a == b)
        && plan.styles.iter().all(|(a, b)| a == b)
        && plan.modes.iter().all(|(a, b)| a == b);
    if ops.is_empty() && plan.ops.is_empty() && identity {
        return Ok(crate::build::insert_subtrees(doc, ids, placements, offset));
    }
    let mut ops: Vec<Operation> = plan.ops.iter().cloned().chain(ops).collect();
    let mut scratch = doc.clone();
    if scratch.apply_unchecked(&Transaction(ops.clone())).is_err() {
        return Ok(crate::build::insert_subtrees(doc, ids, placements, offset));
    }
    let placements: Vec<crate::build::Placement> = placements
        .iter()
        .map(|p| {
            let mut nodes = p.nodes.clone();
            relink(&mut nodes, &map);
            nodes.iter_mut().for_each(|n| plan.apply(n));
            crate::build::Placement {
                nodes,
                parent: p.parent,
                index: p.index,
            }
        })
        .collect();
    let (tx, created) = crate::build::insert_subtrees(&scratch, ids, &placements, offset);
    ops.extend(tx.0);
    Ok((Transaction(ops), created))
}

/// **A document for drawing only**: `doc` with each of `nodes` moved to the top
/// of the first page, last, with its own transform — where an insert of it
/// lands, with no ancestor's transform or layout, and a vault main among them
/// resolved like any other (the picker's tiles and drag picture, and code export
/// writing a kit's mains as components). A node already there stays put. The
/// page's own mode is cleared, so the nodes read the default mode, as the vault
/// they come from does.
///
/// 🚨 **Not a document anything may keep**: the move is applied unchecked, and
/// a vault node on a page breaks the vault's rules. That is nothing to a
/// reader that only draws, which is all this is for.
pub fn as_placed(doc: &Document, nodes: &[NodeId]) -> Document {
    let mut scratch = doc.clone();
    let root = doc.first_page();
    let end = doc.get(root).map_or(0, |r| r.children.len());
    let mut ops: Vec<Operation> = nodes
        .iter()
        .filter(|n| doc.get(**n).is_some_and(|x| x.parent != Some(root)))
        .enumerate()
        .map(|(i, n)| Operation::Reparent {
            id: *n,
            new_parent: root,
            index: end + i,
        })
        .collect();
    if doc.get(root).is_some_and(|r| r.mode().is_some()) {
        ops.push(Operation::SetMode {
            id: root,
            mode: None,
        });
    }
    if !ops.is_empty() {
        let _ = scratch.apply_unchecked(&Transaction(ops));
    }
    scratch
}

/// Point every link, swap, bound layer and preferred main in `nodes` that `map`
/// knows at its new id — [`adopt`]'s map applied to a pasted subtree, before it is inserted.
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
            p.value.preferred.iter_mut().for_each(to);
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
    /// Of `updated`, the mains whose sync wrote **more than their version** —
    /// what *"Updated N components"* counts (§15 D1036 (5)): every synced main
    /// moves to the new version, so counting `updated` named untouched mains,
    /// and a version-only publish as N components changed.
    pub changed: Vec<NodeId>,
    /// The vault mains the update deleted — a variant the kit removed from a
    /// set it still publishes (§15 D982 (c), D1036 (4)). Their instances are
    /// detached by the commit, and the caller says so.
    pub removed: Vec<NodeId>,
    /// The kits' variables the update added or rewrote, and those it removed —
    /// their users detached, values kept (§15 D1064, 5A).
    pub vars_changed: Vec<crate::variable::VarId>,
    pub vars_removed: Vec<crate::variable::VarId>,
    /// The same for the kits' styles.
    pub styles_changed: Vec<crate::style::StyleId>,
    pub styles_removed: Vec<crate::style::StyleId>,
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
/// origins cleared — no instance is promoted (D1011 (3)). **A node the kit only
/// moved is moved** — a variant out of its set, a main into a set, a layer into
/// another group — never deleted and copied again (§15 D1022).
///
/// Built against a scratch document that each op is applied to as it is made,
/// so every op is read against the state the ones before it leave, and an op
/// that would change nothing is dropped.
///
/// A main made local lands on `page`, the one the update was accepted from.
pub fn update(
    doc: &Document,
    ids: &mut IdSource,
    page: NodeId,
    kit: &str,
    version: u32,
    published: &Published,
) -> Result<Update, KitError> {
    update_all(doc, ids, page, &[(kit, version, published)])
}

/// [`update`] for several kits as **one** transaction — *Accept all* (5B), one
/// undo step. Built on one scratch, each kit against the state the ones before
/// it leave, so the commit's propagation runs once over the whole of it.
pub fn update_all(
    doc: &Document,
    ids: &mut IdSource,
    page: NodeId,
    kits: &[(&str, u32, &Published)],
) -> Result<Update, KitError> {
    // 🚨 **A kit carrying a third kit's newer modes waits for that kit's
    // update when both are accepted** (§15 D1064, `arch-scribe`'s reading):
    // each kit is planned against what the ones before it leave, so `[A, B]`
    // with A carrying B's new mode was refused as `Modes(B)` where `[B, A]`
    // took. The refused kit's own update is moved first, and the whole built
    // again — the failed attempts spend ids and leave gaps, nothing else.
    let mut order: Vec<(&str, u32, &Published)> = kits.to_vec();
    for _ in 0..kits.len() {
        match update_in_order(doc, ids, page, &order) {
            Err(KitError::Modes(k)) => match order.iter().position(|(id, _, _)| *id == k) {
                Some(at) if at > 0 => {
                    let moved = order.remove(at);
                    order.insert(0, moved);
                }
                _ => return Err(KitError::Modes(k)),
            },
            other => return other,
        }
    }
    update_in_order(doc, ids, page, &order)
}

/// `from`'s entries appended to `to`, each once, in the order first met.
fn push_new<T: PartialEq + Copy>(to: &mut Vec<T>, from: &[T]) {
    for x in from {
        if !to.contains(x) {
            to.push(*x);
        }
    }
}

/// [`update_all`] for `kits` in the order given.
fn update_in_order(
    doc: &Document,
    ids: &mut IdSource,
    page: NodeId,
    kits: &[(&str, u32, &Published)],
) -> Result<Update, KitError> {
    let Some(&(kit0, version0, published0)) = kits.first() else {
        return Ok(Update {
            tx: Transaction(Vec::new()),
            updated: Vec::new(),
            made_local: Vec::new(),
            changed: Vec::new(),
            removed: Vec::new(),
            vars_changed: Vec::new(),
            vars_removed: Vec::new(),
            styles_changed: Vec::new(),
            styles_removed: Vec::new(),
        });
    };
    let mut b = Builder {
        scratch: doc.clone(),
        ops: Vec::new(),
        kit: kit0,
        version: version0,
        published: published0,
        at: 0,
        later: Vec::new(),
        dropped: Vec::new(),
        keep_out: Vec::new(),
        tokens: Default::default(),
    };
    let mut updated = Vec::new();
    let mut made_local = Vec::new();
    let (mut changed, mut removed) = (Vec::new(), Vec::new());
    let (mut vars_changed, mut vars_removed) = (Vec::new(), Vec::new());
    let (mut styles_changed, mut styles_removed) = (Vec::new(), Vec::new());
    for (at, &(kit, version, published)) in kits.iter().enumerate() {
        (b.kit, b.version, b.published, b.at) = (kit, version, published, at);
        let src = published.document();
        // 🚨 **A main the kit removed forks what it reads before the kit's
        // tables are rewritten** (§15 D1066, `arch-scribe`'s reading 3): a
        // variable only that main read is one the new publish no longer
        // carries, so the plan below removes it and detaches its users — the
        // main among them — and `make_local` at the end found only one mode's
        // literals, the outcome D1066 rules out. A top whose kit node the
        // publish no longer holds at all is made local for certain; the moved
        // ones classified below fork there, by `make_local`, as before.
        let gone: Vec<Node> = b
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
                    .is_some_and(|o| o.kit == kit && src.get(o.node).is_none())
            })
            .flat_map(|t| b.scratch.capture_subtree(t).unwrap_or_default())
            .collect();
        if !gone.is_empty() {
            let fork = crate::tokens::fork(&b.scratch, ids, &gone);
            let mut ops = fork.ops.clone();
            for n in &gone {
                ops.extend(fork.rebind_ops(n, n.link.and_then(|l| b.scratch.get(l))));
            }
            for op in ops {
                b.push(op)?;
            }
        }
        // The kit's variables and styles first (§15 D1064): every one this
        // document holds rewritten to the publish, what it no longer has
        // detached and removed, what it newly reads added — so each main's sync
        // below writes bindings that already have somewhere to land.
        let tokens = crate::tokens::plan(
            &b.scratch,
            ids,
            &crate::tokens::Source {
                vars: src.variables(),
                styles: src.styles(),
                kit: Some((kit, version, published.name())),
            },
            src.node_map().values(),
            Some(kit),
        )?;
        for op in tokens.ops.iter().cloned() {
            b.push(op)?;
        }
        // Once each: one kit's plan can add a variable another kit's sync
        // then rewrites, and the summary lists one row per entry.
        push_new(&mut vars_changed, &tokens.changed_vars);
        push_new(&mut vars_removed, &tokens.removed_vars);
        push_new(&mut styles_changed, &tokens.changed_styles);
        push_new(&mut styles_removed, &tokens.removed_styles);
        b.tokens = tokens;
        let mut tops: Vec<NodeId> = b
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
        // 🚨 **A main the kit moved between a set and the top level is moved
        // here, never deleted and vendored again** (§15 D1022, the kits QA
        // review's finding). A variant taken out of its set left its set's sync
        // deleting the copy — every instance of it cut loose, the toast saying
        // overrides were kept — and a lone main combined into a set was made
        // local, though still published, its instances no longer following it.
        //
        // A published top held below the vault's top level — the variant taken
        // out — is lifted to it, and synced as a top.
        if let Some(v) = b.scratch.vault() {
            for &p in published.tops() {
                if let Some(h) = b.here(p)
                    && b.scratch.get(h).and_then(|n| n.parent) != Some(v)
                {
                    let index = b.scratch.get(v).map_or(0, |n| n.children.len());
                    b.push(Operation::Reparent {
                        id: h,
                        new_parent: v,
                        index,
                    })?;
                    tops.push(h);
                }
            }
        }
        // A vault top the kit put inside another top — the main combined into a
        // set — waits for that top's sync, which moves it in ([`Builder::sync`]
        // finds a child anywhere in the vault); a top the document does not hold
        // yet is begun as an empty shell for the sync to fill.
        let mut moving: FxHashSet<NodeId> = FxHashSet::default();
        for t in tops.clone() {
            let Some(x) = b
                .scratch
                .get(t)
                .and_then(|n| n.origin.as_ref())
                .map(|o| o.node)
            else {
                continue;
            };
            if src.get(x).is_none() || src.in_vault(x) {
                continue;
            }
            if let Some(s) = top_of(src, x).filter(|s| *s != x) {
                moving.insert(t);
                if b.here(s).is_none() {
                    let vault = b.scratch.vault().ok_or(KitError::Incomplete(s))?;
                    let end = b.scratch.get(vault).map_or(0, |v| v.children.len());
                    let shell = b.insert_shell(ids, vault, end, s)?;
                    tops.push(shell);
                }
            }
        }
        // Every top classified before any is synced: a main the kit removed is
        // made local **whole** — kept as a copy where used — so no sync may move a
        // layer out of it, which one that ran first did (§15 D1024, the kits QA
        // review's second pass: a layer the kit moved from a removed main into
        // another was taken out of the local copy, and its instances lost it).
        let mut syncing: Vec<(NodeId, NodeId)> = Vec::new();
        for t in tops {
            if moving.contains(&t) {
                continue;
            }
            let Some(x) = b
                .scratch
                .get(t)
                .and_then(|n| n.origin.as_ref())
                .map(|o| o.node)
            else {
                continue;
            };
            let still = src.get(x).is_some() && top_of(src, x) == Some(x) && !src.in_vault(x);
            if still {
                syncing.push((t, x));
            } else {
                made_local.push(t);
                b.keep_out.push(t);
            }
        }
        for (t, x) in syncing {
            let from = b.ops.len();
            b.vendor_reached(ids, x)?;
            b.sync(ids, t, x)?;
            updated.push(t);
            if b.ops[from..]
                .iter()
                .any(|op| !matches!(op, Operation::SetOrigin { .. }))
            {
                changed.push(t);
            }
        }
        // What a sync dropped and no other sync moved away is gone from the
        // publish; what was moved is where its new parent's sync put it.
        for (n, from) in std::mem::take(&mut b.dropped) {
            if b.scratch.get(n).and_then(|n| n.parent) == Some(from) {
                if b.scratch.get(n).is_some_and(|n| n.component) {
                    removed.push(n);
                } else if let Some(t) = top_of(&b.scratch, from)
                    && !changed.contains(&t)
                {
                    // A deleted layer is a change of the main that held it,
                    // though it falls outside every top's sync (the record's
                    // reading of `3b9cfc3`: a main losing its last layer was
                    // counted unchanged, and the toast said nothing changed).
                    changed.push(t);
                }
                b.push(Operation::DeleteNode { id: n })?;
            }
        }
        // A frame of the document's set to a mode the kit removed reads the
        // mapping again (§15 D1064): an instance whose main set it, and kept it
        // as an override. 🚨 **Not a copy that set it as its main did** — the
        // main is rewritten by its sync and the commit's follow carries that;
        // this write would be read as the user's and keep the copy off its main
        // (`arch-scribe`'s reading, the removal's follow filter's shape).
        // The main must still be there for the follow to come from — a variant
        // this update deletes takes its copies off it, and the commit detaches
        // them on the mode this reset is for (`arch-scribe`'s reading of the
        // first cut, which asked the old document) — and must have set the
        // copy's mode before the update.
        let follows = |n: &Node| {
            let before = doc.get(n.id).map(|x| x.mode);
            let main = n
                .link
                .filter(|l| b.scratch.contains(*l))
                .and_then(|l| doc.get(l))
                .map(|s| s.mode);
            !b.scratch.in_vault(n.id) && main.is_some() && main == before
        };
        let mut stale: Vec<NodeId> = b
            .scratch
            .node_map()
            .values()
            .filter(|n| n.mode.is_some_and(|m| !b.scratch.variables().has_mode(m)))
            .filter(|n| !follows(n))
            .map(|n| n.id)
            .collect();
        stale.sort();
        for id in stale {
            b.push(Operation::SetMode { id, mode: None })?;
        }
    }
    // The references a sync could not point yet, now that every main of every
    // kit accepted is synced (§15 D1022).
    for (d, x, at) in std::mem::take(&mut b.later) {
        let (kit, version, published) = kits[at];
        (b.kit, b.version, b.published, b.at) = (kit, version, published, at);
        let n = published.document().get(x).ok_or(KitError::Incomplete(x))?;
        let (link, swap, props) = (n.link, n.swap, n.props.clone());
        let link = link.map(|l| b.resolve(l)).transpose()?;
        let swap = swap.map(|s| b.resolve(s)).transpose()?;
        let mut props = props;
        for p in &mut props {
            for bnd in &mut p.value.bound {
                *bnd = b.resolve(*bnd)?;
            }
        }
        let from = b.ops.len();
        for op in [
            Operation::SetLink { id: d, link },
            Operation::SetSwap { id: d, swap },
            Operation::SetProperties { id: d, props },
        ] {
            b.push(op)?;
        }
        // Pointed after every sync, so outside each top's count above.
        if b.ops.len() > from
            && let Some(t) = top_of(&b.scratch, d)
            && !changed.contains(&t)
        {
            changed.push(t);
        }
    }
    for op in make_local(&b.scratch, ids, page, &made_local).0 {
        b.push(op)?;
    }
    // 🚨 **The pictures the update brings, first in the transaction** (the
    // v0.5.0 release review's `[R3-L5-01]`). Only `vendor` added images, so a
    // fill a sync set to a new picture, or a layer it inserted wearing one, left
    // the document with a reference and no bytes — saved that way, and never
    // repaired, since the next update finds the fill equal. Asked of the whole
    // vault as the scratch leaves it, against the scratch, so what `vendor`
    // already added is not added twice; an id is taken from the first accepted
    // kit's publish that holds it.
    let held = b
        .scratch
        .vault()
        .and_then(|v| b.scratch.capture_subtree(v))
        .unwrap_or_default();
    let mut images: Vec<(crate::ImageId, crate::ImageEntry)> = Vec::new();
    for id in crate::build::image_ids_in(&held) {
        if let Some(e) = kits.iter().find_map(|(_, _, p)| p.document().image(&id)) {
            images.push((id, e.clone()));
        }
    }
    let mut ops = crate::build::missing_image_ops(&b.scratch, &images);
    ops.append(&mut b.ops);
    Ok(Update {
        tx: Transaction(ops),
        updated,
        made_local,
        changed,
        removed,
        vars_changed,
        vars_removed,
        styles_changed,
        styles_removed,
    })
}

/// ***Make local*** (§5.3e, 4D/4E, 6B; §15 D1011 (3), D1016): reveal each of
/// `mains` — children of the vault — as an ordinary main of the document, last
/// on `page`, every origin in it cleared. It no longer follows its kit.
///
/// 🚨 **No instance is promoted**, which is the whole of D1011 (3): promoting one
/// would make its overrides the main's values and re-base every other instance
/// against it. Revealing the copy leaves every instance linked to the same main
/// with every override as it was — and is undone in one step. A kit main this
/// one nests stays vendored; it is still that kit's. Placing the revealed main
/// is the caller's (the vault is never resolved). A node that is not a vault
/// child is skipped.
///
/// 🚨 **And a kit main that nests this one keeps the kit's** (§15 D1036 (1),
/// the v0.5.0 release review's `[R1-L2-02]`): revealing *Icon* while the vault's
/// *Card* still nests it left *Card*'s nested copy linked to the now-local main
/// — a vault node linking out, so an edit of the local *Icon* reached the kit's
/// *Card*, deleting it detached it, and the next update of the kit vendored a
/// fresh *Icon* and silently reverted *Card*'s instances. So in the same
/// transaction every revealed main a remaining vault node reaches is copied back
/// into the vault, origins kept, and those references are pointed at the copy:
/// the kit's components keep the kit's version, and the revealed one is the
/// user's own fork.
///
/// 🚨 **And the variables and styles it reads are forked with it** (§15
/// D1066, the maintainer's ruling): a main that no longer follows its kit
/// would otherwise go on reading the kit's variables — rewritten by every
/// update while another of the kit's mains is held, orphaned once none is.
/// The document gains its own copies (`crate::tokens::fork`), every mode's
/// value kept, and the revealed nodes' own bindings point at them; a nested
/// kit instance's bindings stay the kit's.
pub fn make_local(
    doc: &Document,
    ids: &mut IdSource,
    page: NodeId,
    mains: &[NodeId],
) -> Transaction {
    make_local_report(doc, ids, page, mains).tx
}

/// What [`make_local`] did beyond its transaction — for the app's status line.
#[derive(Debug, Default)]
pub struct MadeLocal {
    pub tx: Transaction,
    /// The kits' variables and styles forked fresh, not reused (§15 D1066).
    pub forked: (usize, usize),
    /// Revealed frames set to a kit's own mode, which the forks do not read.
    pub kit_frames: Vec<NodeId>,
}

/// [`make_local`], with what it forked.
pub fn make_local_report(
    doc: &Document,
    ids: &mut IdSource,
    page: NodeId,
    mains: &[NodeId],
) -> MadeLocal {
    let mut ops = Vec::new();
    let Some(vault) = doc.vault() else {
        return MadeLocal::default();
    };
    let root = page;
    let mains: Vec<NodeId> = mains
        .iter()
        .copied()
        .filter(|m| doc.get(*m).and_then(|n| n.parent) == Some(vault))
        .collect();
    let revealed: FxHashSet<NodeId> = mains
        .iter()
        .flat_map(|m| doc.capture_subtree(*m).unwrap_or_default())
        .map(|n| n.id)
        .collect();
    // The revealed mains the rest of the vault reaches — and, through their
    // copies, the ones those reach — copied back in under fresh ids.
    let staying: Vec<Node> = doc
        .capture_subtree(vault)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n.id != vault && !revealed.contains(&n.id))
        .collect();
    let mut map: FxHashMap<NodeId, NodeId> = FxHashMap::default();
    let mut fresh: Vec<Vec<Node>> = Vec::new();
    let mut pending: Vec<NodeId> = refs_out(&staying)
        .into_iter()
        .filter(|r| revealed.contains(r))
        .collect();
    while let Some(r) = pending.pop() {
        let Some(t) = top_of(doc, r) else { continue };
        if map.contains_key(&t) {
            continue;
        }
        let template = doc.capture_subtree(t).unwrap_or_default();
        let Some((mut copy, _)) = crate::document::remap_subtree(&template, ids) else {
            continue;
        };
        for (c, s) in copy.iter_mut().zip(&template) {
            c.origin = s.origin.clone();
            map.insert(s.id, c.id);
        }
        pending.extend(
            refs_out(&template)
                .into_iter()
                .filter(|r| revealed.contains(r)),
        );
        fresh.push(copy);
    }
    let base = doc.get(vault).map_or(0, |v| v.children.len());
    for (i, mut copy) in fresh.into_iter().enumerate() {
        relink(&mut copy, &map);
        ops.push(Operation::InsertSubtree {
            nodes: copy,
            parent: vault,
            index: base + i,
        });
    }
    for n in &staying {
        let mut moved = n.clone();
        relink(std::slice::from_mut(&mut moved), &map);
        if moved.link != n.link {
            ops.push(Operation::SetLink {
                id: n.id,
                link: moved.link,
            });
        }
        if moved.swap != n.swap {
            ops.push(Operation::SetSwap {
                id: n.id,
                swap: moved.swap,
            });
        }
        if moved.props != n.props {
            ops.push(Operation::SetProperties {
                id: n.id,
                props: moved.props,
            });
        }
    }
    // The forks, and the revealed nodes' own bindings pointed at them.
    let shown: Vec<Node> = mains
        .iter()
        .flat_map(|m| doc.capture_subtree(*m).unwrap_or_default())
        .collect();
    let fork = crate::tokens::fork(doc, ids, &shown);
    ops.extend(fork.ops.iter().cloned());
    for n in &shown {
        ops.extend(fork.rebind_ops(n, n.link.and_then(|l| doc.get(l))));
    }
    let end = doc.get(root).map_or(0, |r| r.children.len());
    for (i, m) in mains.iter().enumerate() {
        for n in doc.capture_subtree(*m).unwrap_or_default() {
            ops.push(Operation::SetOrigin {
                id: n.id,
                origin: None,
            });
        }
        ops.push(Operation::Reparent {
            id: *m,
            new_parent: root,
            index: end + i,
        });
    }
    MadeLocal {
        tx: Transaction(ops),
        forked: fork.made,
        kit_frames: fork.kit_frames,
    }
}

/// [`update`]'s state: the scratch document every op is applied to as it is
/// made, and the ops so far.
struct Builder<'a> {
    scratch: Document,
    ops: Vec<Operation>,
    kit: &'a str,
    version: u32,
    published: &'a Published,
    /// Which of [`update_all`]'s kits `kit`, `version` and `published` are.
    at: usize,
    /// 🚨 **The references a sync could not point yet** (§15 D1022): a node of
    /// the scratch, the published node it was built from, and its kit's index.
    /// A layer added to a main reaches the scratch only when that main is
    /// synced, so a main that nests it — synced first by vault order, which a
    /// paste or a later vendoring can leave either way round, or a variant
    /// nesting a later sibling — has nothing to link its new layer's copy to.
    /// The update failed there as `Incomplete`, on every accept, for good (the
    /// kits QA review's finding, reproduced). Such a link, swap or binding is
    /// written empty and recorded here, and [`update_all`] points it once
    /// every main of every kit accepted is synced. The ops between apply
    /// unchecked, so nothing sees the gap; the commit's checks see the end.
    later: Vec<(NodeId, NodeId, usize)>,
    /// A child a sync found its published parent no longer holding, though the
    /// publish still has it elsewhere, and the parent it was dropped from: deleted
    /// once the kit's tops are synced unless another sync moved it away (§15
    /// D1022). Deleted at once, a move between parents was a delete and a fresh
    /// copy, its instances cut loose.
    dropped: Vec<(NodeId, NodeId)>,
    /// The vault tops being made local: nothing is moved out of them (§15
    /// D1024).
    keep_out: Vec<NodeId>,
    /// Where the current kit's variables, modes and styles landed in the scratch
    /// (§15 D1064) — every published node a sync writes is pointed through it.
    tokens: crate::tokens::Plan,
}

impl Builder<'_> {
    /// [`Self::here`], or why not: a node whose main the scratch holds without
    /// it is an update owed — a third kit's main held at another version —
    /// named as one, as `vendor` names it, rather than as a broken kit.
    fn resolve(&self, r: NodeId) -> Result<NodeId, KitError> {
        if let Some(h) = self.here(r) {
            // A kit built on this document reaching one of its live mains
            // (§15 D1036 (2)) — `vendor`'s refusal, for an update.
            if !self.scratch.in_vault(h) {
                return Err(KitError::Circular(main_holding(&self.scratch, h)));
            }
            return Ok(h);
        }
        let src = self.published.document();
        let held = top_of(src, r).and_then(|t| self.here(t)).is_some();
        Err(if held {
            KitError::NeedsUpdate(r)
        } else {
            KitError::Incomplete(r)
        })
    }

    /// Point `n`'s outside references at the scratch where they can be, and
    /// clear the rest — answering whether any was cleared, to be pointed later
    /// (`later`). `inside` is what needs no pointing: the subtree being
    /// inserted, already remapped.
    ///
    /// Only a vault node is pointed at here: a live node of the document — a
    /// kit built on this one — is deferred, and [`Self::resolve`] refuses it
    /// once every sync is done (§15 D1036 (2)).
    fn point_or_defer(&self, n: &mut Node, inside: &FxHashSet<NodeId>) -> bool {
        let frozen = |x: NodeId| self.here(x).filter(|h| self.scratch.in_vault(*h));
        let mut deferred = false;
        if let Some(l) = n.link
            && !inside.contains(&l)
        {
            n.link = frozen(l);
            deferred |= n.link.is_none();
        }
        if let Some(s) = n.swap
            && !inside.contains(&s)
        {
            n.swap = frozen(s);
            deferred |= n.swap.is_none();
        }
        for p in &mut n.props {
            let before = p.value.bound.len();
            p.value.bound = p
                .value
                .bound
                .iter()
                .filter_map(|b| {
                    if inside.contains(b) {
                        Some(*b)
                    } else {
                        frozen(*b)
                    }
                })
                .collect();
            deferred |= p.value.bound.len() != before;
            // Pointed where held, never deferred: an unheld preferred main is
            // passed over.
            for q in &mut p.value.preferred {
                if !inside.contains(q)
                    && let Some(h) = self.here(*q)
                {
                    *q = h;
                }
            }
        }
        deferred
    }

    /// Whether `a` is `n` or above it in the scratch.
    fn is_ancestor(&self, a: NodeId, n: NodeId) -> bool {
        std::iter::successors(Some(n), |x| self.scratch.get(*x).and_then(|x| x.parent))
            .take(self.scratch.len() + 1)
            .any(|x| x == a)
    }

    /// Published node `s` as an **empty** child of `parent` at `index` — its own
    /// fields, no children — for [`Self::sync`] to fill, moving in the copies the
    /// document already holds of what is under it (§15 D1022, D1024): a set the
    /// kit combined a held lone main into, at the vault's top, or a group made
    /// around held layers. A whole copy would have minted those a second time.
    /// Answers the shell's id.
    fn insert_shell(
        &mut self,
        ids: &mut IdSource,
        parent: NodeId,
        index: usize,
        s: NodeId,
    ) -> Result<NodeId, KitError> {
        let src = self.published.document();
        let mut top = src.get(s).ok_or(KitError::Incomplete(s))?.clone();
        top.children.clear();
        self.tokens.apply(&mut top);
        let template = vec![top];
        let (mut copy, root) =
            crate::document::remap_subtree(&template, ids).ok_or(KitError::Incomplete(s))?;
        copy[0].origin = Some(source_origin(src, self.kit, self.version, &template[0]));
        let inside: FxHashSet<NodeId> = [root].into_iter().collect();
        if self.point_or_defer(&mut copy[0], &inside) {
            self.later.push((root, s, self.at));
        }
        self.push(Operation::InsertSubtree {
            nodes: copy,
            parent,
            index,
        })?;
        Ok(root)
    }

    /// Apply `op` to the scratch and keep it — unless it would change nothing.
    fn push(&mut self, op: Operation) -> Result<(), KitError> {
        if op.changes_nothing(&self.scratch) {
            return Ok(());
        }
        let at = op.overwrites().unwrap_or_else(|| self.scratch.first_page());
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
            let key = self.scratch.get(*dc).and_then(key_of);
            if key.is_some_and(|k| pn.children.contains(&k)) {
                continue;
            }
            // **Every dropped child waits** (`dropped`): one still published
            // under another parent may be moved there by that parent's sync, and
            // one gone may still hold layers that were moved out of it — a group
            // the kit ungrouped, whose layers its parent's sync lifts out (§15
            // D1024, the kits QA review's second pass; deleted here at once, it
            // took them with it, and the instances kept the old ones detached
            // beside fresh copies).
            self.dropped.push((*dc, d));
        }
        for (i, pc) in pn.children.iter().enumerate() {
            let mut held = self
                .scratch
                .get(d)
                .map(|n| n.children.clone())
                .unwrap_or_default()
                .into_iter()
                .find(|c| self.scratch.get(*c).and_then(key_of) == Some(*pc));
            // **Not here but held elsewhere is a move** (§15 D1022): a main the
            // kit combined into this set, a layer it moved into this group. Never
            // one of `d`'s own ancestors, which would make a cycle.
            if held.is_none()
                && let Some(h) = self.here(*pc)
                && h != d
                && !self.is_ancestor(h, d)
                && !self.keep_out.iter().any(|t| self.is_ancestor(*t, h))
            {
                let index = i.min(self.scratch.get(d).map_or(0, |n| n.children.len()));
                self.push(Operation::Reparent {
                    id: h,
                    new_parent: d,
                    index,
                })?;
                held = Some(h);
            }
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
        self.tokens.apply(&mut want);
        // 🚨 **Only the fields this kind carries** (§15 D1024): `state_ops`
        // writes every field, and `changes_nothing` never calls paint on a kind
        // that takes none a no-op, so a group's `SetFills` was applied, refused
        // as `WrongKindForOp`, and every update of a kit main holding a group
        // failed — found by the kits QA review's grouping probe, the group being
        // what that edit makes. What `apply` refuses by kind is skipped.
        for op in crate::reset::state_ops(&want) {
            if op.changes_nothing(&self.scratch) {
                continue;
            }
            match self.scratch.apply_unchecked(&Transaction(vec![op.clone()])) {
                Ok(_) => self.ops.push(op),
                Err(crate::OpError::WrongKindForOp) => {}
                Err(_) => return Err(KitError::Incomplete(d)),
            }
        }
        let mut pointed = pn.clone();
        if self.point_or_defer(&mut pointed, &FxHashSet::default()) {
            self.later.push((d, x, self.at));
        }
        let (link, swap, props) = (pointed.link, pointed.swap, pointed.props);
        // A top carries the kit's name (`Origin::name`): this kit's tops take the
        // name it publishes now — a rename is what `Changes::renamed` lets
        // through, and it must arrive (`arch-scribe`'s reading of D1016's first
        // cut, which kept each copy's first name) — and a third kit's top keeps
        // the one it arrived with. Below the top, none.
        let base = source_origin(src, self.kit, self.version, &pn);
        let is_top = self.scratch.get(d).and_then(|n| n.parent) == self.scratch.vault();
        let name = match (is_top, base.kit == self.kit) {
            (true, true) => self
                .published
                .name()
                .map(str::to_string)
                .or_else(|| dn.origin.as_ref().and_then(|o| o.name.clone())),
            (true, false) => dn.origin.as_ref().and_then(|o| o.name.clone()),
            (false, _) => None,
        };
        let origin = Some(Origin { name, ..base });
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

    /// Insert published `x` as a new child of the scratch's `parent` at `index`
    /// and fill it by [`Self::sync`]. Answers the new node's id.
    ///
    /// 🚨 **A shell and a sync, never the published subtree copied whole** (§15
    /// D1024, the kits QA review's second pass): a node new to the publish may
    /// hold layers the document already has — a group or a frame the kit made
    /// around existing layers (*Group selection*, *Frame selection*) — and a
    /// whole copy minted them a second time while the held ones were dropped, so
    /// every instance kept its overridden layer detached beside a fresh one. The
    /// sync moves a held layer in and copies only what is new.
    fn insert_new(
        &mut self,
        ids: &mut IdSource,
        parent: NodeId,
        index: usize,
        x: NodeId,
    ) -> Result<NodeId, KitError> {
        let index = index.min(self.scratch.get(parent).map_or(0, |p| p.children.len()));
        let shell = self.insert_shell(ids, parent, index, x)?;
        self.sync(ids, shell, x)?;
        Ok(shell)
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
    // The instance is built against the document the vendoring leaves, which
    // holds the main to copy.
    let (mut ops, scratch, shown) = vendored(doc, ids, kit, version, published, main)?;
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

/// [`vendor`], and the document its ops leave — what a verb that goes on to use
/// the vendored main builds the rest of its transaction against: [`insert`]'s
/// instance, or a swap to a kit main the document has never used (§15 D1019).
/// The vault is never resolved, so the scratch is the only place the copy can be
/// read before the commit.
pub fn vendored(
    doc: &Document,
    ids: &mut IdSource,
    kit: &str,
    version: u32,
    published: &Published,
    main: NodeId,
) -> Result<(Vec<Operation>, Document, NodeId), KitError> {
    let (ops, shown) = vendor(doc, ids, kit, version, published, main)?;
    let mut scratch = doc.clone();
    scratch
        .apply_unchecked(&Transaction(ops.clone()))
        .map_err(|_| KitError::Incomplete(main))?;
    Ok((ops, scratch, shown))
}

/// The mains of kit `kit`'s published state that a swap at `root` could show and
/// `doc` **does not hold yet** — the swap picker's offer past the vault (§15
/// D1019): a pick is vendored at the swap, through [`vendored`], and swapped to
/// in the same transaction. What `doc` already holds of the kit is
/// `swap::options`' to list, as any main of `doc`'s own.
///
/// The same rules as `swap::options`: `root` must take a swap, a main of the
/// **same kind** only, `filter` a prefix of the name and `search` anywhere in it,
/// both without case; sorted by name, then id. A main the kit itself vendored
/// from a third kit — its published vault — is not offered: it is that kit's,
/// and the third kit lists it. ⚠️ **No cycle check here**, unlike
/// `swap::can_swap_to`: a kit main reaches only kit mains, never one of `doc`'s
/// own, so it cannot hold the main `root` sits in — and the swap verb's own
/// check runs on the scratch [`vendored`] leaves, so a wrong argument here is
/// refused there rather than committed.
pub fn swap_offers(
    doc: &Document,
    kit: &str,
    published: &Published,
    root: NodeId,
    filter: &str,
    search: &str,
) -> Vec<NodeId> {
    let Some(r) = doc.get(root) else {
        return Vec::new();
    };
    if !crate::swap::can_swap(doc, root) {
        return Vec::new();
    }
    let src = published.document();
    let filter = filter.trim().to_lowercase();
    let search = search.trim().to_lowercase();
    let held = held_index(doc);
    let mut out: Vec<&Node> = src
        .node_map()
        .values()
        .filter(|n| n.component && !src.in_vault(n.id))
        .filter(|n| std::mem::discriminant(&n.kind) == std::mem::discriminant(&r.kind))
        .filter(|n| {
            let name = n.name.to_lowercase();
            name.starts_with(&filter) && name.contains(&search)
        })
        .filter(|n| find(doc, &held, &(kit.to_string(), n.id)).is_none())
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    out.into_iter().map(|n| n.id).collect()
}
