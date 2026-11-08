//! *Clean up…*: take away what a design no longer shows — paint switched off or
//! buried under an opaque fill, hidden and empty layers, groups holding one layer,
//! and stray guides — as one transaction.
//!
//! Every rule takes only what draws nothing, or what draws exactly what replaces
//! it; whatever a rule finds but would change a pixel by taking is listed as
//! [`Kept`] with the reason. The rules run in [`Rule::ALL`]'s order on a copy of
//! the document, each seeing what the ones before it took, so a shape that loses
//! its last hidden fill is then empty, and a group that loses an empty sibling is
//! then a one-layer group.

use crate::build;
use crate::component;
use crate::guide::{GuideAxis, GuideId};
use crate::node::{BlendMode, NodeKind};
use crate::query::is_effectively_locked;
use crate::{Brush, Document, NodeId, Operation, Resolved, Transaction};
use std::collections::HashSet;

/// One kind of thing the verb takes, in the order it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    /// Fills, strokes, effects and layout grids whose eye is off.
    HiddenPaint,
    /// Fills under a visible, fully opaque solid fill in the same list.
    CoveredFills,
    /// Layers whose eye is off, with what they hold.
    HiddenLayers,
    /// Groups and frames with no layers, and layers that draw nothing.
    EmptyLayers,
    /// Groups holding one layer, which takes the group's place.
    OneLayerGroups,
    /// Ruler guides in scope.
    Guides,
}

impl Rule {
    /// Paint first, then layers, then groups, then guides: the order the run
    /// takes them in and the preview lists them in.
    pub const ALL: [Rule; 6] = [
        Rule::HiddenPaint,
        Rule::CoveredFills,
        Rule::HiddenLayers,
        Rule::EmptyLayers,
        Rule::OneLayerGroups,
        Rule::Guides,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Rule::HiddenPaint => "Hidden paint",
            Rule::CoveredFills => "Covered fills",
            Rule::HiddenLayers => "Hidden layers",
            Rule::EmptyLayers => "Empty layers",
            Rule::OneLayerGroups => "One-layer groups",
            Rule::Guides => "Guides",
        }
    }

    /// What the rule takes, for its switch's tooltip.
    pub fn hint(self) -> &'static str {
        match self {
            Rule::HiddenPaint => "Fills, strokes, effects and layout grids whose eye is off",
            Rule::CoveredFills => "Fills under a fully opaque solid fill, which never show",
            Rule::HiddenLayers => "Layers whose eye is off, with everything inside them",
            Rule::EmptyLayers => "Groups and frames with no layers, and layers that draw nothing",
            Rule::OneLayerGroups => "Groups holding one layer: the layer takes the group's place",
            Rule::Guides => "Ruler guides on the selected frames, the page or every page",
        }
    }

    /// How prefs spell it.
    pub fn key(self) -> &'static str {
        match self {
            Rule::HiddenPaint => "hidden_paint",
            Rule::CoveredFills => "covered_fills",
            Rule::HiddenLayers => "hidden_layers",
            Rule::EmptyLayers => "empty_layers",
            Rule::OneLayerGroups => "one_layer_groups",
            Rule::Guides => "guides",
        }
    }

    fn index(self) -> usize {
        Rule::ALL.iter().position(|r| *r == self).unwrap_or(0)
    }
}

/// Which rules run, and whether locked layers are left alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// One switch per [`Rule::ALL`] entry, in that order.
    pub on: [bool; 6],
    /// List a locked layer (and a guide while guides are locked) instead of
    /// cleaning it.
    pub skip_locked: bool,
    /// Whether the view has guides locked; read only with [`Self::skip_locked`].
    pub guides_locked: bool,
}

impl Default for Rules {
    /// Hidden layers and guides start off: people hide a layer or place a guide on
    /// purpose. The rest start on, and locked layers are skipped.
    fn default() -> Self {
        Self {
            on: [true, true, false, true, true, false],
            skip_locked: true,
            guides_locked: false,
        }
    }
}

impl Rules {
    pub fn is_on(&self, r: Rule) -> bool {
        self.on[r.index()]
    }

    pub fn set(&mut self, r: Rule, on: bool) {
        self.on[r.index()] = on;
    }

    /// These rules with every switch on, for counting what each would take.
    pub fn all_on(self) -> Self {
        Self {
            on: [true; 6],
            ..self
        }
    }
}

/// What the verb runs over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// These layers and everything under them; guides owned by any of them.
    Layers(Vec<NodeId>),
    /// Everything on these pages, guides included.
    Pages(Vec<NodeId>),
}

/// Why something a rule found is listed and left alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// A locked layer, or a guide while guides are locked, with skipping on.
    Locked,
    /// A main component, or a layer holding one: deleting it detaches this many
    /// instances.
    Main { instances: usize },
    /// An empty layer in an auto layout parent, which spaces its siblings.
    Spacer,
    /// A layer with an export.
    Export,
    /// A list a style supplies: the list is the style's.
    Style,
    /// A layer in a variant set with interaction states: a state may show it.
    State,
    /// A layer that masks the layers above it.
    Mask,
    /// A group whose opacity would be lost by ungrouping.
    Opacity,
    /// A group whose blend mode would be lost by ungrouping.
    Blend,
    /// A group whose effects would be lost by ungrouping.
    Effects,
    /// A group that lays out its layer.
    Layout,
    /// A layer in a main whose copy in an instance draws a different list, so
    /// what the main covers may show there.
    Instance,
}

impl Kept {
    /// The amber words a kept row carries.
    pub fn words(self) -> String {
        match self {
            Kept::Locked => "Locked · skipped".into(),
            Kept::Main { instances: 0 } => "Main component · kept".into(),
            Kept::Main { instances: 1 } => "Main · 1 instance would detach".into(),
            Kept::Main { instances } => format!("Main · {instances} instances would detach"),
            Kept::Spacer => "In an auto layout · kept".into(),
            Kept::Export => "Has an export · kept".into(),
            Kept::Style => "From a style · kept".into(),
            Kept::State => "Paired with a state · kept".into(),
            Kept::Mask => "Masks the layers above · kept".into(),
            Kept::Opacity => "Has opacity · kept".into(),
            Kept::Blend => "Has a blend mode · kept".into(),
            Kept::Effects => "Has effects · kept".into(),
            Kept::Layout => "Has a layout · kept".into(),
            Kept::Instance => "An instance shows it · kept".into(),
        }
    }
}

/// What a finding is about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Subject {
    Layer(NodeId),
    Guide {
        id: GuideId,
        axis: GuideAxis,
        position: f64,
        owner: NodeId,
    },
}

/// One row of the preview: what a rule found on one layer or guide.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub rule: Rule,
    pub subject: Subject,
    /// What it loses, in words (*Fill · Drop shadow*, *No layers*).
    pub what: String,
    /// How many items it loses: one per fill, stroke, effect or grid, one per
    /// layer, group or guide. Zero when kept.
    pub items: usize,
    pub kept: Option<Kept>,
}

/// The whole run: its rows, in rule order and then tree order, and the
/// transaction that does it.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub findings: Vec<Finding>,
    pub tx: Transaction,
}

impl Plan {
    /// Every item the run takes.
    pub fn items(&self) -> usize {
        self.findings.iter().map(|f| f.items).sum()
    }

    /// The items `rule` takes.
    pub fn items_of(&self, rule: Rule) -> usize {
        self.findings
            .iter()
            .filter(|f| f.rule == rule)
            .map(|f| f.items)
            .sum()
    }

    /// The distinct layers that lose something; guides are not layers.
    pub fn layers(&self) -> usize {
        let mut seen = HashSet::new();
        for f in self.findings.iter().filter(|f| f.kept.is_none()) {
            if let Subject::Layer(id) = f.subject {
                seen.insert(id);
            }
        }
        seen.len()
    }

    /// The rows listed and left alone.
    pub fn kept(&self) -> usize {
        self.findings.iter().filter(|f| f.kept.is_some()).count()
    }
}

/// Plan a clean-up of `scope` in `doc` with `rules`.
///
/// Read-only: the rules run on a copy, each transaction applied to it before the
/// next rule looks, and their operations concatenated into [`Plan::tx`], which is
/// therefore valid against `doc` as one transaction.
pub fn plan(doc: &Document, res: &Resolved, scope: &Scope, rules: &Rules) -> Plan {
    let mut run = Run {
        sim: doc.clone(),
        scope,
        rules,
        plan: Plan::default(),
    };
    if rules.is_on(Rule::HiddenPaint) {
        run.hidden_paint();
    }
    if rules.is_on(Rule::CoveredFills) {
        run.covered_fills();
    }
    if rules.is_on(Rule::HiddenLayers) {
        run.hidden_layers();
    }
    if rules.is_on(Rule::EmptyLayers) {
        run.empty_layers();
    }
    if rules.is_on(Rule::OneLayerGroups) {
        run.one_layer_groups(res);
    }
    if rules.is_on(Rule::Guides) {
        run.guides();
    }
    run.plan
}

struct Run<'a> {
    sim: Document,
    scope: &'a Scope,
    rules: &'a Rules,
    plan: Plan,
}

impl Run<'_> {
    /// The layers in scope now, in tree order, pages left out.
    fn layers(&self) -> Vec<NodeId> {
        let roots: Vec<NodeId> = match self.scope {
            Scope::Layers(ids) => ids
                .iter()
                .copied()
                .filter(|id| self.sim.contains(*id) && !self.sim.is_page(*id))
                .collect(),
            Scope::Pages(pages) => pages
                .iter()
                .filter_map(|p| self.sim.get(*p))
                .flat_map(|p| p.children().iter().copied())
                .collect(),
        };
        build::subtree_nodes(&self.sim, &roots)
            .into_iter()
            .filter(|id| !self.sim.is_page(*id))
            .collect()
    }

    /// Inside an instance and not its root: never touched.
    fn in_instance(&self, id: NodeId) -> bool {
        component::instance_root(&self.sim, id).is_some_and(|r| r != id)
    }

    fn instance_root(&self, id: NodeId) -> bool {
        component::instance_root(&self.sim, id) == Some(id)
    }

    fn locked(&self, id: NodeId) -> bool {
        self.rules.skip_locked && is_effectively_locked(&self.sim, id)
    }

    /// In a variant of a set with interaction states, where a state can show,
    /// hide or restructure a layer the rest variant holds.
    fn state_paired(&self, id: NodeId) -> bool {
        let mut at = Some(id);
        while let Some(n) = at.and_then(|i| self.sim.get(i)) {
            if let Some(set) = crate::variant::set_of(&self.sim, n.id())
                && self
                    .sim
                    .get(set)
                    .and_then(|s| s.set.as_ref())
                    .is_some_and(|s| s.interaction().is_some())
            {
                return true;
            }
            at = n.parent();
        }
        false
    }

    /// The instances deleting `id`'s subtree would detach, when it holds a main.
    fn mains_within(&self, id: NodeId) -> Option<usize> {
        let mains: Vec<NodeId> = build::subtree_nodes(&self.sim, &[id])
            .into_iter()
            .filter(|n| self.sim.get(*n).is_some_and(|n| n.component()))
            .collect();
        (!mains.is_empty()).then(|| {
            mains
                .iter()
                .map(|m| component::instances_of(&self.sim, *m).len())
                .sum()
        })
    }

    /// Whether `id`'s box is part of a laid-out item's: its parent lays out, or a
    /// group or boolean between it and such a parent takes its box from its
    /// layers, so taking `id` would move the item's siblings.
    fn in_layout(&self, id: NodeId) -> bool {
        let mut at = self.sim.get(id).and_then(|n| n.parent());
        while let Some(p) = at.and_then(|p| self.sim.get(p)) {
            if self.sim.resolved_display(p.id()).is_some() {
                return true;
            }
            if !matches!(p.kind(), NodeKind::Group | NodeKind::Boolean { .. }) {
                return false;
            }
            at = p.parent();
        }
        false
    }

    /// Whether `id` is a boolean's operand, or inside one: its outline draws into
    /// the boolean, whatever its own paint.
    fn in_boolean(&self, id: NodeId) -> bool {
        let mut at = self.sim.get(id).and_then(|n| n.parent());
        while let Some(p) = at.and_then(|p| self.sim.get(p)) {
            if matches!(p.kind(), NodeKind::Boolean { .. }) {
                return true;
            }
            at = p.parent();
        }
        false
    }

    /// Whether a copy of `id` in an instance, at any depth, draws a fill list
    /// other than `id`'s, so an item `id` covers may show there.
    fn copy_differs(&self, id: NodeId) -> bool {
        let Some(fills) = self.sim.get(id).map(|n| &n.paint().fills) else {
            return false;
        };
        let nodes = self.sim.node_map();
        let copies = |c: &crate::node::Node| {
            let mut link = c.link();
            while let Some(l) = link {
                if l == id {
                    return true;
                }
                link = nodes.get(&l).and_then(|n| n.link());
            }
            false
        };
        nodes
            .values()
            .filter(|c| copies(c))
            .any(|c| !crate::same_values(&c.paint().fills, fills))
    }

    fn find(&mut self, rule: Rule, id: NodeId, what: String, items: usize, kept: Option<Kept>) {
        self.plan.findings.push(Finding {
            rule,
            subject: Subject::Layer(id),
            what,
            items: if kept.is_some() { 0 } else { items },
            kept,
        });
    }

    /// Apply `tx` to the copy and keep its operations; refused ones are dropped
    /// whole, so the plan never carries a transaction the document would refuse.
    fn take(&mut self, tx: Transaction) -> bool {
        if tx.0.is_empty() {
            return true;
        }
        match self.sim.apply(&tx) {
            Ok(_) => {
                self.plan.tx.0.extend(tx.0);
                true
            }
            Err(_) => false,
        }
    }

    fn hidden_paint(&mut self) {
        let start = self.plan.findings.len();
        let mut ops = Vec::new();
        for id in self.layers() {
            if self.in_instance(id) || self.instance_root(id) {
                continue;
            }
            let Some(n) = self.sim.get(id) else { continue };
            let styles = n.styles();
            let p = n.paint();
            let hidden_fills = p.fills.iter().filter(|f| !f.value.visible).count();
            let hidden_strokes = p.strokes.iter().filter(|s| !s.value.visible).count();
            let hidden_effects: Vec<&'static str> = n
                .effects()
                .iter()
                .filter(|e| !e.value.visible)
                .map(|e| e.value.kind.label())
                .collect();
            let hidden_grids = n.grids().iter().filter(|g| !g.value.visible).count();
            // Split by whether a style supplies the list: those are listed, kept.
            let mut free = Vec::new();
            let mut styled = Vec::new();
            let mut sort = |by_style: bool, words: Vec<String>| {
                if by_style {
                    styled.extend(words);
                } else {
                    free.extend(words);
                }
            };
            sort(
                styles.fills.is_some(),
                counted(hidden_fills, "Fill", "fills"),
            );
            sort(
                styles.strokes.is_some(),
                counted(hidden_strokes, "Stroke", "strokes"),
            );
            sort(
                styles.effects.is_some(),
                hidden_effects.iter().map(|w| w.to_string()).collect(),
            );
            sort(false, counted(hidden_grids, "Layout grid", "layout grids"));
            let mut items = 0;
            if styles.fills.is_none() {
                items += hidden_fills;
            }
            if styles.strokes.is_none() {
                items += hidden_strokes;
            }
            if styles.effects.is_none() {
                items += hidden_effects.len();
            }
            items += hidden_grids;
            if items > 0 && self.locked(id) {
                self.find(
                    Rule::HiddenPaint,
                    id,
                    free.join(" · "),
                    0,
                    Some(Kept::Locked),
                );
            } else if items > 0 {
                if styles.fills.is_none() && hidden_fills > 0 {
                    let mut fills = p.fills.clone();
                    fills.retain(|f| f.value.visible);
                    ops.push(Operation::SetFills { id, fills });
                }
                if styles.strokes.is_none() && hidden_strokes > 0 {
                    let mut strokes = p.strokes.clone();
                    strokes.retain(|s| s.value.visible);
                    ops.push(Operation::SetStrokes { id, strokes });
                }
                if styles.effects.is_none() && !hidden_effects.is_empty() {
                    let mut effects = n.effects().to_vec();
                    effects.retain(|e| e.value.visible);
                    ops.push(Operation::SetEffects { id, effects });
                }
                if hidden_grids > 0 {
                    let mut grids = n.grids().to_vec();
                    grids.retain(|g| g.value.visible);
                    ops.push(Operation::SetLayoutGrids { id, grids });
                }
                self.find(Rule::HiddenPaint, id, free.join(" · "), items, None);
            }
            if !styled.is_empty() {
                self.find(
                    Rule::HiddenPaint,
                    id,
                    styled.join(" · "),
                    0,
                    Some(Kept::Style),
                );
            }
        }
        if !self.take(Transaction(ops)) {
            self.plan.findings.truncate(start);
        }
    }

    fn covered_fills(&mut self) {
        let start = self.plan.findings.len();
        let mut ops = Vec::new();
        for id in self.layers() {
            if self.in_instance(id) || self.instance_root(id) {
                continue;
            }
            let Some(n) = self.sim.get(id) else { continue };
            let fills = &n.paint().fills;
            // The topmost fill nothing can be seen through.
            let Some((top, colour)) =
                fills
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(i, f)| match f.value.brush.literal() {
                        Some(Brush::Solid(c)) if f.value.visible && c.components[3] >= 1.0 => {
                            Some((i, *c))
                        }
                        _ => None,
                    })
            else {
                continue;
            };
            if top == 0 {
                continue;
            }
            let what = match top {
                1 => format!("Fill under {}", hex(colour)),
                n => format!("{n} fills under {}", hex(colour)),
            };
            if n.styles().fills.is_some() {
                self.find(Rule::CoveredFills, id, what, 0, Some(Kept::Style));
            } else if self.locked(id) {
                self.find(Rule::CoveredFills, id, what, 0, Some(Kept::Locked));
            } else if self.copy_differs(id) {
                self.find(Rule::CoveredFills, id, what, 0, Some(Kept::Instance));
            } else {
                ops.push(Operation::SetFills {
                    id,
                    fills: fills[top..].to_vec(),
                });
                self.find(Rule::CoveredFills, id, what, top, None);
            }
        }
        if !self.take(Transaction(ops)) {
            self.plan.findings.truncate(start);
        }
    }

    fn hidden_layers(&mut self) {
        let hidden: Vec<NodeId> = self
            .layers()
            .into_iter()
            .filter(|id| {
                self.sim
                    .get(*id)
                    .is_some_and(|n| !n.visible() && n.visible_var().is_none())
            })
            .collect();
        let start = self.plan.findings.len();
        let mut gone = Vec::new();
        for id in build::outermost(&self.sim, &hidden) {
            if self.in_instance(id) {
                continue;
            }
            let within = build::subtree_nodes(&self.sim, &[id]);
            let kept = if self.locked(id)
                || self.rules.skip_locked
                    && within
                        .iter()
                        .any(|n| self.sim.get(*n).is_some_and(|n| n.locked()))
            {
                Some(Kept::Locked)
            } else if let Some(instances) = self.mains_within(id) {
                Some(Kept::Main { instances })
            } else if within
                .iter()
                .any(|n| build::exported_in_bulk(&self.sim, *n))
            {
                Some(Kept::Export)
            } else if self.state_paired(id) {
                Some(Kept::State)
            } else {
                None
            };
            if kept.is_none() {
                gone.push(id);
            }
            self.find(Rule::HiddenLayers, id, "Deleted".into(), 1, kept);
        }
        let tx = build::delete(&self.sim, &gone);
        if !self.take(tx) {
            self.plan.findings.truncate(start);
        }
    }

    /// Whether `id` draws nothing and holds nothing, and the words for it. A text
    /// is empty only with no content and no content binding, since one with
    /// content draws even unpainted (in black, or its runs' colours).
    fn emptiness(&self, id: NodeId) -> Option<&'static str> {
        let n = self.sim.get(id)?;
        let paints = n.paint().fills.iter().any(|f| f.value.visible)
            || n.paint().strokes.iter().any(|s| s.value.visible)
            || n.effects().iter().any(|e| e.value.visible);
        match n.kind() {
            NodeKind::Root | NodeKind::Slice { .. } => None,
            NodeKind::Group | NodeKind::Boolean { .. } => {
                n.children().is_empty().then_some("No layers")
            }
            NodeKind::Artboard { .. } => {
                (n.children().is_empty() && !paints).then_some("No layers")
            }
            NodeKind::Text { content, .. } => (content.is_empty()
                && n.text_vars()[crate::variable::TEXT_CONTENT].is_none())
            .then_some("No text"),
            _ if self.in_boolean(id) => None,
            _ => (!paints).then_some("Nothing drawn"),
        }
    }

    fn empty_layers(&mut self) {
        let mut listed: HashSet<NodeId> = HashSet::new();
        // Until nothing new empties: a group whose only layer goes is empty next.
        loop {
            let mut gone = Vec::new();
            let mut found = Vec::new();
            for id in self.layers() {
                if listed.contains(&id) || self.in_instance(id) || self.instance_root(id) {
                    continue;
                }
                let Some(what) = self.emptiness(id) else {
                    continue;
                };
                let n = self.sim.get(id).expect("listed above");
                let kept = if self.locked(id) {
                    Some(Kept::Locked)
                } else if n.mask() {
                    Some(Kept::Mask)
                } else if !n.exports().is_empty() {
                    Some(Kept::Export)
                } else if n.component() {
                    Some(Kept::Main {
                        instances: component::instances_of(&self.sim, id).len(),
                    })
                } else if self.in_layout(id) {
                    Some(Kept::Spacer)
                } else if self.state_paired(id) {
                    Some(Kept::State)
                } else {
                    None
                };
                listed.insert(id);
                if kept.is_none() {
                    gone.push(id);
                }
                found.push((id, what, kept));
            }
            let outer = build::outermost(&self.sim, &gone);
            let start = self.plan.findings.len();
            for (id, what, kept) in found {
                // A layer inside another empty one goes with it, under its row.
                if kept.is_none() && !outer.contains(&id) {
                    continue;
                }
                self.find(Rule::EmptyLayers, id, what.into(), 1, kept);
            }
            if outer.is_empty() {
                break;
            }
            let tx = build::delete(&self.sim, &outer);
            if !self.take(tx) {
                self.plan.findings.truncate(start);
                break;
            }
        }
    }

    /// Why a one-layer group cannot go without changing what it draws or exports.
    fn group_kept(&self, id: NodeId) -> Option<Kept> {
        let n = self.sim.get(id)?;
        if self.locked(id) {
            Some(Kept::Locked)
        } else if n.opacity() != 1.0 || n.opacity_var().is_some() {
            Some(Kept::Opacity)
        } else if n.blend() != BlendMode::default_for(n.kind()) {
            Some(Kept::Blend)
        } else if !n.effects().is_empty() {
            Some(Kept::Effects)
        } else if n.mask()
            || n.children()
                .iter()
                .any(|c| self.sim.get(*c).is_some_and(|c| c.mask()))
        {
            // A lone mask ungrouped would clip the layers above the group.
            Some(Kept::Mask)
        } else if !n.exports().is_empty() {
            Some(Kept::Export)
        } else if self.sim.resolved_display(id).is_some() {
            Some(Kept::Layout)
        } else if self.in_layout(id) {
            Some(Kept::Spacer)
        } else if self.state_paired(id) {
            Some(Kept::State)
        } else {
            None
        }
    }

    fn one_layer_groups(&mut self, res: &Resolved) {
        let mut resolved: Option<Resolved> = None;
        for id in self.layers() {
            let Some(n) = self.sim.get(id) else { continue };
            let one = matches!(n.kind(), NodeKind::Group)
                && n.children().len() == 1
                && n.visible()
                && n.visible_var().is_none()
                && !n.component()
                && n.variant().is_empty();
            if !one || self.in_instance(id) || self.instance_root(id) {
                continue;
            }
            let child = n.children()[0];
            let stays = self
                .sim
                .get(child)
                .map_or_else(String::new, |c| c.name().to_string());
            let what = format!("Ungrouped · {stays} stays");
            if let Some(kept) = self.group_kept(id) {
                self.find(Rule::OneLayerGroups, id, what, 0, Some(kept));
                continue;
            }
            // The plan's own document has moved on from `res` once anything was
            // taken, so it is resolved afresh then.
            let r = resolved.get_or_insert_with(|| match self.plan.tx.0.is_empty() {
                true => res.clone(),
                false => Resolved::rebuild(&self.sim),
            });
            let Ok(tx) = build::ungroup(&self.sim, r, id) else {
                continue;
            };
            let Ok(outcome) = self.sim.apply(&tx) else {
                continue;
            };
            r.update(&self.sim, &outcome.dirty);
            self.plan.tx.0.extend(tx.0);
            self.find(Rule::OneLayerGroups, id, what, 1, None);
        }
    }

    fn guides(&mut self) {
        let owners: HashSet<NodeId> = match self.scope {
            Scope::Layers(_) => self.layers().into_iter().collect(),
            Scope::Pages(pages) => pages.iter().copied().collect(),
        };
        let in_scope = |owner: NodeId| match self.scope {
            Scope::Layers(_) => owners.contains(&owner),
            Scope::Pages(_) => {
                owners.contains(&owner)
                    || self.sim.page_of(owner).is_some_and(|p| owners.contains(&p))
            }
        };
        let found: Vec<_> = self
            .sim
            .guides()
            .iter()
            .filter(|g| in_scope(g.owner))
            .map(|g| (g.id, g.axis, g.position, g.owner))
            .collect();
        let locked = self.rules.skip_locked && self.rules.guides_locked;
        let mut ops = Vec::new();
        let start = self.plan.findings.len();
        for (id, axis, position, owner) in found {
            if !locked {
                ops.push(Operation::RemoveGuide { id });
            }
            self.plan.findings.push(Finding {
                rule: Rule::Guides,
                subject: Subject::Guide {
                    id,
                    axis,
                    position,
                    owner,
                },
                what: "Removed".into(),
                items: usize::from(!locked),
                kept: locked.then_some(Kept::Locked),
            });
        }
        if !self.take(Transaction(ops)) {
            self.plan.findings.truncate(start);
        }
    }
}

/// `n` things in words, one per kind: *Fill*, *2 fills*; nothing for zero.
fn counted(n: usize, one: &str, many: &str) -> Vec<String> {
    match n {
        0 => Vec::new(),
        1 => vec![one.to_string()],
        n => vec![format!("{n} {many}")],
    }
}

fn hex(c: peniko::Color) -> String {
    let [r, g, b, _] = c.to_rgba8().to_u8_array();
    format!("#{r:02X}{g:02X}{b:02X}")
}
