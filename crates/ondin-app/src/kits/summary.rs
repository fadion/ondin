//! 5A's extended summary (G21, D1038's *Kit updates*, built in §15 D1065's
//! second amendment): what accepting
//! the waiting kit updates would change, read off the update before it is
//! taken — the components it rewrites, and the kits' variables and styles it
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
    /// The components the update rewrites, by name, in the vault's order.
    pub components: Vec<String>,
    pub changed: Vec<Token>,
    pub added: Vec<Token>,
    pub removed: Vec<Token>,
}

impl KitSummary {
    /// The headline's counts (G21): components, variables, styles.
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        let all = || self.changed.iter().chain(&self.added).chain(&self.removed);
        let vars = all()
            .filter(|t| matches!(t.kind, TokenKind::Var(_)))
            .count();
        let styles = all()
            .filter(|t| matches!(t.kind, TokenKind::Style(_)))
            .count();
        (self.components.len(), vars, styles)
    }
}

/// The whole summary, a part per kit in `kits`' order.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Summary {
    pub kits: Vec<KitSummary>,
}

impl Summary {
    /// The layers every removal detaches, and the one removed name when there
    /// is a single removal — the footer's *"4 layers using atlas/Label keep
    /// its values"*.
    pub(crate) fn detaching(&self) -> (usize, Option<&str>) {
        let removed: Vec<&Token> = self
            .kits
            .iter()
            .flat_map(|k| &k.removed)
            .filter(|t| t.detach > 0)
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
/// one whose modes changed is read in its new columns (`arch-scribe`'s reading
/// (c) of the first cut, which listed a new kit's variables with no value).
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

    for m in &update.changed {
        if let Some(n) = doc.get(*m)
            && let Some(o) = n.origin()
        {
            let i = part(&mut kits, &o.kit);
            kits[i].components.push(n.name().to_string());
        }
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
        });
    }
    Summary { kits }
}

/// How many rows a section shows before *and N more*: the popover stands under
/// the top bar, and a kit that renamed its whole palette must not push its
/// buttons off the window.
const MAX_ROWS: usize = 6;

/// A row's height.
const ROW_H: f32 = 20.0;

/// The headline under a kit's name (G21): *2 components · 4 variables · 1
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

/// Draw one kit's sections — *Changed*, *Added*, *Removed* — under its row.
pub(crate) fn draw(ui: &mut egui::Ui, k: &KitSummary) {
    use crate::theme::{self, icon};
    let w = ui.available_width();
    let section = |ui: &mut egui::Ui, title: &str, rows: Vec<Row<'_>>| {
        if rows.is_empty() {
            return;
        }
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(title)
                .size(10.5)
                .color(theme::text::FAINT),
        );
        let more = rows.len().saturating_sub(MAX_ROWS);
        for r in rows.into_iter().take(MAX_ROWS) {
            row(ui, w, r);
        }
        if more > 0 {
            ui.label(
                egui::RichText::new(format!("and {more} more"))
                    .size(11.0)
                    .color(theme::text::DIM),
            );
        }
    };
    fn token(t: &Token) -> Row<'_> {
        let lead = match (t.kind, t.old.as_ref().or(t.new.as_ref())) {
            (TokenKind::Var(_), Some(Value::Color(c))) => Lead::Chip(*c),
            (TokenKind::Var(VarKind::Number), _) => Lead::Glyph(icon::HASH),
            (TokenKind::Var(VarKind::String), _) => Lead::Glyph(icon::TEXT_T),
            (TokenKind::Var(_), _) => Lead::Glyph(icon::HASH),
            (TokenKind::Style(_), _) => Lead::Glyph(icon::SWATCHES),
        };
        let tail = if t.detach > 0 {
            Tail::Text(format!(
                "{} layer{} detach",
                t.detach,
                if t.detach == 1 { "" } else { "s" }
            ))
        } else {
            match (t.kind, &t.old, &t.new) {
                (TokenKind::Style(_), ..) => Tail::Text("style".into()),
                (_, Some(Value::Color(a)), Some(Value::Color(b))) => Tail::Chips(*a, *b),
                (_, Some(a), Some(b)) => Tail::Text(format!(
                    "{} → {}",
                    value_text(a).unwrap_or_default(),
                    value_text(b).unwrap_or_default()
                )),
                (_, None, Some(v)) | (_, Some(v), None) => {
                    value_text(v).map_or(Tail::None, Tail::Text)
                }
                _ => Tail::None,
            }
        };
        Row {
            lead,
            name: &t.name,
            tail,
        }
    }
    let mut changed: Vec<Row<'_>> = k
        .components
        .iter()
        .map(|c| Row {
            lead: Lead::Glyph(icon::HEXAGON),
            name: c,
            tail: Tail::Text("component".into()),
        })
        .collect();
    changed.extend(k.changed.iter().map(token));
    section(ui, "Changed", changed);
    section(ui, "Added", k.added.iter().map(token).collect());
    section(ui, "Removed", k.removed.iter().map(token).collect());
}

enum Lead {
    Chip(ondin_core::peniko::Color),
    Glyph(&'static str),
}

enum Tail {
    None,
    Text(String),
    Chips(ondin_core::peniko::Color, ondin_core::peniko::Color),
}

struct Row<'a> {
    lead: Lead,
    name: &'a str,
    tail: Tail,
}

/// One row: its chip or glyph, its name, and on the right what changes.
fn row(ui: &mut egui::Ui, w: f32, r: Row<'_>) {
    use crate::theme::{self, color, icon};
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, ROW_H), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    let chip = |x: f32, c| {
        crate::panels::variables::swatch(
            p,
            egui::Rect::from_center_size(egui::pos2(x + 5.0, y), egui::vec2(10.0, 10.0)),
            c,
        );
    };
    match r.lead {
        Lead::Chip(c) => chip(rect.left() + 1.0, c),
        Lead::Glyph(g) => {
            p.text(
                egui::pos2(rect.left(), y),
                egui::Align2::LEFT_CENTER,
                g,
                theme::icon_font(11.0),
                theme::text::DIM,
            );
        }
    }
    // The tail first, so the name is cut short of it rather than under it.
    let right = rect.right() - 2.0;
    let tail_left = match r.tail {
        Tail::None => right,
        Tail::Text(t) => p
            .text(
                egui::pos2(right, y),
                egui::Align2::RIGHT_CENTER,
                t,
                egui::FontId::proportional(10.5),
                theme::text::FAINT,
            )
            .left(),
        Tail::Chips(a, b) => {
            chip(right - 10.0, b);
            let arrow = p.text(
                egui::pos2(right - 14.0, y),
                egui::Align2::RIGHT_CENTER,
                icon::ARROW_RIGHT,
                theme::icon_font(10.0),
                theme::text::FAINT,
            );
            chip(arrow.left() - 14.0, a);
            arrow.left() - 14.0
        }
    };
    let name_rect =
        egui::Rect::from_x_y_ranges(rect.left() + 18.0..=tail_left - 8.0, rect.y_range());
    p.with_clip_rect(name_rect).text(
        egui::pos2(name_rect.left(), y),
        egui::Align2::LEFT_CENTER,
        r.name,
        egui::FontId::proportional(11.5),
        color::TEXT,
    );
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
                origin: None,
                forked: None,
                description: String::new(),
            },
        }
    }

    /// **The summary names what the update changes** (G21): v2 turns `brand`
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
            },
            transform: None,
            name: Some(name.into()),
        };
        kit.apply(&Transaction(vec![
            Operation::AddMode {
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
        assert_eq!(k.components, ["Icon", "Badge"]);
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
        assert_eq!(s.detaching(), (1, Some("old")));
    }
}
