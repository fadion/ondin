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
        .chain(n.paint.strokes.iter().filter_map(|s| s.width_var))
        .chain(
            n.effects
                .iter()
                .flat_map(|e| e.vars.iter().flatten().copied()),
        )
        .chain(n.corner_vars.iter().flatten().copied())
        .chain(n.spacing_vars.iter().flatten().copied())
        .chain(n.size_vars.iter().flatten().copied())
        .chain(n.text_vars.iter().flatten().copied())
        .chain(n.visible_var)
        .chain(n.opacity_var)
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
            if let Some(x) = &mut s.width_var {
                v(x);
            }
        }
        for e in n.effects.iter_mut() {
            e.vars.iter_mut().flatten().for_each(v);
        }
        n.corner_vars.iter_mut().flatten().for_each(v);
        n.spacing_vars.iter_mut().flatten().for_each(v);
        n.size_vars.iter_mut().flatten().for_each(v);
        n.text_vars.iter_mut().flatten().for_each(v);
        if let Some(x) = &mut n.visible_var {
            v(x);
        }
        if let Some(x) = &mut n.opacity_var {
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
            // A text style reads no variable (§15 D1067).
            StyleValue::Effects(_) | StyleValue::Text(_) => value.clone(),
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
            c => c.clone(),
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
                cells.entry(m.id).or_insert_with(|| first.clone());
            }
        }
        let next = Variable {
            id: to,
            name: var.name.clone(),
            kind: var.kind,
            cells,
            origin: Some(o),
            forked: None,
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
                        forked: None,
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
        // Alike as the variables' filter has it (`alike`): the same style, or
        // — on a vault source, which only the up-front fork rebinds to one of
        // the document's own — the document's fork of it (`arch-scribe`'s
        // reading (e) of D1066's fixes: the copy was unbound from a style its
        // main reads, an override).
        let styles = doc.styles();
        let forked_of = |mine: Option<StyleId>, kits: StyleId| {
            mine.and_then(|m| styles.get(m)?.forked.as_ref())
                .zip(styles.get(kits).and_then(|k| k.origin.as_ref()))
                .is_some_and(|(f, o)| (f.kit.as_str(), f.style) == (o.kit.as_str(), o.style))
        };
        let same = |n: &Node| {
            n.link.and_then(|l| doc.get(l)).is_some_and(|s| {
                n.styles
                    .iter()
                    .filter(|(_, st)| gone.contains(st))
                    .all(|(slot, st)| {
                        s.styles.get(slot) == Some(st)
                            || (doc.in_vault(s.id) && forked_of(s.styles.get(slot), st))
                    })
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
                .is_some_and(|s| reads_as_its_source(n, s, gone, t, doc.in_vault(s.id)))
        };
        // 🚨 **Not a style this removal also removes**: `remove_ops` detaches
        // every style reading a removed variable, and a style the kit dropped
        // with it is already gone above — its `SetStyleValue` named nothing,
        // and every update dropping a style and a variable it read failed for
        // good (`Incomplete`). Found by D1066's style test, not by a reading.
        let dropped = &plan.removed_styles;
        ops.extend(
            crate::variable::remove_ops(doc, gone)
                .into_iter()
                .filter(|op| !follows(op, &same))
                .filter(
                    |op| !matches!(op, Operation::SetStyleValue { id, .. } if dropped.contains(id)),
                ),
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

/// Whether `y`, a source's binding, is alike `x`, a copy's: the same variable,
/// or the document's fork of it (§15 D1066).
fn alike(t: &Variables, x: VarId, y: Option<VarId>) -> bool {
    y == Some(x)
        || y.and_then(|y| t.get(y)?.forked.clone())
            .zip(t.get(x).and_then(|v| v.origin.clone()))
            .is_some_and(|(f, o)| f.key() == o.key())
}

/// Whether every field of copy `n` bound to one of `gone` is bound alike on its
/// source `s` — so the commit's follow, not a detach, is what rewrites it.
///
/// **Alike counts a fork** (§15 D1066, `arch-scribe`'s reading (a) of its
/// fixes): a main the kit removes is forked before this plan runs
/// (`kit::update_in_order`), so its binding is the document's fork of the
/// variable the copy still reads — and that copy follows the main's rewrite
/// onto the fork. Read as unlike, the copy was detached to one mode's literal,
/// which the commit took for an override. And **paints match by item id**
/// (D980), as the follow does — by position, a drifted copy's own fill could be
/// spared onto a variable the plan removes, and the whole update was refused
/// (`BadBinding`). ⚠️ **The answer is the node's, and one `SetFills` covers a
/// whole list**: a drifted copy with an own fill to detach has its followed
/// fill detached with it, to the colour it drew — an override where its main
/// moves on. Leaving that item bound for the follow does not work: the follow
/// takes no item of a list the transaction also wrote, and the commit refuses
/// the binding. Measured; kept as the edge it is.
///
/// 🚨 **The fork counts only on a source in the vault** (`vault`;
/// `arch-scribe`'s reading (d)): a vault node binds kits' variables alone, so
/// a fork there is one the up-front block just wrote, and this update's sync
/// or `make_local` carries it. A main made local before is outside the vault
/// and no edit of this update's — counting its fork alike spared a copy the
/// follow then never rewrote, left bound to a removed variable (`BadBinding`).
fn reads_as_its_source(n: &Node, s: &Node, gone: &[VarId], t: &Variables, vault: bool) -> bool {
    let hit = |v: Option<VarId>| v.is_some_and(|v| gone.contains(&v));
    let alike = |x: VarId, y: Option<VarId>| y == Some(x) || (vault && alike(t, x, y));
    let brush = |x: &BrushSource, y: Option<&BrushSource>| match (x, y) {
        (BrushSource::Bound { var, alpha }, Some(BrushSource::Bound { var: v2, alpha: a2 })) => {
            alpha == a2 && alike(*var, Some(*v2))
        }
        _ => false,
    };
    let fills = n.paint.fills.iter().all(|f| {
        !hit(f.brush.var())
            || brush(
                &f.brush,
                s.paint
                    .fills
                    .iter()
                    .find(|k| k.id == f.id)
                    .map(|k| &k.brush),
            )
    });
    let strokes = n.paint.strokes.iter().all(|f| {
        let src = s.paint.strokes.iter().find(|k| k.id == f.id);
        (!hit(f.brush.var()) || brush(&f.brush, src.map(|k| &k.brush)))
            // And its width's binding (§15 D1073).
            && (!hit(f.width_var)
                || f.width_var.is_some_and(|x| alike(x, src.and_then(|k| k.width_var))))
    });
    let slots = |a: &[Option<VarId>], b: &[Option<VarId>]| {
        a.iter()
            .zip(b)
            .all(|(x, y)| !hit(*x) || x.is_some_and(|x| alike(x, *y)))
    };
    // And each effect's numbers (§15 D1074), by item.
    let effects = n.effects.iter().all(|e| {
        let none = [None; 4];
        let src = s
            .effects
            .iter()
            .find(|k| k.id == e.id)
            .map_or(&none, |k| &k.vars);
        slots(&e.vars, src)
    });
    fills
        && strokes
        && effects
        && slots(&n.corner_vars, &s.corner_vars)
        && slots(&n.spacing_vars, &s.spacing_vars)
        && slots(&n.size_vars, &s.size_vars)
        && slots(&n.text_vars, &s.text_vars)
        && (!hit(n.visible_var) || n.visible_var.is_some_and(|x| alike(x, s.visible_var)))
        && (!hit(n.opacity_var) || n.opacity_var.is_some_and(|x| alike(x, s.opacity_var)))
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

/// What *Make local* forks of the kits' variables and styles (§15 D1066): the
/// document's own copies of those the revealed mains read, and where each
/// kit's one went.
///
/// A main made local no longer follows its kit, so it must not go on reading
/// the kit's variables either — those are rewritten by every update while
/// another of the kit's mains is held, and read-only and orphaned once none
/// is. Detaching to literals was the other answer and the worse one: a literal
/// is one mode's value, and every instance of the main in another mode would
/// have changed colour without a word. A fork keeps every mode's value.
#[derive(Debug, Default)]
pub(crate) struct Fork {
    /// The fork's own: modes the document gains, then the variables and
    /// styles — every one before the bindings that read it.
    pub ops: Vec<Operation>,
    vars: FxHashMap<VarId, VarId>,
    styles: FxHashMap<StyleId, StyleId>,
    /// The revealed nodes set to a kit's own mode, which the forks do not
    /// read — a kit's mode governs its kit's variables alone — so they may
    /// draw otherwise once forked: the floating status says so.
    pub kit_frames: Vec<crate::NodeId>,
    /// How many forks were made, not reused: variables, styles.
    pub made: (usize, usize),
}

/// Whether `n`'s binding `x` in a field is its own — not the same binding its
/// link source `src` holds in that field, which the commit's follow carries
/// and a fork must not write over (or it reads as an override).
fn own<T: PartialEq>(x: Option<T>, at_src: Option<T>) -> Option<T> {
    match (&x, &at_src) {
        (Some(a), Some(b)) if a == b => None,
        _ => x,
    }
}

/// The variable the fill of item `id` reads on source `s` — **by item id, as the
/// follow matches items (§15 D980)**, never by position: in a copy whose list
/// drifted, a positional match forked a followed fill into an override and left
/// a local one on the kit's variable (`arch-scribe`'s reading 2 of D1066).
fn src_fill(s: &Node, id: crate::ItemId) -> Option<VarId> {
    s.paint.fills.iter().find(|k| k.id == id)?.brush.var()
}

/// [`src_fill`] for a stroke.
fn src_stroke(s: &Node, id: crate::ItemId) -> Option<VarId> {
    s.paint.strokes.iter().find(|k| k.id == id)?.brush.var()
}

/// [`src_stroke`] for the stroke's width binding (§15 D1073).
fn src_stroke_width(s: &Node, id: crate::ItemId) -> Option<VarId> {
    s.paint.strokes.iter().find(|k| k.id == id)?.width_var
}

/// The kits' variables and styles `n`'s own bindings read ([`own`]).
fn own_reads(n: &Node, src: Option<&Node>) -> (Vec<VarId>, Vec<StyleId>) {
    let mut vs = Vec::new();
    for f in &n.paint.fills {
        vs.extend(own(f.brush.var(), src.and_then(|s| src_fill(s, f.id))));
    }
    for s in &n.paint.strokes {
        vs.extend(own(s.brush.var(), src.and_then(|x| src_stroke(x, s.id))));
        vs.extend(own(
            s.width_var,
            src.and_then(|x| src_stroke_width(x, s.id)),
        ));
    }
    let slots = |a: &[Option<VarId>], b: Option<&[Option<VarId>]>| -> Vec<VarId> {
        a.iter()
            .enumerate()
            .filter_map(|(i, x)| own(*x, b.and_then(|b| b.get(i).copied().flatten())))
            .collect()
    };
    for e in &n.effects {
        let at = src.and_then(|x| x.effects.iter().find(|k| k.id == e.id));
        vs.extend(slots(&e.vars, at.map(|k| &k.vars[..])));
    }
    vs.extend(slots(&n.corner_vars, src.map(|s| &s.corner_vars[..])));
    vs.extend(slots(&n.spacing_vars, src.map(|s| &s.spacing_vars[..])));
    vs.extend(slots(&n.size_vars, src.map(|s| &s.size_vars[..])));
    vs.extend(slots(&n.text_vars, src.map(|s| &s.text_vars[..])));
    vs.extend(own(n.visible_var, src.and_then(|s| s.visible_var)));
    vs.extend(own(n.opacity_var, src.and_then(|s| s.opacity_var)));
    let ss = n
        .styles
        .iter()
        .filter_map(|(slot, s)| own(Some(s), src.and_then(|x| x.styles.get(slot))))
        .collect();
    (vs, ss)
}

/// `base`, else `base 2`, `base 3`… — the first that `taken` refuses not.
fn free(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|k| format!("{base} {k}"))
        .find(|n| !taken(n))
        .expect("an unbounded range finds a free name")
}

/// Build the [`Fork`] for `nodes` — the revealed mains' every node, as `doc`
/// holds them (§15 D1066).
///
/// - **What is forked**: the kits' variables and styles the nodes' **own**
///   bindings read ([`own_reads`]), closed over styles and aliases. A node of a
///   nested kit instance bound as its vault main is keeps the kit's binding:
///   the main it follows is still the kit's.
/// - **A fork made before is reused** — the document's variable or style
///   whose [`Variable::forked`] is that kit's same one — so two *Make local*s
///   of one kit share it. Nothing updates a fork; it is the document's own.
/// - **Named** `Kit/name`, the kit's name as its group header has it, made
///   unique among the document's own (`name 2`).
/// - **Every mode of the document gets the cell the per-kit mapping reads
///   there** (`Variables::kit_mode`), so nothing on the canvas changes. A
///   document with **no modes of its own** — no variable of its own either
///   (`variable::check`) — takes the first forked kit's modes, by name: with
///   no hand-set entry anywhere, each maps to its kit mode of the same name,
///   so what every kit's variables read is unchanged too.
/// - **A kit's mode set on a revealed frame stays** — a nested kit instance
///   below may read it — and is reported ([`Fork::kit_frames`]).
pub(crate) fn fork(doc: &Document, ids: &mut IdSource, nodes: &[Node]) -> Fork {
    let t = doc.variables();
    let mut out = Fork::default();
    let mut pending: Vec<VarId> = Vec::new();
    let mut want_styles: Vec<StyleId> = Vec::new();
    for n in nodes {
        let src = n.link.and_then(|l| doc.get(l));
        let (vs, ss) = own_reads(n, src);
        pending.extend(vs);
        for s in ss {
            if let Some(st) = doc.styles().get(s)
                && st.origin.is_some()
                && !want_styles.contains(&s)
            {
                want_styles.push(s);
                pending.extend(st.value.vars());
            }
        }
        if n.mode.is_some_and(|m| t.kit_mode_of(m).is_some()) {
            out.kit_frames.push(n.id);
        }
    }
    let mut want: Vec<VarId> = Vec::new();
    let mut seen: FxHashSet<VarId> = FxHashSet::default();
    while let Some(v) = pending.pop() {
        if !seen.insert(v) {
            continue;
        }
        let Some(var) = t.get(v).filter(|x| x.origin.is_some()) else {
            continue;
        };
        want.push(v);
        for c in var.cells.values() {
            if let Cell::Alias(a) = c {
                pending.push(*a);
            }
        }
    }
    want.sort();
    want_styles.sort();

    let forked_var = |o: &VarOrigin| {
        t.local()
            .find(|x| x.forked.as_ref().is_some_and(|f| f.key() == o.key()))
            .map(|x| x.id)
    };
    let fresh: Vec<VarId> = want
        .iter()
        .copied()
        .filter(|v| {
            t.get(*v)
                .and_then(|x| x.origin.as_ref())
                .is_some_and(|o| forked_var(o).is_none())
        })
        .collect();

    // The document's modes the forks are written in — its own, else the first
    // forked kit's, made its own.
    let mut modes = t.clone();
    if modes.modes.is_empty()
        && let Some(km) = fresh
            .first()
            .and_then(|v| t.get(*v))
            .and_then(Variable::kit)
            .and_then(|k| t.kits().get(k))
    {
        for (i, m) in km.modes.iter().enumerate() {
            let mode = crate::variable::Mode {
                id: ids.mint_mode(),
                name: m.name.clone(),
            };
            modes.modes.push(mode.clone());
            out.ops.push(Operation::AddMode {
                mode,
                index: i,
                cells: Vec::new(),
            });
        }
        // 🚨 **Every other kit held keeps the column it read** (`arch-scribe`'s
        // reading 1 of D1066's first cut). With no mode of its own the document
        // read each kit's **first**; the adopted first mode is now the default,
        // and a kit with a mode of that name elsewhere in its list — *Night,
        // Day* beside *Day, Night* — would read that one by name. So the new
        // default is pinned, by hand, to the first of each kit whose name match
        // differs: what it drew is what it draws.
        let default = modes.modes.first().map(|m| m.id);
        let adopted = fresh
            .first()
            .and_then(|v| t.get(*v))
            .and_then(Variable::kit);
        for (kit, held) in t.kits() {
            let (Some(d), Some(first)) = (default, held.modes.first()) else {
                continue;
            };
            if Some(kit.as_str()) == adopted || modes.kit_mode(kit, Some(d)) == Some(first.id) {
                continue;
            }
            let mut pinned = held.clone();
            pinned.map.insert(d, first.id);
            modes.kits.insert(kit.clone(), pinned.clone());
            out.ops.push(Operation::SetKitModes {
                kit: kit.clone(),
                modes: Some(pinned),
            });
        }
    }

    for v in &want {
        let o = t.get(*v).and_then(|x| x.origin.as_ref()).expect("a kit's");
        let to = forked_var(o).unwrap_or_else(|| ids.mint_var());
        out.vars.insert(*v, to);
    }
    let mut names: Vec<String> = t.local().map(|x| x.name.clone()).collect();
    for v in &fresh {
        let var = t.get(*v).expect("listed from the table");
        let kit = var.kit().expect("a kit's");
        let prefix = t
            .kits()
            .get(kit)
            .and_then(|k| k.name.as_deref())
            .unwrap_or("Kit");
        let name = free(&format!("{prefix}/{}", var.name), |n| {
            names.iter().any(|x| x == n)
        });
        names.push(name.clone());
        let cells = modes
            .modes
            .iter()
            .filter_map(|m| {
                let col = modes.kit_mode(kit, Some(m.id))?;
                let cell = match var.cells.get(&col)? {
                    Cell::Alias(a) => Cell::Alias(*out.vars.get(a).unwrap_or(a)),
                    c => c.clone(),
                };
                Some((m.id, cell))
            })
            .collect();
        out.ops.push(Operation::AddVariable {
            var: Variable {
                id: out.vars[v],
                name,
                kind: var.kind,
                cells,
                origin: None,
                forked: var.origin.clone(),
            },
        });
        out.made.0 += 1;
    }

    let remap = Plan {
        vars: out.vars.clone(),
        ..Plan::default()
    };
    let mut style_names: Vec<(crate::style::StyleSlot, String)> = doc
        .styles()
        .iter()
        .filter(|s| s.origin.is_none())
        .map(|s| (s.slot(), s.name.clone()))
        .collect();
    for s in &want_styles {
        let style = doc.styles().get(*s).expect("listed from the table");
        let o = style.origin.as_ref().expect("a kit's");
        let held = doc.styles().iter().find(|x| {
            x.origin.is_none()
                && x.forked
                    .as_ref()
                    .is_some_and(|f| (f.kit.as_str(), f.style) == (o.kit.as_str(), o.style))
        });
        if let Some(h) = held {
            out.styles.insert(*s, h.id);
            continue;
        }
        let to = ids.mint_style();
        out.styles.insert(*s, to);
        let prefix = t
            .kits()
            .get(&o.kit)
            .and_then(|k| k.name.as_deref())
            .unwrap_or("Kit");
        let slot = style.slot();
        let name = free(&format!("{prefix}/{}", style.name), |n| {
            style_names.iter().any(|(sl, x)| *sl == slot && x == n)
        });
        style_names.push((slot, name.clone()));
        out.ops.push(Operation::AddStyle {
            style: Style {
                id: to,
                name,
                value: remap.remap_value(&style.value),
                origin: None,
                forked: Some(o.clone()),
            },
        });
        out.made.1 += 1;
    }
    out
}

impl Fork {
    /// The ops that point `n`'s own bindings ([`own`]) at the forks — the
    /// bindings it shares with its link source `src` left for the follow.
    pub(crate) fn rebind_ops(&self, n: &Node, src: Option<&Node>) -> Vec<Operation> {
        let id = n.id;
        let mut ops = Vec::new();
        let to = |x: Option<VarId>, at_src: Option<VarId>| -> Option<VarId> {
            own(x, at_src).and_then(|v| self.vars.get(&v).copied())
        };
        let mut fills = n.paint.fills.clone();
        let mut wrote = false;
        for f in fills.iter_mut() {
            let at = src.and_then(|s| src_fill(s, f.id));
            if let BrushSource::Bound { var, .. } = &mut f.value.brush
                && let Some(v) = to(Some(*var), at)
            {
                *var = v;
                wrote = true;
            }
        }
        if wrote {
            ops.push(Operation::SetFills { id, fills });
        }
        let mut strokes = n.paint.strokes.clone();
        let mut wrote = false;
        for s in strokes.iter_mut() {
            let at = src.and_then(|x| src_stroke(x, s.id));
            if let BrushSource::Bound { var, .. } = &mut s.value.brush
                && let Some(v) = to(Some(*var), at)
            {
                *var = v;
                wrote = true;
            }
            let at = src.and_then(|x| src_stroke_width(x, s.id));
            if let Some(var) = s.value.width_var
                && let Some(v) = to(Some(var), at)
            {
                s.value.width_var = Some(v);
                wrote = true;
            }
        }
        if wrote {
            ops.push(Operation::SetStrokes { id, strokes });
        }
        // And each effect's numbers (§15 D1074), by item, in one list write.
        let mut effects = n.effects.clone();
        let mut wrote = false;
        for e in effects.iter_mut() {
            let at = src.and_then(|x| x.effects.iter().find(|k| k.id == e.id));
            for slot in 0..4 {
                if let Some(var) = e.vars[slot]
                    && let Some(v) = to(Some(var), at.and_then(|k| k.vars[slot]))
                {
                    e.vars[slot] = Some(v);
                    wrote = true;
                }
            }
        }
        if wrote {
            ops.push(Operation::SetEffects { id, effects });
        }
        fn slots<const N: usize>(
            a: [Option<VarId>; N],
            b: Option<[Option<VarId>; N]>,
            to: impl Fn(Option<VarId>, Option<VarId>) -> Option<VarId>,
        ) -> Option<[Option<VarId>; N]> {
            let mut out = a;
            let mut wrote = false;
            for i in 0..N {
                if let Some(v) = to(a[i], b.and_then(|b| b[i])) {
                    out[i] = Some(v);
                    wrote = true;
                }
            }
            wrote.then_some(out)
        }
        if let Some(vars) = slots(n.corner_vars, src.map(|s| s.corner_vars), to) {
            ops.push(Operation::SetCornerVars { id, vars });
        }
        if let Some(vars) = slots(n.spacing_vars, src.map(|s| s.spacing_vars), to) {
            ops.push(Operation::SetSpacingVars { id, vars });
        }
        if let Some(vars) = slots(n.size_vars, src.map(|s| s.size_vars), to) {
            ops.push(Operation::SetSizeVars { id, vars });
        }
        if let Some(vars) = slots(n.text_vars, src.map(|s| s.text_vars), to) {
            ops.push(Operation::SetTextVars { id, vars });
        }
        if let Some(v) = to(n.visible_var, src.and_then(|s| s.visible_var)) {
            ops.push(Operation::SetVisibleVar { id, var: Some(v) });
        }
        if let Some(v) = to(n.opacity_var, src.and_then(|s| s.opacity_var)) {
            ops.push(Operation::SetOpacityVar { id, var: Some(v) });
        }
        for (slot, s) in n.styles.iter() {
            if own(Some(s), src.and_then(|x| x.styles.get(slot))).is_some()
                && let Some(to) = self.styles.get(&s)
            {
                ops.push(crate::style::bind_op(id, slot, Some(*to)));
            }
        }
        ops
    }
}
