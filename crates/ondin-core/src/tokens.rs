//! The variables and styles a kit carries (§5.3e, §5.3f, §15 D1064).
//!
//! A kit's published state is a document, and since step 4 it holds the
//! variables and styles its mains read, as any document does: **the table is
//! frozen with the mains** (`crate::kit::freeze`). A consumer vendoring a main
//! takes them too, into its own tables, each **minted an id of the consumer's**
//! and marked with where it came from ([`VarOrigin`], [`StyleOrigin`]) — exactly
//! what a vendored node is (§15 D1010) — and the main's copy is pointed at them.
//! A kit's modes are held beside them as [`KitModes`], never as modes of the
//! document (D1038 (b)).
//!
//! **One function does that for every door** ([`plan`]): the insert's vendoring
//! (`crate::kit::vendor`), an update (`crate::kit::update_all`, which also
//! rewrites what it holds), and a carrying paste (`crate::kit::adopt`), whose
//! source is the other document's tables as the clipboard took them
//! ([`Tokens`]). Each source variable lands on the consumer's copy of the same
//! identity when there is one, whatever version it holds — bringing it up to date
//! is an update's — and is minted fresh otherwise.

use crate::document::Document;
use crate::id::IdSource;
use crate::kit::KitError;
use crate::node::Node;
use crate::op::Operation;
use crate::style::{Style, StyleId, StyleOrigin, StyleValue, Styles};
use crate::variable::{
    BrushSource, Cell, KitMode, KitModes, ModeId, VarId, VarOrigin, Variable, Variables,
};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A copy of the kits' variables and styles some layers read — what the
/// clipboard carries beside the vault mains (`crate::kit::carried`), so a paste
/// into another document can vendor them with the mains ([`carried`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<Variable>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kits: BTreeMap<String, KitModes>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<Style>,
}

impl Tokens {
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.kits.is_empty() && self.styles.is_empty()
    }
    /// Whether the copy carries variable `v` — a paste keeps a binding to it, to
    /// be pointed at the vendored copy (`crate::variable::land`).
    pub fn holds_var(&self, v: VarId) -> bool {
        self.vars.iter().any(|x| x.id == v)
    }
    /// Whether the copy carries style `s`.
    pub fn holds_style(&self, s: StyleId) -> bool {
        self.styles.iter().any(|x| x.id == s)
    }
    fn tables(&self) -> (Variables, Styles) {
        let vars = Variables {
            modes: Vec::new(),
            vars: self.vars.iter().map(|v| (v.id, v.clone())).collect(),
            kits: self.kits.clone(),
        };
        (vars, Styles::from_list(self.styles.clone()))
    }
}

/// Every variable id a node's own fields read.
pub(crate) fn node_vars(n: &Node) -> impl Iterator<Item = VarId> + '_ {
    n.paint
        .fills
        .iter()
        .filter_map(|f| f.brush.var())
        .chain(n.paint.strokes.iter().filter_map(|s| s.brush.var()))
        .chain(n.corner_vars.iter().flatten().copied())
        .chain(n.spacing_vars.iter().flatten().copied())
        .chain(n.size_vars.iter().flatten().copied())
        .chain(n.visible_var)
}

/// The variables and styles `nodes` read in `vars` and `styles`, **closed**: a
/// bound style's paints, and every alias in every cell, followed — the set a
/// frozen copy or a vendoring must carry for every binding to resolve.
pub(crate) fn reached<'a>(
    vars: &Variables,
    styles: &Styles,
    nodes: impl IntoIterator<Item = &'a Node>,
) -> (Vec<VarId>, Vec<StyleId>) {
    let mut vs: Vec<VarId> = Vec::new();
    let mut ss: Vec<StyleId> = Vec::new();
    let mut pending: Vec<VarId> = Vec::new();
    for n in nodes {
        pending.extend(node_vars(n));
        for (_, s) in n.styles.iter() {
            if !ss.contains(&s)
                && let Some(st) = styles.get(s)
            {
                ss.push(s);
                pending.extend(st.value.vars());
            }
        }
    }
    let mut seen: FxHashSet<VarId> = FxHashSet::default();
    while let Some(v) = pending.pop() {
        if !seen.insert(v) {
            continue;
        }
        let Some(var) = vars.get(v) else { continue };
        vs.push(v);
        for c in var.cells.values() {
            if let Cell::Alias(t) = c {
                pending.push(*t);
            }
        }
    }
    vs.sort();
    ss.sort();
    (vs, ss)
}

/// What a copy of `subtrees` and the vault mains it carries reads of `doc`'s
/// **kits'** variables and styles — the clipboard's half of a carrying paste
/// (§15 D1064). The document's own are not here: a paste rebinds those by name
/// or detaches them (D1038's rule, `crate::variable::land`).
pub fn carried<'a>(doc: &Document, nodes: impl IntoIterator<Item = &'a Node>) -> Tokens {
    let t = doc.variables();
    let (vs, ss) = reached(t, doc.styles(), nodes);
    let vars: Vec<Variable> = vs
        .iter()
        .filter_map(|v| t.get(*v))
        .filter(|v| v.origin.is_some())
        .cloned()
        .collect();
    let styles: Vec<Style> = ss
        .iter()
        .filter_map(|s| doc.styles().get(*s))
        .filter(|s| s.origin.is_some())
        .cloned()
        .collect();
    let mut kits = BTreeMap::new();
    for k in vars.iter().filter_map(Variable::kit) {
        if let Some(km) = t.kits().get(k) {
            kits.entry(k.to_string()).or_insert_with(|| KitModes {
                map: BTreeMap::new(),
                ..km.clone()
            });
        }
    }
    Tokens { vars, kits, styles }
}

/// One of a kit's modes as a [`Source`] holds it: its id there, the kit's own id
/// for it (what a consumer's [`KitMode::from`] matches), and its name.
type SourceMode = (ModeId, ModeId, String);

/// Where a [`plan`]'s variables and styles come from.
pub(crate) struct Source<'a> {
    pub vars: &'a Variables,
    pub styles: &'a Styles,
    /// The kit whose published state these tables are — its id, the version,
    /// its name — whose **own** variables (no origin there) are this kit's.
    /// `None` for a clipboard's [`Tokens`], every one of which names its kit.
    pub kit: Option<(&'a str, u32, Option<&'a str>)>,
}

impl Source<'_> {
    fn var_origin(&self, v: &Variable) -> Option<VarOrigin> {
        match (&v.origin, self.kit) {
            (Some(o), _) => Some(o.clone()),
            (None, Some((kit, version, _))) => Some(VarOrigin {
                kit: kit.to_string(),
                var: v.id,
                version,
            }),
            (None, None) => None,
        }
    }
    fn style_origin(&self, s: &Style) -> Option<StyleOrigin> {
        match (&s.origin, self.kit) {
            (Some(o), _) => Some(o.clone()),
            (None, Some((kit, version, _))) => Some(StyleOrigin {
                kit: kit.to_string(),
                style: s.id,
                version,
            }),
            (None, None) => None,
        }
    }
    /// Kit `kit`'s modes as this source holds them, in the kit's order — the
    /// source's own modes for its own kit — and the kit's name.
    fn modes_of(&self, kit: &str) -> Option<(Vec<SourceMode>, Option<String>)> {
        if let Some((own, _, name)) = self.kit
            && own == kit
        {
            let modes = self
                .vars
                .modes()
                .iter()
                .map(|m| (m.id, m.id, m.name.clone()))
                .collect();
            return Some((modes, name.map(str::to_string)));
        }
        let km = self.vars.kits().get(kit)?;
        Some((
            km.modes
                .iter()
                .map(|m| (m.id, m.from, m.name.clone()))
                .collect(),
            km.name.clone(),
        ))
    }
    /// The kit a mode of this source belongs to.
    fn kit_of_mode(&self, m: ModeId) -> Option<String> {
        if let Some((own, _, _)) = self.kit
            && self.vars.mode(m).is_some()
        {
            return Some(own.to_string());
        }
        self.vars.kit_mode_of(m).map(|(k, _)| k.to_string())
    }
}

/// What [`plan`] built: the ops that bring the consumer's tables to hold what
/// the source nodes read, and the maps that point a copy of those nodes at them.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    pub ops: Vec<Operation>,
    pub vars: FxHashMap<VarId, VarId>,
    pub modes: FxHashMap<ModeId, ModeId>,
    pub styles: FxHashMap<StyleId, StyleId>,
    /// For an update: the kit's variables and styles this document held and
    /// the publish no longer has, detached from their users and removed.
    pub removed_vars: Vec<VarId>,
    pub removed_styles: Vec<StyleId>,
    /// For an update: the kit's variables and styles it added or rewrote.
    pub changed_vars: Vec<VarId>,
    pub changed_styles: Vec<StyleId>,
}

impl Plan {
    /// Point `n`'s bindings, its frame's mode and its styles at the consumer's
    /// copies. An id the plan does not know is left as it is — a variable the
    /// target already held by the paste rule, or one of the node's own document.
    pub(crate) fn apply(&self, n: &mut Node) {
        let v = |x: &mut VarId| {
            if let Some(to) = self.vars.get(x) {
                *x = *to;
            }
        };
        for f in n.paint.fills.iter_mut() {
            if let BrushSource::Bound { var, .. } = &mut f.brush {
                v(var);
            }
        }
        for s in n.paint.strokes.iter_mut() {
            if let BrushSource::Bound { var, .. } = &mut s.brush {
                v(var);
            }
        }
        n.corner_vars.iter_mut().flatten().for_each(v);
        n.spacing_vars.iter_mut().flatten().for_each(v);
        n.size_vars.iter_mut().flatten().for_each(v);
        if let Some(x) = &mut n.visible_var {
            v(x);
        }
        if let Some(m) = n.mode
            && let Some(to) = self.modes.get(&m)
        {
            n.mode = Some(*to);
        }
        let refs = n.styles;
        for (slot, s) in refs.iter() {
            if let Some(to) = self.styles.get(&s) {
                n.styles.set(slot, Some(*to));
            }
        }
    }

    /// A style's value with its paints pointed at the consumer's variables.
    fn remap_value(&self, value: &StyleValue) -> StyleValue {
        let to = |b: &BrushSource| match b {
            BrushSource::Bound { var, alpha } => BrushSource::Bound {
                var: *self.vars.get(var).unwrap_or(var),
                alpha: *alpha,
            },
            b => b.clone(),
        };
        match value {
            StyleValue::Fills(v) => StyleValue::Fills(
                v.iter()
                    .map(|f| {
                        f.map(|f| crate::node::Fill {
                            brush: to(&f.brush),
                            ..f.clone()
                        })
                    })
                    .collect(),
            ),
            StyleValue::Strokes(v) => StyleValue::Strokes(
                v.iter()
                    .map(|s| {
                        s.map(|s| crate::node::Stroke {
                            brush: to(&s.brush),
                            ..s.clone()
                        })
                    })
                    .collect(),
            ),
            StyleValue::Effects(_) => value.clone(),
        }
    }
}

/// Build the [`Plan`] that lands what `nodes` — nodes of `src` — read into
/// `doc` (§15 D1064).
///
/// Each kit whose variables are read gets its modes held here (a [`KitModes`]),
/// matched to the source's by the kit's own mode ids; a variable or style lands
/// on `doc`'s copy of the same identity, else is minted; a frame's mode is
/// mapped to the consumer's id for the same kit mode.
///
/// `sync`, the kit an update brings `doc` up to (`crate::kit::update_all`): every
/// variable and style of that kit `doc` holds is rewritten to the source's — its
/// name, its cells, its value — the kit's modes are taken as the publish has
/// them (a mode added gets an id, one gone is dropped, a hand-set mapping entry
/// kept while its kit mode stays), and what the publish no longer has is
/// detached from its users and removed. Without `sync`, a kit whose modes `doc`
/// holds otherwise than the source is [`KitError::Modes`]: the consumer's
/// variables of it have no cell in a mode it lacks, and an update comes first.
pub(crate) fn plan<'a>(
    doc: &Document,
    ids: &mut IdSource,
    src: &Source<'_>,
    nodes: impl IntoIterator<Item = &'a Node>,
    sync: Option<&str>,
) -> Result<Plan, KitError> {
    let nodes: Vec<&Node> = nodes.into_iter().collect();
    let (mut want_vars, mut want_styles) = reached(src.vars, src.styles, nodes.iter().copied());
    // An update writes every variable and style of its kit, not only those the
    // synced mains read: a consumer's own layer may be bound to any of them.
    if let Some(kit) = sync {
        for v in src.vars.iter() {
            if src.var_origin(v).is_some_and(|o| o.kit == kit) && !want_vars.contains(&v.id) {
                want_vars.push(v.id);
            }
        }
        for s in src.styles.iter() {
            if src.style_origin(s).is_some_and(|o| o.kit == kit) && !want_styles.contains(&s.id) {
                want_styles.push(s.id);
                for v in s.value.vars() {
                    if !want_vars.contains(&v) {
                        want_vars.push(v);
                    }
                }
            }
        }
        // Aliases of what was added, closed again.
        let mut i = 0;
        while i < want_vars.len() {
            if let Some(var) = src.vars.get(want_vars[i]) {
                for c in var.cells.values() {
                    if let Cell::Alias(t) = c
                        && !want_vars.contains(t)
                        && src.vars.get(*t).is_some()
                    {
                        want_vars.push(*t);
                    }
                }
            }
            i += 1;
        }
    }

    let t = doc.variables();
    let mut plan = Plan::default();
    let mut records: BTreeMap<String, KitModes> = BTreeMap::new();
    let mut changed_records: Vec<String> = Vec::new();

    // The kits read: by the variables, and by any frame set to a kit's mode.
    let mut kits: Vec<String> = Vec::new();
    for v in &want_vars {
        if let Some(o) = src.vars.get(*v).and_then(|v| src.var_origin(v))
            && !kits.contains(&o.kit)
        {
            kits.push(o.kit);
        }
    }
    for n in &nodes {
        if let Some(m) = n.mode
            && let Some(k) = src.kit_of_mode(m)
            && !kits.contains(&k)
        {
            kits.push(k);
        }
    }
    if let Some(k) = sync
        && !kits.iter().any(|x| x == k)
        && t.kits().contains_key(k)
    {
        kits.push(k.to_string());
    }

    // 🚨 **The kit's own document holds its own variables, never copies of
    // them** (`crate::kit::find`'s own-kit arm, §15 D1023, made for the tables):
    // a kit instance pasted back into its kit links to the live main, and kit
    // copies of the kit's variables beside the originals would make every bound
    // field of it an override (`arch-scribe`'s reading of the first build).
    let own = doc.meta().id.as_deref();
    for kit in &kits {
        let (src_modes, name) = match src.modes_of(kit) {
            Some(m) => m,
            None if sync == Some(kit.as_str()) => (Vec::new(), None),
            None => continue,
        };
        if own == Some(kit.as_str()) {
            for (sid, from, _) in &src_modes {
                if t.mode(*from).is_some() {
                    plan.modes.insert(*sid, *from);
                }
            }
            continue;
        }
        let held = t.kits().get(kit);
        // A held record **covering** the source's modes takes its variables: a
        // consumer holding a third kit at a newer version — a mode more — than
        // the kit that carries it is the ordinary case, as a main held at any
        // version is reused (`arch-scribe`'s reading: refusing it named an
        // update no update could give).
        let covers = held.is_some_and(|h| {
            src_modes
                .iter()
                .all(|(_, from, _)| h.modes.iter().any(|m| m.from == *from))
        });
        let record = match held {
            Some(h) if covers && sync != Some(kit.as_str()) => h.clone(),
            Some(h) if sync == Some(kit.as_str()) => {
                let modes: Vec<KitMode> = src_modes
                    .iter()
                    .map(|(_, from, nm)| KitMode {
                        id: h
                            .modes
                            .iter()
                            .find(|m| m.from == *from)
                            .map_or_else(|| ids.mint_mode(), |m| m.id),
                        name: nm.clone(),
                        from: *from,
                    })
                    .collect();
                let map = h
                    .map
                    .iter()
                    .filter(|(_, to)| modes.iter().any(|m| m.id == **to))
                    .map(|(a, b)| (*a, *b))
                    .collect();
                KitModes {
                    name: name.clone().or_else(|| h.name.clone()),
                    modes,
                    map,
                }
            }
            Some(_) => return Err(KitError::Modes(kit.clone())),
            None => KitModes {
                name: name.clone(),
                modes: src_modes
                    .iter()
                    .map(|(_, from, nm)| KitMode {
                        id: ids.mint_mode(),
                        name: nm.clone(),
                        from: *from,
                    })
                    .collect(),
                map: BTreeMap::new(),
            },
        };
        for (sid, from, _) in &src_modes {
            if let Some(m) = record.modes.iter().find(|m| m.from == *from) {
                plan.modes.insert(*sid, m.id);
            }
        }
        if held != Some(&record) {
            changed_records.push(kit.clone());
        }
        records.insert(kit.clone(), record);
    }

    // The variables: the held copy of each identity, else a fresh id.
    // A variable of a kit with no modes in the source has nothing to be held
    // under, and is left unmapped rather than pointed at nothing.
    for v in &want_vars {
        let Some(var) = src.vars.get(*v) else {
            continue;
        };
        let Some(o) = src.var_origin(var) else {
            continue;
        };
        // The kit's own: its own variable, when it still has it.
        if own == Some(o.kit.as_str()) {
            if t.get(o.var).is_some_and(|x| x.origin.is_none()) {
                plan.vars.insert(*v, o.var);
            }
            continue;
        }
        if !records.contains_key(&o.kit) {
            continue;
        }
        let to = match t.by_origin(o.key()) {
            Some(h) => h.id,
            None => ids.mint_var(),
        };
        plan.vars.insert(*v, to);
    }
    for v in &want_vars {
        let Some(var) = src.vars.get(*v) else {
            continue;
        };
        let Some(o) = src.var_origin(var) else {
            continue;
        };
        let Some(&to) = plan.vars.get(v) else {
            continue;
        };
        let held = t.get(to);
        let rewrite = held.is_none() || sync == Some(o.kit.as_str());
        if !rewrite || own == Some(o.kit.as_str()) {
            continue;
        }
        let remap = |c: &Cell| match c {
            Cell::Alias(a) => Cell::Alias(*plan.vars.get(a).unwrap_or(a)),
            c => *c,
        };
        let mut cells: BTreeMap<ModeId, Cell> = var
            .cells
            .iter()
            .filter_map(|(m, c)| Some((*plan.modes.get(m)?, remap(c))))
            .collect();
        // A mode the held record has and the source does not — a third kit held
        // newer than the kit carrying it — takes the source's first mode's cell,
        // that kit's default.
        let first = src.modes_of(&o.kit).and_then(|(m, _)| {
            m.first()
                .and_then(|(sid, _, _)| var.cells.get(sid))
                .map(remap)
        });
        if let (Some(r), Some(first)) = (records.get(&o.kit), first) {
            for m in &r.modes {
                cells.entry(m.id).or_insert(first);
            }
        }
        let next = Variable {
            id: to,
            name: var.name.clone(),
            kind: var.kind,
            cells,
            origin: Some(o),
        };
        match held {
            None => {
                plan.changed_vars.push(to);
                plan.ops.push(Operation::AddVariable { var: next });
            }
            Some(h) => {
                // The version alone is no change anyone reads.
                let mut a = h.clone();
                let mut b = next.clone();
                if let (Some(x), Some(y)) = (a.origin.as_mut(), b.origin.as_mut()) {
                    x.version = 0;
                    y.version = 0;
                }
                if a != b {
                    plan.changed_vars.push(to);
                }
                if *h != next {
                    plan.ops.push(Operation::SetVariable { var: next });
                }
            }
        }
    }

    // The styles, the same way.
    for s in &want_styles {
        let Some(style) = src.styles.get(*s) else {
            continue;
        };
        let Some(o) = src.style_origin(style) else {
            continue;
        };
        if own == Some(o.kit.as_str()) {
            if doc
                .styles()
                .get(o.style)
                .is_some_and(|x| x.origin.is_none())
            {
                plan.styles.insert(*s, o.style);
            }
            continue;
        }
        let to = match doc.styles().by_origin((&o.kit, o.style)) {
            Some(h) => h.id,
            None => ids.mint_style(),
        };
        plan.styles.insert(*s, to);
    }
    for s in &want_styles {
        let Some(style) = src.styles.get(*s) else {
            continue;
        };
        let Some(o) = src.style_origin(style) else {
            continue;
        };
        let Some(&to) = plan.styles.get(s) else {
            continue;
        };
        let held = doc.styles().get(to);
        if (held.is_some() && sync != Some(o.kit.as_str())) || own == Some(o.kit.as_str()) {
            continue;
        }
        let value = plan.remap_value(&style.value);
        match held {
            None => {
                plan.changed_styles.push(to);
                plan.ops.push(Operation::AddStyle {
                    style: Style {
                        id: to,
                        name: style.name.clone(),
                        value,
                        origin: Some(o),
                    },
                });
            }
            Some(h) => {
                let mut wrote = false;
                if h.name != style.name {
                    plan.ops.push(Operation::SetStyleName {
                        id: to,
                        name: style.name.clone(),
                    });
                    wrote = true;
                }
                if h.value != value {
                    plan.ops.push(Operation::SetStyleValue { id: to, value });
                    wrote = true;
                }
                if h.origin.as_ref().map(|x| x.version) != Some(o.version) {
                    plan.ops.push(Operation::SetStyleOrigin {
                        id: to,
                        origin: Some(o),
                    });
                }
                if wrote {
                    plan.changed_styles.push(to);
                }
            }
        }
    }

    // An update's removals: what this document holds of the kit and the
    // publish no longer has — detached from every user, values kept (5A).
    if let Some(kit) = sync {
        let kept_v: FxHashSet<VarId> = plan.vars.values().copied().collect();
        plan.removed_vars = t
            .of_kit(kit)
            .map(|v| v.id)
            .filter(|v| !kept_v.contains(v))
            .collect();
        let kept_s: FxHashSet<StyleId> = plan.styles.values().copied().collect();
        plan.removed_styles = doc
            .styles()
            .iter()
            .filter(|s| s.origin.as_ref().is_some_and(|o| o.kit == kit))
            .map(|s| s.id)
            .filter(|s| !kept_s.contains(s))
            .collect();
    }

    let mut ops: Vec<Operation> = Vec::new();
    // Removals first, read against the document before anything is rewritten,
    // so each user detaches to what it drew.
    //
    // 🚨 **Except a copy that reads it only where its main does**: the main is
    // rewritten by the sync, and the commit's follow carries that to the copy
    // — a detach written here would be the user's write of the copy, read by
    // the commit as an override, and the instance would keep the old colour
    // where its main changed (this test file's first run).
    let follows = |op: &Operation, removed: &dyn Fn(&Node) -> bool| {
        op.overwrites()
            .and_then(|id| doc.get(id))
            .is_some_and(|n| !doc.in_vault(n.id) && removed(n))
    };
    if !plan.removed_styles.is_empty() {
        let gone = &plan.removed_styles;
        let same = |n: &Node| {
            n.link.and_then(|l| doc.get(l)).is_some_and(|s| {
                n.styles
                    .iter()
                    .filter(|(_, st)| gone.contains(st))
                    .all(|(slot, st)| s.styles.get(slot) == Some(st))
            })
        };
        ops.extend(
            crate::style::remove_ops(doc, gone)
                .into_iter()
                .filter(|op| !follows(op, &same)),
        );
    }
    if !plan.removed_vars.is_empty() {
        let gone = &plan.removed_vars;
        let same = |n: &Node| {
            n.link
                .and_then(|l| doc.get(l))
                .is_some_and(|s| reads_as_its_source(n, s, gone))
        };
        ops.extend(
            crate::variable::remove_ops(doc, gone)
                .into_iter()
                .filter(|op| !follows(op, &same)),
        );
    }
    for kit in changed_records {
        ops.push(Operation::SetKitModes {
            modes: records.remove(&kit),
            kit,
        });
    }
    ops.append(&mut plan.ops);
    plan.ops = ops;
    Ok(plan)
}

/// Whether every field of copy `n` bound to one of `gone` is bound alike on its
/// source `s` — so the commit's follow, not a detach, is what rewrites it.
fn reads_as_its_source(n: &Node, s: &Node, gone: &[VarId]) -> bool {
    let hit = |v: Option<VarId>| v.is_some_and(|v| gone.contains(&v));
    let paints = |a: &[&BrushSource], b: &[&BrushSource]| {
        a.iter()
            .enumerate()
            .all(|(i, x)| !hit(x.var()) || b.get(i).is_some_and(|y| y == x))
    };
    let fills: Vec<&BrushSource> = n.paint.fills.iter().map(|f| &f.brush).collect();
    let sfills: Vec<&BrushSource> = s.paint.fills.iter().map(|f| &f.brush).collect();
    let strokes: Vec<&BrushSource> = n.paint.strokes.iter().map(|f| &f.brush).collect();
    let sstrokes: Vec<&BrushSource> = s.paint.strokes.iter().map(|f| &f.brush).collect();
    let slots =
        |a: &[Option<VarId>], b: &[Option<VarId>]| a.iter().zip(b).all(|(x, y)| !hit(*x) || x == y);
    paints(&fills, &sfills)
        && paints(&strokes, &sstrokes)
        && slots(&n.corner_vars, &s.corner_vars)
        && slots(&n.spacing_vars, &s.spacing_vars)
        && slots(&n.size_vars, &s.size_vars)
        && (!hit(n.visible_var) || n.visible_var == s.visible_var)
}

/// [`plan`] for a clipboard's [`Tokens`] — a carrying paste (`crate::kit::adopt`).
pub(crate) fn plan_carried<'a>(
    doc: &Document,
    ids: &mut IdSource,
    tokens: &Tokens,
    nodes: impl IntoIterator<Item = &'a Node>,
) -> Result<Plan, KitError> {
    if tokens.is_empty() {
        return Ok(Plan::default());
    }
    let (vars, styles) = tokens.tables();
    plan(
        doc,
        ids,
        &Source {
            vars: &vars,
            styles: &styles,
            kit: None,
        },
        nodes,
        None,
    )
}
