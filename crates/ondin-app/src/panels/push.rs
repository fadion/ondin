//! **Push to main** in the app (§5.3d, `ondin_core::push`): the doors that write
//! an instance's differences into its main — the Component card's ⋯ rows, the
//! canvas menu's *Push all to main*, a local layer's *Push to ‹main›*, and the
//! *Push … to main* row beside every reset of one thing — and the toast each
//! commit leaves, counting the instances that took it and the ones that kept
//! their own.

use crate::app::OndinApp;
use crate::theme::icon;
use eframe::egui;
use ondin_core::push::{self, Blocked, Kind, LayerBlocked, Outcome, Pushable};
use ondin_core::{ItemId, NodeId, Operation, Transaction};

/// The pushes' readings, as of the session revision they were read at: each is a
/// walk of an instance, and the card is drawn every frame.
#[derive(Default)]
pub(crate) struct PushCache {
    rev: Option<u64>,
    pushable: std::collections::HashMap<NodeId, Pushable>,
    /// A row's tooltip, per root and push, read on hover only: it commits the
    /// push on a scratch document to count what follows.
    tips: std::collections::HashMap<(NodeId, KindKey), String>,
}

/// `Kind` as a map key.
type KindKey = u8;

fn key(kind: Kind) -> KindKey {
    match kind {
        Kind::All => 0,
        Kind::Properties => 1,
        Kind::Fields => 2,
        Kind::Layers => 3,
        Kind::Order => 4,
        Kind::Removals => 5,
    }
}

impl PushCache {
    fn at(&mut self, rev: u64) -> &mut Self {
        if self.rev != Some(rev) {
            *self = Self {
                rev: Some(rev),
                ..Self::default()
            };
        }
        self
    }
}

/// What a push wrote, for its toast.
pub(crate) enum PushWhat {
    All,
    Properties(usize),
    Fields(usize),
    Layers(Vec<String>),
    Order,
    Removals(Vec<String>),
}

/// `“Badge”`, or *2 layers* for several.
fn named(names: &[String], noun: &str) -> String {
    match names {
        [one] => format!("“{one}”"),
        _ => crate::ui::count(names.len(), noun),
    }
}

/// The toast a push leaves (§5.3d, the words of R4): what went into which main,
/// how many instances took it, and how many kept their own — the surprise a push
/// can cause, said out loud.
pub(crate) fn toast(what: &PushWhat, main: &str, out: Outcome) -> String {
    let follow = |n: usize| match n {
        1 => "1 instance follows".to_string(),
        n => format!("{n} instances follow"),
    };
    let keep = |n: usize| match n {
        1 => "1 keeps its own".to_string(),
        n => format!("{n} keep their own"),
    };
    let mut parts: Vec<String> = Vec::new();
    match what {
        PushWhat::Layers(names) => {
            parts.push(format!("Pushed {} into {main}", named(names, "layer")));
            let n = out.follow + out.kept;
            parts.push(match n {
                1 => "1 other instance now has it".to_string(),
                n => format!("{n} other instances now have it"),
            });
        }
        PushWhat::Removals(names) => {
            parts.push(format!("Deleted {} from {main}", named(names, "layer")));
            parts.push(match out.follow {
                1 => "1 instance lost it".to_string(),
                n => format!("{n} instances lost it"),
            });
            if out.kept > 0 {
                parts.push(format!("{} with edits keep it, unlinked", out.kept));
            }
        }
        _ => {
            parts.push(match what {
                PushWhat::Properties(n) => {
                    format!("Pushed {} into {main}", crate::ui::count(*n, "property"))
                }
                PushWhat::Fields(n) => {
                    format!("Pushed {} into {main}", crate::ui::count(*n, "field"))
                }
                PushWhat::Order => format!("Pushed the order into {main}"),
                _ => format!("Pushed this instance into {main}"),
            });
            parts.push(follow(out.follow));
            if out.kept > 0 {
                parts.push(keep(out.kept));
            }
        }
    }
    parts.push("Ctrl+Z to undo".into());
    parts.join(" · ")
}

impl OndinApp {
    /// [`push::pushable`] at instance `root`, from the cache while the document is
    /// unchanged.
    pub(crate) fn pushable_of(&mut self, root: NodeId) -> Pushable {
        let rev = self.session.revision();
        let doc = &self.session.doc;
        *self
            .push_cache
            .at(rev)
            .pushable
            .entry(root)
            .or_insert_with(|| push::pushable(doc, root))
    }

    /// A push row's tooltip (R1): what push `kind` from `root` writes into which
    /// main, and what it would do to the main's other instances — read while the
    /// row is hovered and kept for the revision.
    pub(crate) fn push_tip(&mut self, root: NodeId, kind: Kind) -> String {
        let rev = self.session.revision();
        if let Some(t) = self.push_cache.at(rev).tips.get(&(root, key(kind))) {
            return t.clone();
        }
        let main = self.push_main_name(root);
        let actor = self.session.ids.actor();
        let doc = &self.session.doc;
        let mut ids = ondin_core::IdSource::new(actor ^ 0x5157);
        let ops = push::push(doc, kind, root, &mut ids);
        let out = push::predicted(doc, root, &ops, actor);
        let others = |n: usize| match n {
            1 => "1 other instance follows".to_string(),
            n => format!("{n} other instances follow"),
        };
        let kept = match out.kept {
            0 => String::new(),
            1 => ", 1 keeps its own".to_string(),
            n => format!(", {n} keep their own"),
        };
        let what = match kind {
            Kind::All => "properties, fields, layers and order",
            Kind::Properties => "properties",
            Kind::Fields => "fields",
            Kind::Layers => "local layers",
            Kind::Order => "order",
            Kind::Removals => "",
        };
        let text = match kind {
            Kind::Removals => {
                let gone: Vec<String> = ops
                    .iter()
                    .filter_map(|op| match op {
                        Operation::DeleteNode { id } => {
                            Some(crate::private::layer(doc, *id).into_owned())
                        }
                        _ => None,
                    })
                    .collect();
                let lose = match out.follow {
                    1 => "1 instance loses it".to_string(),
                    n => format!("{n} instances lose it"),
                };
                let keep = match out.kept {
                    0 => String::new(),
                    n => format!(", {n} with edits keep it, unlinked"),
                };
                format!(
                    "Delete {} from {main} — {lose}{keep}",
                    named(&gone, "layer").replace(['“', '”'], "")
                )
            }
            _ => format!(
                "Write this instance's {what} into {main} — {}{kept}",
                others(out.follow)
            ),
        };
        self.push_cache.tips.insert((root, key(kind)), text.clone());
        text
    }

    /// The name of the main a push from `root` writes into.
    pub(crate) fn push_main_name(&self, root: NodeId) -> String {
        let doc = &self.session.doc;
        push::home(doc, root)
            .map(|m| crate::private::layer(doc, m).into_owned())
            .unwrap_or_default()
    }

    /// Why the instance at `root` cannot push, in the card's words, or `None`.
    pub(crate) fn push_refusal(&self, root: NodeId) -> Option<String> {
        let doc = &self.session.doc;
        let main = push::home(doc, root)?;
        let name = self.push_main_name(root);
        match push::blocked(doc, root)? {
            Blocked::Kit => Some(match self.kit_line(main) {
                Some(k) => format!(
                    "{name} comes from the {} kit — change it in the kit",
                    k.kit_name
                ),
                None => format!("{name} comes from a kit — change it in the kit"),
            }),
            Blocked::Locked => Some(format!("{name} is locked — unlock it to push into it")),
        }
    }

    /// Why the local layer `layer` cannot be pushed, in the card's words, or
    /// `None` when it can.
    pub(crate) fn layer_push_refusal(&self, layer: NodeId) -> Option<String> {
        let doc = &self.session.doc;
        let root = doc
            .get(layer)?
            .parent()
            .and_then(|p| ondin_core::component::instance_root(doc, p))?;
        if let Some(why) = self.push_refusal(root) {
            return Some(why);
        }
        let main = self.push_main_name(root);
        let own = crate::private::layer(doc, layer).into_owned();
        Some(match push::layer_blocked(doc, layer)? {
            LayerBlocked::WrapsLinked => {
                format!("This group holds {main}'s own layers — push them one at a time")
            }
            LayerBlocked::HoldsMain => {
                format!("{own} holds a {main} — a component cannot contain itself")
            }
        })
    }

    /// The one instance root a push from the selection writes from: the selected
    /// layer's own instance root. `None` (having said why) for anything else.
    fn push_root(&mut self) -> Option<NodeId> {
        let doc = &self.session.doc;
        let roots: Vec<NodeId> = ondin_core::build::outermost(doc, self.session.selection.ids())
            .into_iter()
            .filter_map(|id| ondin_core::component::instance_root(doc, id))
            .collect();
        match roots.as_slice() {
            [one] => Some(*one),
            [] => {
                self.session.info("Select an instance to push");
                None
            }
            _ => {
                self.session.info("Select one instance to push");
                None
            }
        }
    }

    /// Push `kind` from the selected instance, as one undo step.
    pub(crate) fn push_selection(&mut self, kind: Kind) {
        let Some(root) = self.push_root() else {
            return;
        };
        if let Some(why) = self.push_refusal(root) {
            self.session.info(why);
            return;
        }
        let p = self.pushable_of(root);
        let doc = &self.session.doc;
        let names = |ids: Vec<NodeId>| -> Vec<String> {
            ids.into_iter()
                .filter(|id| doc.get(*id).is_some())
                .map(|id| crate::private::layer(doc, id).into_owned())
                .collect()
        };
        let ops = push::push(doc, kind, root, &mut self.session.ids);
        let what = match kind {
            Kind::All => PushWhat::All,
            Kind::Properties => PushWhat::Properties(p.properties),
            Kind::Fields => PushWhat::Fields(p.fields),
            Kind::Order => PushWhat::Order,
            // Named by the instance's own layers, each linked in the same push
            // to the new main layer its subtree's first node is.
            Kind::Layers => PushWhat::Layers(names(
                ops.iter()
                    .filter_map(|op| match op {
                        Operation::InsertSubtree { nodes, .. } => nodes.first().map(|n| n.id()),
                        _ => None,
                    })
                    .filter_map(|new| {
                        ops.iter().find_map(|op| match op {
                            Operation::SetLink { id, link } if *link == Some(new) => Some(*id),
                            _ => None,
                        })
                    })
                    .collect(),
            )),
            Kind::Removals => PushWhat::Removals(names(
                ops.iter()
                    .filter_map(|op| match op {
                        Operation::DeleteNode { id } => Some(*id),
                        _ => None,
                    })
                    .collect(),
            )),
        };
        self.commit_push(root, ops, what);
    }

    /// Push the local layer `layer` into its instance's main (R3's button).
    pub(crate) fn push_local_layer(&mut self, layer: NodeId) {
        if let Some(why) = self.layer_push_refusal(layer) {
            self.session.info(why);
            return;
        }
        let doc = &self.session.doc;
        let Some(root) = doc
            .get(layer)
            .and_then(|n| n.parent())
            .and_then(|p| ondin_core::component::instance_root(doc, p))
        else {
            return;
        };
        let name = crate::private::layer(doc, layer).into_owned();
        let ops = push::push_layer(doc, layer, &mut self.session.ids);
        self.commit_push(root, ops, PushWhat::Layers(vec![name]));
    }

    /// A reset of one thing, committed as its push instead — the row beside a
    /// reset (`ui::reset_push_menu`): each field the reset would write back is
    /// written into the main from the copy.
    pub(crate) fn push_from_reset(&mut self, tx: Transaction) {
        let doc = &self.session.doc;
        let Some(root) =
            tx.0.iter()
                .find_map(|op| op.overwrites())
                .and_then(|c| ondin_core::component::instance_root(doc, c))
        else {
            return;
        };
        if let Some(why) = self.push_refusal(root) {
            self.session.info(why);
            return;
        }
        let ops = push::fields(doc, &tx.0, &mut self.session.ids);
        let n = ops
            .iter()
            .filter(|op| {
                !matches!(
                    op,
                    Operation::SetInsets { .. } | Operation::SetLayoutItem { .. }
                )
            })
            .count()
            .max(1);
        self.commit_push(root, ops, PushWhat::Fields(n));
    }

    /// A ghost row's *Push removal to main*: the list item the copy `c` removed,
    /// deleted from the main's list. `list` names which list.
    pub(crate) fn push_item_removal(&mut self, c: NodeId, list: &Operation, item: ItemId) {
        let doc = &self.session.doc;
        let Some(root) = ondin_core::component::instance_root(doc, c) else {
            return;
        };
        if let Some(why) = self.push_refusal(root) {
            self.session.info(why);
            return;
        }
        let ops = push::item_removal(doc, c, list, item)
            .map(|t| t.0)
            .unwrap_or_default();
        self.commit_push(root, ops, PushWhat::Fields(1));
    }

    /// Commit a push from `root` as one step, *Push to main*, and say what it did.
    fn commit_push(&mut self, root: NodeId, ops: Vec<Operation>, what: PushWhat) {
        if ops.is_empty() {
            self.session
                .info("Nothing here differs from the main component");
            return;
        }
        let main = self.push_main_name(root);
        let before = self.session.doc.clone();
        self.session.name_step("Push to main", icon::PUSH_TO_MAIN);
        if !self.commit_edit(Transaction(ops.clone())) {
            return;
        }
        let out = push::outcome(&before, &self.session.doc, root, &ops);
        let text = toast(&what, &main, out);
        self.session.info(text);
    }
}

/// The ⋯ menu's push rows (R1): under a rule after the resets, *Push all*, then
/// each family with its count, and *Push removals* apart under a second rule —
/// the one push that deletes. Text rows with a count, as the resets are; a row
/// with nothing to push is disabled and counts nothing. Returns the push picked.
pub(super) fn push_rows(
    ui: &mut egui::Ui,
    p: Pushable,
    props_defined: bool,
    refusal: Option<&str>,
    mut tip: impl FnMut(Kind) -> String,
) -> Option<Kind> {
    let mut picked = None;
    let count_of = |n: usize| (n > 0).then(|| n.to_string());
    let blocked = refusal.is_some();
    crate::ui::menu_sep(ui);
    let mut row = |ui: &mut egui::Ui, kind: Kind, label: &str, n: Option<usize>, any: bool| {
        let live = any && !blocked;
        let count = n.and_then(count_of);
        let r = crate::ui::MenuRow::new("", label)
            .accel(count.as_deref())
            .enabled(live);
        let resp = crate::ui::menu_row(ui, r, crate::ui::MENU_ROW_H);
        let resp = match (refusal, any) {
            (Some(why), _) => resp.on_disabled_hover_text(why),
            (None, false) => resp.on_disabled_hover_text(match kind {
                Kind::Order => "The order matches the main",
                _ => "Nothing here differs from the main component",
            }),
            (None, true) => {
                if resp.hovered() {
                    let t = tip(kind);
                    resp.on_hover_text(t)
                } else {
                    resp
                }
            }
        };
        if live && resp.clicked() {
            picked = Some(kind);
        }
    };
    row(ui, Kind::All, "Push all to main", None, p.any());
    if props_defined {
        row(
            ui,
            Kind::Properties,
            "Push properties to main",
            Some(p.properties),
            p.properties > 0,
        );
    }
    row(
        ui,
        Kind::Fields,
        "Push fields to main",
        Some(p.fields),
        p.fields > 0,
    );
    row(
        ui,
        Kind::Layers,
        "Push local layers to main",
        Some(p.layers),
        p.layers > 0,
    );
    row(ui, Kind::Order, "Push order to main", None, p.order > 0);
    crate::ui::menu_sep(ui);
    row(
        ui,
        Kind::Removals,
        "Push removals to main",
        Some(p.removals),
        p.removals > 0,
    );
    picked
}

/// The labels [`push_rows`] draws, for the menu's width.
pub(super) const PUSH_LABELS: [&str; 6] = [
    "Push all to main",
    "Push properties to main",
    "Push fields to main",
    "Push local layers to main",
    "Push order to main",
    "Push removals to main",
];
