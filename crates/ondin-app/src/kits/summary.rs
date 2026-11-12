//! The kit update notice's extended summary (§15 D1038's *Kit updates*, D1065):
//! what accepting the waiting kit updates would change, read off the update
//! before it is taken — the components it rewrites, and the kits' variables and styles it
//! changes, adds and removes, each changed colour old → new and each changed
//! number old → new, and how many layers each removal detaches.
//!
//! **Read from the update's operations, never from a scratch copy it was
//! applied to**: an update is committed through the commit's passes
//! (`propagate::owed`), and its bare transaction is not one `Document::apply`
//! is bound to take. The new records are the `AddVariable`, `SetVariable`,
//! `AddStyle` and `SetStyleName` it carries; the old are the document's.

use eframe::egui;
use ondin_core::kit::Update;
use ondin_core::style::StyleSlot;
use ondin_core::variable::{Cell, KitModes, Value, VarId, VarKind, Variable};
use ondin_core::{Document, NodeId, Operation};
use std::collections::BTreeMap;

/// What a cached summary was read at: the document's revision, and each
/// waiting kit's id and published version.
pub(crate) type Key = (u64, Vec<(String, u32)>);

/// One variable or style a summary names.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Token {
    pub name: String,
    pub kind: TokenKind,
    /// What the document reads now, and what it will — `None` for a style, for
    /// an added one's old and a removed one's new.
    pub old: Option<Value>,
    pub new: Option<Value>,
    /// For a removed one: the layers of the document it detaches, values kept.
    pub detach: usize,
    /// For a removed one: the vault tops that read it, which keep it while
    /// they are held back (`kit::update_holding`).
    pub readers: Vec<NodeId>,
}

/// One component an update changes or removes — a row of the review, ticked
/// by its vault top.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Component {
    /// The vault top the row holds back when unticked: the main, or the set a
    /// removed variant belongs to.
    pub top: NodeId,
    pub name: String,
    /// The instances outside the vault the update reaches.
    pub instances: usize,
    /// Of those, the ones with overrides.
    pub overridden: usize,
    pub change: Change,
}

/// What an update does to a [`Component`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Change {
    /// Rewritten to the new publish.
    Changed,
    /// Otherwise as published, but reading a variable or style the publish
    /// removes, which it detaches from: the first one's name, and how many more.
    Unbinds(String, usize),
    /// No longer published: made a local main, its instances still following it.
    MadeLocal,
    /// A variant its set's update deletes, detaching this many instances.
    Detached(usize),
}

impl Change {
    /// Whether the component stays the kit's — a row of the review's *Changed*
    /// section rather than *Removed*.
    pub(crate) fn stays(&self) -> bool {
        matches!(self, Change::Changed | Change::Unbinds(..))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TokenKind {
    Var(VarKind),
    Style(StyleSlot),
}

/// One kit's part of the summary.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct KitSummary {
    pub kit: String,
    /// The components the update rewrites, then those it removes, in the
    /// vault's order, then those it only detaches from a removed variable or
    /// style.
    pub components: Vec<Component>,
    pub changed: Vec<Token>,
    pub added: Vec<Token>,
    pub removed: Vec<Token>,
}

impl KitSummary {
    /// The headline's counts: components, variables, styles.
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        let all = || self.changed.iter().chain(&self.added).chain(&self.removed);
        let vars = all()
            .filter(|t| matches!(t.kind, TokenKind::Var(_)))
            .count();
        let styles = all()
            .filter(|t| matches!(t.kind, TokenKind::Style(_)))
            .count();
        (self.tops().len(), vars, styles)
    }

    /// The vault tops the review ticks, each once.
    pub(crate) fn tops(&self) -> Vec<NodeId> {
        let mut out: Vec<NodeId> = Vec::new();
        for c in &self.components {
            if !out.contains(&c.top) {
                out.push(c.top);
            }
        }
        out
    }
}

/// The whole summary, a part per kit in `kits`' order.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Summary {
    pub kits: Vec<KitSummary>,
    /// `kit::Update::needs`: which top's new version reaches which.
    pub needs: Vec<(NodeId, NodeId)>,
}

impl Summary {
    /// The layers every removal detaches, and the one removed name when there
    /// is a single removal — the footer's *"4 layers using atlas/Label keep
    /// its values"*. A removal a held top in `off` keeps detaches nothing.
    pub(crate) fn detaching(
        &self,
        off: &std::collections::HashSet<NodeId>,
    ) -> (usize, Option<&str>) {
        let removed: Vec<&Token> = self
            .kits
            .iter()
            .flat_map(|k| &k.removed)
            .filter(|t| t.detach > 0 && !t.readers.iter().any(|r| off.contains(r)))
            .collect();
        let n = removed.iter().map(|t| t.detach).sum();
        let one = match removed[..] {
            [t] => Some(t.name.as_str()),
            _ => None,
        };
        (n, one)
    }
}

/// The value `var` reads in the document's default mode, `lookup` answering
/// any variable an alias names and `kits` the kit mode records to read by in
/// place of the document's — the update's own (`SetKitModes`), for a value
/// after it: a kit held for the first time has no record in the document, and
/// one whose modes changed is read in its new columns. Reading the document's
/// records alone would list a new kit's variables with no value.
fn read<'a>(
    doc: &Document,
    var: &'a Variable,
    lookup: &dyn Fn(VarId) -> Option<&'a Variable>,
    kits: &BTreeMap<String, KitModes>,
) -> Option<Value> {
    let t = doc.variables();
    let col = |v: &Variable| match v.kit() {
        Some(k) => match kits.get(k) {
            Some(km) => default_column(doc, km),
            None => t.kit_mode(k, None),
        },
        None => t.default_mode(),
    };
    let mut at = var;
    for _ in 0..=t.iter().count() + 8 {
        match at.cells.get(&col(at)?)? {
            Cell::Color(c) => return Some(Value::Color(*c)),
            Cell::Number(n) => return Some(Value::Number(*n)),
            Cell::Boolean(b) => return Some(Value::Boolean(*b)),
            Cell::String(s) => return Some(Value::String(s.clone())),
            Cell::Alias(next) => at = lookup(*next)?,
        }
    }
    None
}

/// The column of `km`'s kit read in the document's default mode — the
/// mapping's rule (`Variables::kit_mode`) over a record the document does not
/// hold yet: the hand-set entry, else the kit mode of the default's name, else
/// the kit's first.
fn default_column(doc: &Document, km: &KitModes) -> Option<ondin_core::ModeId> {
    let t = doc.variables();
    if let Some(m) = t.default_mode() {
        if let Some(to) = km.map.get(&m)
            && km.mode(*to).is_some()
        {
            return Some(*to);
        }
        if let Some(named) = t
            .mode(m)
            .and_then(|dm| km.modes.iter().find(|x| x.name == dm.name))
        {
            return Some(named.id);
        }
    }
    km.modes.first().map(|x| x.id)
}

/// The layers outside the vault a removal of `bound` detaches: every user
/// except a copy bound alike to its source, which follows its main instead
/// (`tokens::plan`'s *"except a copy that reads it only where its main
/// does"*) — the count is an estimate of the same rule, field by field
/// collapsed to *reads it at all*.
fn detached(doc: &Document, users: Vec<NodeId>, reads: impl Fn(NodeId) -> bool) -> usize {
    users
        .into_iter()
        .filter(|u| !doc.in_vault(*u))
        .filter(|u| {
            let src = doc.get(*u).and_then(|n| n.link());
            !src.is_some_and(&reads)
        })
        .count()
}

/// The vault top `id` sits under, itself included; `None` outside the vault.
fn vault_top(doc: &Document, id: NodeId) -> Option<NodeId> {
    let vault = doc.vault()?;
    let mut at = id;
    for _ in 0..=doc.len() {
        let p = doc.get(at)?.parent()?;
        if p == vault {
            return Some(at);
        }
        at = p;
    }
    None
}

/// The instances of `mains` outside the vault, and how many of them carry
/// overrides.
fn instances_outside(doc: &Document, mains: &[NodeId]) -> (usize, usize) {
    let all: Vec<NodeId> = mains
        .iter()
        .flat_map(|m| ondin_core::component::instances_of(doc, *m))
        .filter(|i| !doc.in_vault(*i))
        .collect();
    let drifted = all
        .iter()
        .filter(|i| ondin_core::reset::drift(doc, **i).any())
        .count();
    (all.len(), drifted)
}

/// The vault tops that keep a removed token while held: [`reader_tops`], or
/// none for one whose name the publish gives another (`Update::named_over`),
/// which is removed however its readers are held.
fn keepers(named_over: bool, doc: &Document, users: &[NodeId]) -> Vec<NodeId> {
    match named_over {
        true => Vec::new(),
        false => reader_tops(doc, users),
    }
}

/// The vault tops of `users`, each once.
fn reader_tops(doc: &Document, users: &[NodeId]) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = Vec::new();
    for t in users.iter().filter_map(|u| vault_top(doc, *u)) {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// The summary of `update` over `doc`. Values are read in the document's
/// default mode, as the browser's first column shows them.
pub(crate) fn summary(doc: &Document, update: &Update) -> Summary {
    let t = doc.variables();
    let mut new_vars: BTreeMap<VarId, &Variable> = BTreeMap::new();
    let mut new_styles: BTreeMap<ondin_core::style::StyleId, (String, StyleSlot)> = BTreeMap::new();
    let mut new_kits: BTreeMap<String, KitModes> = BTreeMap::new();
    for op in &update.tx.0 {
        match op {
            Operation::AddVariable { var } | Operation::SetVariable { var } => {
                new_vars.insert(var.id, var);
            }
            Operation::AddStyle { style } => {
                new_styles.insert(style.id, (style.name.clone(), style.slot()));
            }
            Operation::SetKitModes {
                kit,
                modes: Some(km),
            } => {
                new_kits.insert(kit.clone(), km.clone());
            }
            _ => {}
        }
    }
    let held_kits = BTreeMap::new();
    let lookup_new = |v: VarId| new_vars.get(&v).copied().or_else(|| t.get(v));
    let lookup_old = |v: VarId| t.get(v);

    let mut kits: Vec<KitSummary> = Vec::new();
    fn part(kits: &mut Vec<KitSummary>, kit: &str) -> usize {
        match kits.iter().position(|k| k.kit == kit) {
            Some(i) => i,
            None => {
                kits.push(KitSummary {
                    kit: kit.to_string(),
                    ..Default::default()
                });
                kits.len() - 1
            }
        }
    }

    let rows = update
        .changed
        .iter()
        .map(|m| (*m, *m, Change::Changed))
        .chain(
            update
                .made_local
                .iter()
                .map(|m| (*m, *m, Change::MadeLocal)),
        )
        .chain(update.removed.iter().filter_map(|m| {
            let n = instances_outside(doc, &[*m]).0;
            Some((vault_top(doc, *m)?, *m, Change::Detached(n)))
        }));
    for (top, m, change) in rows {
        let Some(o) = doc.get(top).and_then(|n| n.origin()) else {
            continue;
        };
        let mains = match doc.get(m) {
            Some(n) if n.set().is_some() => ondin_core::variant::variants(doc, m),
            _ => vec![m],
        };
        let (instances, overridden) = instances_outside(doc, &mains);
        let i = part(&mut kits, &o.kit);
        kits[i].components.push(Component {
            top,
            name: crate::private::layer(doc, m).into_owned(),
            instances,
            overridden,
            change,
        });
    }
    for v in &update.vars_changed {
        let Some(next) = new_vars.get(v) else {
            continue;
        };
        let Some(kit) = next.kit() else { continue };
        let new = read(doc, next, &lookup_new, &new_kits);
        let token = |old| Token {
            name: next.name.clone(),
            kind: TokenKind::Var(next.kind),
            old,
            new,
            detach: 0,
            readers: Vec::new(),
        };
        let i = part(&mut kits, kit);
        match t.get(*v) {
            Some(held) => kits[i]
                .changed
                .push(token(read(doc, held, &lookup_old, &held_kits))),
            None => kits[i].added.push(token(None)),
        }
    }
    for s in &update.styles_changed {
        let held = doc.styles().get(*s);
        let (name, slot) = match (new_styles.get(s), held) {
            (Some(n), _) => n.clone(),
            (None, Some(h)) => {
                // Renamed or revalued: the name the update leaves.
                let name = update
                    .tx
                    .0
                    .iter()
                    .rev()
                    .find_map(|op| match op {
                        Operation::SetStyleName { id, name } if id == s => Some(name.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| h.name.clone());
                (name, h.slot())
            }
            (None, None) => continue,
        };
        let kit = held.and_then(|h| h.kit()).map(str::to_string).or_else(|| {
            update.tx.0.iter().find_map(|op| match op {
                Operation::AddStyle { style } if style.id == *s => style.kit().map(str::to_string),
                _ => None,
            })
        });
        let Some(kit) = kit else { continue };
        let token = Token {
            name,
            kind: TokenKind::Style(slot),
            old: None,
            new: None,
            detach: 0,
            readers: Vec::new(),
        };
        let i = part(&mut kits, &kit);
        if held.is_some() {
            kits[i].changed.push(token);
        } else {
            kits[i].added.push(token);
        }
    }
    for v in &update.vars_removed {
        let Some(held) = t.get(*v) else { continue };
        let Some(kit) = held.kit() else { continue };
        let users = ondin_core::variable::users(doc, *v);
        let detach = detached(doc, users.clone(), |src| users.contains(&src));
        let i = part(&mut kits, kit);
        kits[i].removed.push(Token {
            name: held.name.clone(),
            kind: TokenKind::Var(held.kind),
            old: read(doc, held, &lookup_old, &held_kits),
            new: None,
            detach,
            readers: keepers(update.named_over.0.contains(v), doc, &users),
        });
    }
    for s in &update.styles_removed {
        let Some(held) = doc.styles().get(*s) else {
            continue;
        };
        let Some(kit) = held.kit() else { continue };
        let users = ondin_core::style::users(doc, *s);
        let detach = detached(doc, users.clone(), |src| users.contains(&src));
        let i = part(&mut kits, kit);
        kits[i].removed.push(Token {
            name: held.name.clone(),
            kind: TokenKind::Style(held.slot()),
            old: None,
            new: None,
            detach,
            readers: keepers(update.named_over.1.contains(s), doc, &users),
        });
    }
    // A main the sync leaves as it is still changes when it reads what the
    // publish removes: the removal detaches it, and holding it back keeps it.
    let mut unbinds: Vec<(NodeId, Vec<String>)> = Vec::new();
    for t in kits.iter().flat_map(|k| &k.removed) {
        for r in &t.readers {
            if !update.updated.contains(r) || kits.iter().any(|k| k.tops().contains(r)) {
                continue;
            }
            match unbinds.iter_mut().find(|(top, _)| top == r) {
                Some((_, names)) => names.push(t.name.clone()),
                None => unbinds.push((*r, vec![t.name.clone()])),
            }
        }
    }
    for (top, names) in unbinds {
        let Some(o) = doc.get(top).and_then(|n| n.origin()) else {
            continue;
        };
        let mains = match doc.get(top) {
            Some(n) if n.set().is_some() => ondin_core::variant::variants(doc, top),
            _ => vec![top],
        };
        let (instances, overridden) = instances_outside(doc, &mains);
        let i = part(&mut kits, &o.kit);
        kits[i].components.push(Component {
            top,
            name: crate::private::layer(doc, top).into_owned(),
            instances,
            overridden,
            change: Change::Unbinds(names[0].clone(), names.len() - 1),
        });
    }
    Summary {
        kits,
        needs: update.needs.clone(),
    }
}

/// How many variable and style rows a kit's table shows before *and N more*:
/// they are fixed, so listing a whole renamed palette would only be scrolled
/// past on the way to the components.
pub(crate) const MAX_ROWS: usize = 6;

/// The headline under a kit's name: *2 components · 4 variables · 1
/// style*, the empty kinds left out.
pub(crate) fn headline(k: &KitSummary) -> String {
    let (c, v, s) = k.counts();
    let part = |n: usize, one: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {one}s")),
    };
    [part(c, "component"), part(v, "variable"), part(s, "style")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A value as a row shows it: a colour as a chip, anything else as text.
fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::Color(_) => None,
        Value::Number(n) => Some(crate::panels::variables::number_text(*n)),
        Value::Boolean(b) => Some(if *b { "true" } else { "false" }.to_string()),
        Value::String(s) => Some(crate::panels::variables::string_text(s)),
    }
}

/// Draw one kit's review table under its row: the components it changes and
/// removes, each with its tick, then the variables and styles that come with
/// them, ticked and fixed. Answers what a click asked.
pub(crate) fn draw(ui: &mut egui::Ui, k: &KitSummary, review: &Review<'_>) -> Option<ReviewAct> {
    use crate::theme::{self, icon};
    use crate::ui::{RowMark, RowTail};
    let section = |ui: &mut egui::Ui, title: &str| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(crate::ui::eyebrow(title));
        });
    };
    let chip = |c: &ondin_core::peniko::Color| crate::ui::color_to_egui(*c);
    let token = |t: &Token| -> (RowMark, RowTail) {
        let lead = match (t.kind, t.old.as_ref().or(t.new.as_ref())) {
            (TokenKind::Var(_), Some(Value::Color(c))) => RowMark::Chip(chip(c)),
            (TokenKind::Var(VarKind::Number), _) => RowMark::Glyph(icon::HASH),
            (TokenKind::Var(VarKind::String), _) => RowMark::Glyph(icon::TEXT_T),
            (TokenKind::Var(VarKind::Boolean), _) => RowMark::Glyph(icon::TOGGLE),
            (TokenKind::Var(_), _) => RowMark::Glyph(icon::HASH),
            (TokenKind::Style(_), _) => RowMark::Glyph(icon::SWATCHES),
        };
        let tail = if t.detach > 0 {
            RowTail::Text(format!("{} detach", crate::ui::count(t.detach, "layer")))
        } else {
            match (t.kind, &t.old, &t.new) {
                (TokenKind::Style(_), ..) => RowTail::Text("style".into()),
                (_, Some(Value::Color(a)), Some(Value::Color(b))) => {
                    RowTail::Chips(chip(a), chip(b))
                }
                (_, Some(a), Some(b)) => RowTail::Text(format!(
                    "{} → {}",
                    value_text(a).unwrap_or_default(),
                    value_text(b).unwrap_or_default()
                )),
                (_, None, Some(v)) | (_, Some(v), None) => {
                    value_text(v).map_or(RowTail::None, RowTail::Text)
                }
                _ => RowTail::None,
            }
        };
        (lead, tail)
    };
    let w = ui.available_width();
    let mut act = None;
    // The components, each ticked by its vault top.
    let mut component = |ui: &mut egui::Ui, c: &Component| {
        let on = !review.off.contains(&c.top);
        let blocked = (review.why)(c.top);
        let tail = match (&blocked, &c.change) {
            (Some((tail, _)), _) => tail.clone(),
            (None, Change::Changed) => crate::ui::count(c.instances, "instance"),
            (None, Change::Unbinds(name, 0)) => format!("detaches {name}"),
            (None, Change::Unbinds(name, more)) => format!("detaches {name} +{more}"),
            (None, Change::MadeLocal) => "made local".into(),
            (None, Change::Detached(0)) => "removed".into(),
            (None, Change::Detached(n)) => format!("{} detach", crate::ui::count(*n, "instance")),
        };
        let tip = match blocked {
            Some((_, tip)) => tip,
            None => format!(
                "v{} → v{} · {} · {} with overrides",
                review.held,
                review.version,
                crate::ui::count(c.instances, "instance"),
                c.overridden
            ),
        };
        let resp = crate::ui::tick_row(
            ui,
            w,
            crate::ui::Tick::Open(on),
            crate::ui::RowMark::Glyph(icon::HEXAGON_FILL),
            &c.name,
            crate::ui::RowTail::Text(tail),
            Some(&tip),
        );
        if resp.clicked() {
            act = Some(ReviewAct::Toggle(c.top));
        }
        if resp.secondary_clicked()
            && let Some(at) = resp.interact_pointer_pos()
        {
            act = Some(ReviewAct::Menu(c.top, at));
        }
    };
    let changed: Vec<&Component> = k.components.iter().filter(|c| c.change.stays()).collect();
    let removed: Vec<&Component> = k.components.iter().filter(|c| !c.change.stays()).collect();
    for (title, rows) in [("Changed", changed), ("Removed", removed)] {
        if rows.is_empty() {
            continue;
        }
        section(ui, title);
        for c in rows {
            component(ui, c);
        }
    }
    // The variables and styles: fixed on, as the kit applies them whole.
    let tokens: Vec<&Token> = k.changed.iter().chain(&k.added).chain(&k.removed).collect();
    if !tokens.is_empty() {
        section(ui, "With any of these");
        let more = tokens.len().saturating_sub(MAX_ROWS);
        for t in tokens.into_iter().take(MAX_ROWS) {
            let (mark, tail) = token(t);
            let held = t.readers.iter().find(|r| review.off.contains(r));
            let tail = match held.and_then(|r| (review.name)(*r)) {
                Some(name) => crate::ui::RowTail::Text(format!("kept for {name}")),
                None => tail,
            };
            crate::ui::tick_row(
                ui,
                w,
                crate::ui::Tick::Fixed,
                mark,
                &t.name,
                tail,
                Some("Variables and styles come with any part of this update"),
            );
        }
        if more > 0 {
            ui.label(
                egui::RichText::new(format!("and {more} more"))
                    .size(crate::ui::SMALL_PT)
                    .color(theme::text::dim()),
            );
        }
    }
    act
}

/// What the review needs of the popover to draw a kit's table.
pub(crate) struct Review<'a> {
    /// The vault tops unticked.
    pub off: &'a std::collections::HashSet<NodeId>,
    /// For an unticked top whose update needs another unticked one: the row's
    /// tail and its tooltip.
    pub why: &'a dyn Fn(NodeId) -> Option<(String, String)>,
    /// A component row's name, by its top, across every kit.
    pub name: &'a dyn Fn(NodeId) -> Option<String>,
    /// The version the kit is held at, and the one it publishes.
    pub held: u32,
    pub version: u32,
}

/// What a click in a kit's table asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ReviewAct {
    /// A component's tick, by its top.
    Toggle(NodeId),
    /// A component's menu, opened at a point.
    Menu(NodeId, egui::Pos2),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::meta::{DocumentMeta, KitMark};
    use ondin_core::peniko::Color;
    use ondin_core::variable::Mode;
    use ondin_core::{BrushSource, Fill, IdSource, NodeKind, Transaction, keyed_by_position};

    const KIT: &str = "5eed00000000000000000000000000b2";
    const WHITE: Color = Color::from_rgba8(255, 255, 255, 255);
    const NAVY: Color = Color::from_rgba8(10, 20, 80, 255);
    const DUSK: Color = Color::from_rgba8(120, 60, 140, 255);

    fn bound(var: VarId) -> Vec<ondin_core::Keyed<Fill>> {
        keyed_by_position([Fill {
            brush: BrushSource::Bound { var, alpha: 1.0 },
            visible: true,
        }])
    }

    fn var(id: VarId, name: &str, day: ondin_core::ModeId, cell: Cell) -> Operation {
        Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: match cell {
                    Cell::Number(_) => VarKind::Number,
                    _ => VarKind::Color,
                },
                cells: [(day, cell)].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        }
    }

    /// **The summary names what the update changes**: v2 turns `brand`
    /// white → navy, adds `gap` (Dot's corner) and removes `old` — which a layer
    /// on the consumer's page is bound to, and Badge's instance reads only as
    /// its main does. So: both mains changed, `brand` old → new, `gap` added
    /// at 4, `old` removed detaching **one** layer — the page's, not the copy
    /// that follows its main.
    ///
    /// And v2 puts a new mode, *Dusk*, first: D, with no modes of its own,
    /// reads a kit's first, so `brand`'s new value is its Dusk cell — read by
    /// the update's own `SetKitModes` record, not the document's old one.
    ///
    /// Flip, run: `detached` counting every user outside the vault fails at
    /// *"the page's layer, not Badge's copy"*, 2; and `read` ignoring its
    /// `kits` fails at *"read in the kit's new first mode, Dusk"*, navy.
    #[test]
    fn the_summary_names_what_an_update_changes() {
        let mut ids = IdSource::new(0xBE);
        let root = ids.mint();
        let mut kit = ondin_core::Document::new(root);
        let [icon, dot, badge] = [(); 3].map(|_| ids.mint());
        let (day, dusk) = (ids.mint_mode(), ids.mint_mode());
        let [brand, old, gap] = [(); 3].map(|_| ids.mint_var());
        let frame = |id, parent, index, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(24.0, 24.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        };
        kit.apply(&Transaction(vec![
            Operation::AddMode {
                collection: ondin_core::variable::CollectionId::FIRST,
                mode: Mode {
                    id: day,
                    name: "Day".into(),
                },
                index: 0,
                cells: vec![],
            },
            var(brand, "brand", day, Cell::Color(WHITE)),
            var(old, "old", day, Cell::Color(NAVY)),
            frame(icon, root, 0, "Icon"),
            Operation::CreateNode {
                id: dot,
                parent: icon,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Dot".into()),
            },
            Operation::SetFills {
                id: dot,
                fills: bound(brand),
            },
            Operation::SetComponent {
                id: icon,
                component: true,
            },
            frame(badge, root, 1, "Badge"),
            Operation::SetFills {
                id: badge,
                fills: bound(old),
            },
            Operation::SetComponent {
                id: badge,
                component: true,
            },
        ]))
        .unwrap();
        kit.set_meta(DocumentMeta {
            id: Some(KIT.into()),
            name: Some("Atlas".into()),
            kit: Some(KitMark::default()),
            ..DocumentMeta::default()
        });
        ondin_core::kit::publish(&mut kit, None, 1);

        let mut d = ondin_core::Document::new(ids.mint());
        let droot = d.first_page();
        for main in [icon, badge] {
            let (tx, _) = ondin_core::kit::insert(
                &d,
                &mut ids,
                KIT,
                1,
                kit.published().unwrap(),
                main,
                droot,
            )
            .unwrap();
            d.apply(&tx).unwrap();
        }
        let held_old = d.variables().by_origin((KIT, old)).unwrap().id;
        let page = ids.mint();
        d.apply(&Transaction(vec![
            frame(page, droot, 0, "Page"),
            Operation::SetFills {
                id: page,
                fills: bound(held_old),
            },
        ]))
        .unwrap();

        // v2: brand navy, gap on Dot's corner, old gone.
        let plain = keyed_by_position([Fill {
            brush: WHITE.into(),
            visible: true,
        }]);
        kit.apply(&Transaction(vec![
            Operation::SetVariableCell {
                id: brand,
                mode: day,
                cell: Cell::Color(NAVY),
            },
            var(gap, "gap", day, Cell::Number(4.0)),
            Operation::SetCornerVars {
                id: dot,
                vars: [Some(gap), None, None, None],
            },
            Operation::SetFills {
                id: badge,
                fills: plain,
            },
            Operation::RemoveVariable { id: old },
            // A new first mode, which the update's own record reads in.
            Operation::AddMode {
                collection: ondin_core::variable::CollectionId::FIRST,
                mode: Mode {
                    id: dusk,
                    name: "Dusk".into(),
                },
                index: 0,
                cells: vec![(brand, Cell::Color(DUSK)), (gap, Cell::Number(4.0))],
            },
        ]))
        .unwrap();
        ondin_core::kit::publish(&mut kit, None, 2);
        let u = ondin_core::kit::update_all(
            &d,
            &mut ids,
            d.first_page(),
            &[(KIT, 2, kit.published().unwrap())],
        )
        .unwrap();
        let s = summary(&d, &u);

        let [k] = &s.kits[..] else {
            panic!("one kit: {s:?}")
        };
        let names: Vec<&str> = k.components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Icon", "Badge"]);
        let [changed] = &k.changed[..] else {
            panic!("brand changed: {:?}", k.changed)
        };
        assert_eq!(changed.name, "brand");
        assert_eq!(
            (changed.old.clone(), changed.new.clone()),
            (Some(Value::Color(WHITE)), Some(Value::Color(DUSK))),
            "read in the kit's new first mode, Dusk"
        );
        let [added] = &k.added[..] else {
            panic!("gap added: {:?}", k.added)
        };
        assert_eq!(
            (added.name.as_str(), added.new.clone()),
            ("gap", Some(Value::Number(4.0)))
        );
        let [removed] = &k.removed[..] else {
            panic!("old removed: {:?}", k.removed)
        };
        assert_eq!(removed.name, "old");
        assert_eq!(removed.detach, 1, "the page's layer, not Badge's copy");
        assert_eq!(headline(k), "2 components · 3 variables");
        let mut off = std::collections::HashSet::new();
        assert_eq!(s.detaching(&off), (1, Some("old")));
        // Badge, which reads `old`, held back keeps it: nothing detaches.
        // flip: `detaching` ignoring `off` → (1, Some("old")).
        let [badge] = removed.readers[..] else {
            panic!("the fixture: Badge reads old, {:?}", removed.readers)
        };
        off.insert(badge);
        assert_eq!(s.detaching(&off), (0, None), "kept for Badge");
    }

    /// **A main whose only change is a removed variable is a row**: v2 detaches
    /// Badge from `old` to the same navy and removes `old`, so the sync writes
    /// nothing, yet the removal detaches Badge here — a row the review can hold.
    ///
    /// flip: `summary` dropping its `unbinds` rows → fails at "Badge's row".
    #[test]
    fn a_main_reading_a_removed_variable_is_a_row() {
        let mut ids = IdSource::new(0xBF);
        let root = ids.mint();
        let mut kit = ondin_core::Document::new(root);
        let badge = ids.mint();
        let day = ids.mint_mode();
        let old = ids.mint_var();
        kit.apply(&Transaction(vec![
            Operation::AddMode {
                collection: ondin_core::variable::CollectionId::FIRST,
                mode: Mode {
                    id: day,
                    name: "Day".into(),
                },
                index: 0,
                cells: vec![],
            },
            var(old, "old", day, Cell::Color(NAVY)),
            Operation::CreateNode {
                id: badge,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(24.0, 24.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Badge".into()),
            },
            Operation::SetFills {
                id: badge,
                fills: bound(old),
            },
            Operation::SetComponent {
                id: badge,
                component: true,
            },
        ]))
        .unwrap();
        kit.set_meta(DocumentMeta {
            id: Some(KIT.into()),
            name: Some("Atlas".into()),
            kit: Some(KitMark::default()),
            ..DocumentMeta::default()
        });
        ondin_core::kit::publish(&mut kit, None, 1);
        let mut d = ondin_core::Document::new(ids.mint());
        let droot = d.first_page();
        let (tx, _) =
            ondin_core::kit::insert(&d, &mut ids, KIT, 1, kit.published().unwrap(), badge, droot)
                .unwrap();
        d.apply(&tx).unwrap();
        kit.apply(&Transaction(vec![
            Operation::SetFills {
                id: badge,
                fills: keyed_by_position([Fill {
                    brush: NAVY.into(),
                    visible: true,
                }]),
            },
            Operation::RemoveVariable { id: old },
        ]))
        .unwrap();
        ondin_core::kit::publish(&mut kit, None, 2);
        let v2 = [(KIT, 2, kit.published().unwrap())];
        let u = ondin_core::kit::update_all(&d, &mut ids, d.first_page(), &v2).unwrap();
        let [top] = u.updated[..] else {
            panic!("Badge synced: {:?}", u.updated)
        };
        assert!(u.changed.is_empty(), "the sync writes nothing to Badge");
        let s = summary(&d, &u);
        let [k] = &s.kits[..] else {
            panic!("one kit: {s:?}")
        };
        let [c] = &k.components[..] else {
            panic!("Badge's row: {:?}", k.components)
        };
        assert_eq!(
            (c.top, &c.change),
            (top, &Change::Unbinds("old".into(), 0)),
            "Badge detaches from old"
        );
        assert_eq!(headline(k), "1 component · 1 variable");
        // Its row held, `old` stays for it.
        let held =
            ondin_core::kit::update_holding(&d, &mut ids, d.first_page(), &v2, &[top]).unwrap();
        assert!(held.vars_removed.is_empty(), "old kept for Badge");

        // v3 brings a new variable named `old`: the held one can no longer be
        // kept, so the review neither offers Badge's row nor says "kept for".
        // flip: `keepers` ignoring `named_over` → fails at "kept by nothing".
        let new_old = ids.mint_var();
        kit.apply(&Transaction(vec![
            var(new_old, "old", day, Cell::Color(WHITE)),
            Operation::SetFills {
                id: badge,
                fills: bound(new_old),
            },
        ]))
        .unwrap();
        ondin_core::kit::publish(&mut kit, None, 3);
        let v3 = [(KIT, 3, kit.published().unwrap())];
        let u = ondin_core::kit::update_all(&d, &mut ids, d.first_page(), &v3).unwrap();
        assert_eq!(u.named_over.0.len(), 1, "the fixture: `old` named over");
        let s = summary(&d, &u);
        let [k] = &s.kits[..] else {
            panic!("one kit: {s:?}")
        };
        assert!(
            k.removed
                .iter()
                .any(|t| t.name == "old" && t.readers.is_empty()),
            "the old `old` removed, kept by nothing: {:?}",
            k.removed
        );
        assert!(
            !k.components
                .iter()
                .any(|c| matches!(c.change, Change::Unbinds(..))),
            "no detach row: holding Badge cannot keep it"
        );
        let held =
            ondin_core::kit::update_holding(&d, &mut ids, d.first_page(), &v3, &[top]).unwrap();
        assert!(!held.vars_removed.is_empty(), "removed while Badge is held");
    }
}
