//! The **Component** card — what the selection is to the components machinery
//! (`architecture.md` §5.3d), drawn from §15 D981's accepted chrome
//! (`design/Components.dc.html`, sections 3A–3H): a main's instance count and its
//! two verbs, an instance's link to its main, its drift and the reset family, and
//! the summaries several selected instances get. A layer *inside* an instance gets
//! one line instead of a card — D981's *"one line, not a card"*.
//!
//! **Directly under the identity card** (3H), because it says what the layer *is*.
//!
//! **Counts tell you what each reset will do; a reset with nothing to do is
//! disabled, not hidden** (D981). The counts are `reset::Drift`, cached per
//! session revision (`OndinApp::drift_cache`), since the card is read every frame
//! and a drift is a walk of the instance.
//!
//! ⚠️ **The drift summary counts every difference a *Reset all* would undo** —
//! fields, removed children and order — where the mockup's *3 overrides* sat over a
//! *Reset fields 3* and a *Restore removed children 1*. Read literally that hides a
//! removed child from the summary, and an instance whose only drift is a deleted
//! layer then reads as untouched; the session's choice, open to overturning.
//!
//! **The main's glyph is the filled hexagon** (D981), drawn by
//! `ui::paint_hexagon_filled` since the bundled Phosphor Regular has none (§15 D10,
//! D985); a main's and a variant's face carry it, an instance's the outline one.
//! Until 2026-10-06 every face drew the outline, the filled one owed.

use crate::app::OndinApp;
use crate::theme::{self, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::component;
use ondin_core::reset::{Drift, Kind};
use ondin_core::{NodeId, Operation, Transaction};

/// One overridden field's mark (§15 D981): its tooltip, *Reset to main · 168*,
/// and the transaction that writes the main's value back into that field alone.
pub(super) struct OverrideMark {
    pub(super) tip: String,
    pub(super) tx: Transaction,
}

impl OverrideMark {
    pub(super) fn field(&self) -> ui::FieldMark<'_> {
        ui::FieldMark { tip: &self.tip }
    }

    /// One mark for a field two stored values stand behind — a laid-out W, which
    /// is the size and the item's `width` — resetting both, and naming the first's
    /// value where both differ.
    pub(super) fn and(a: Option<Self>, b: Option<Self>) -> Option<Self> {
        match (a, b) {
            (Some(mut a), Some(b)) => {
                a.tx.0.extend(b.tx.0);
                Some(a)
            }
            (a, b) => a.or(b),
        }
    }
}

/// The card's faces, one per kind of selection D981 draws.
enum Face {
    /// A main component, and how many instances it has.
    Main { name: String, instances: usize },
    /// A component set — its own card, *Variants* (§15 D982).
    Set { set: NodeId },
    /// A variant: a main inside a set, named by its values (§15 D982).
    Variant {
        main: NodeId,
        set: NodeId,
        set_name: String,
        instances: usize,
    },
    /// A layer inside a main, which can be bound to its properties (§15 D982).
    InMain { node: NodeId },
    /// One instance root.
    Instance {
        main: NodeId,
        main_name: String,
        drift: Drift,
        props: PropDrift,
    },
    /// A layer inside an instance — linked to a counterpart in the main, or the
    /// instance's own.
    Child { main_name: String, linked: bool },
    /// Several instance roots and nothing else: of one main (`main` is `Some`) or
    /// of several.
    Instances {
        count: usize,
        main: Option<(NodeId, String)>,
        mains: usize,
        drifted: usize,
        drift: Drift,
        props: PropDrift,
        /// The roots, when they share one owner — one main, or variants of one
        /// set (4E, 4F): what earns them the variant and property rows.
        shared: Option<Vec<NodeId>>,
    },
    /// Instances among ordinary layers — summarised, with a way to narrow to them.
    Mixed { roots: Vec<NodeId>, others: usize },
}

/// An instance's **properties** apart from its other overrides (§15 D982, 4A's
/// *2 properties · 1 override*): how many of its component properties differ from
/// the main, and how many of `Drift::fields`' units are those properties' fields —
/// counted once, as properties, and not again as overrides.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PropDrift {
    pub(crate) props: usize,
    pub(crate) units: usize,
    /// Whether the main defines any property at all — what puts *Reset
    /// properties* in the overflow, disabled at zero by D981's rule for the
    /// card's counted resets, and leaves it out for a component with none.
    pub(crate) defined: bool,
}

/// [`PropDrift`] for the instance rooted at `root`.
pub(crate) fn prop_drift(doc: &ondin_core::Document, root: NodeId) -> PropDrift {
    use ondin_core::variant::{self, PropKind};
    let defined = !variant::instance_properties(doc, root).is_empty();
    let props = variant::instance_properties(doc, root)
        .iter()
        .filter(|p| variant::property_state(doc, root, p).is_some_and(|(_, o)| o))
        .count();
    let units = variant::property_fields(doc, root)
        .into_iter()
        .filter(|(c, kind)| {
            ondin_core::reset::overrides(doc, *c)
                .iter()
                .any(|o| match kind {
                    PropKind::Boolean => matches!(o.reset, Operation::SetVisible { .. }),
                    PropKind::Text => matches!(o.reset, Operation::SetText { .. }),
                    PropKind::Swap => matches!(o.reset, Operation::SetSwap { .. }),
                })
        })
        .count();
    PropDrift {
        props,
        units,
        defined,
    }
}

/// What a click on the card asked for, acted on after it is drawn so nothing is
/// borrowed while the document changes.
enum Act {
    /// Every component property of the selected instances back to the main's.
    ResetProperties,
    Select(Vec<NodeId>),
    GoToMain(NodeId),
    /// The selected child's counterpart in the main — `OndinApp::go_to_main`'s
    /// rule for a child, which selects what it was copied from, not the main.
    GoToSource,
    Reset(Kind),
    Detach,
    SelectAllInstances,
    DuplicateAsComponent,
}

impl OndinApp {
    /// [`reset::drift`](ondin_core::reset::drift) at `scope`, from the cache while
    /// the document is unchanged.
    pub(crate) fn drift_of(&mut self, scope: NodeId) -> Drift {
        let rev = self.session.revision();
        if self.drift_cache.0 != rev {
            self.drift_cache = (rev, Default::default());
        }
        let doc = &self.session.doc;
        *self
            .drift_cache
            .1
            .entry(scope)
            .or_insert_with(|| ondin_core::reset::drift(doc, scope))
    }

    /// Gather [`OndinApp::card_overrides`] for this frame: every selected linked
    /// layer's overrides (`reset::overrides`), grouped by the card that shows them.
    /// Over several instances a card counts and resets every one of them — D981's
    /// *"the dot shows, and a reset resets every one of them"*.
    pub(super) fn gather_card_overrides(&mut self) {
        let doc = &self.session.doc;
        let mut cards: Vec<(&'static str, usize, Vec<Operation>)> = Vec::new();
        let mut fields = Vec::new();
        for id in self.session.selection.ids() {
            let overrides = ondin_core::reset::overrides(doc, *id);
            fields.push((*id, overrides.iter().map(|o| o.reset.clone()).collect()));
            for o in overrides {
                let Some(title) = card_of(&o.reset) else {
                    continue;
                };
                match cards.iter_mut().find(|(t, ..)| *t == title) {
                    Some((_, n, ops)) => {
                        *n += o.units;
                        ops.push(o.reset);
                    }
                    None => cards.push((title, o.units, vec![o.reset])),
                }
            }
        }
        // **Reset transform carries the main's insets**, as a field's reset does
        // (`OndinApp::transform_marks`): on a pinned layer the insets are the
        // placement, and a transform or size written without them is re-pinned
        // where it lands by `keep_insets`. Counted under Position, not here.
        //
        // ⚠️ **Not an instance root's**: its insets are its own placement in its
        // own parent (`propagate::is_placement`), so it keeps them — written back
        // unchanged, which still keeps `keep_insets` off the edit. `c292b50` gave a
        // root the main's and unpinned a pinned instance
        // (`a_transform_reset_keeps_an_instance_roots_own_pin`, `arch-scribe`'s).
        // And only a node whose ops **place** it: a pivot alone moves nothing.
        if let Some((_, _, ops)) = cards.iter_mut().find(|(t, ..)| *t == "Transform") {
            let mut placed: Vec<NodeId> = ops
                .iter()
                .filter(|op| {
                    matches!(
                        op,
                        Operation::SetTransform { .. } | Operation::SetGeometry { .. }
                    )
                })
                .filter_map(Operation::overwrites)
                .collect();
            placed.sort();
            placed.dedup();
            for id in placed {
                let own = ondin_core::reset::placement_is_own(doc, id);
                let from = if own {
                    Some(id)
                } else {
                    ondin_core::reset::source_of(doc, id)
                };
                if let Some(n) = from.and_then(|s| doc.get(s)) {
                    ops.push(Operation::SetInsets {
                        id,
                        insets: *n.insets(),
                    });
                }
            }
        }
        self.card_overrides = cards;
        self.field_overrides = fields;
    }

    /// An overridden field put back (§15 D981): `mark`'s reset, committed when its
    /// ↺ was clicked (`hit`), through [`OndinApp::commit_reset`].
    pub(super) fn reset_marked(&mut self, hit: bool, mark: &Option<OverrideMark>) {
        if let (true, Some(m)) = (hit, mark) {
            self.commit_reset(m.tx.clone());
        }
    }

    /// `tx`, a reset, with the item of every layer it resizes stated beside the
    /// size — the reset's own where it resets the item, else the layer's unchanged
    /// — so the commit door's `build::flex_holds` does not take the reset for the
    /// hand's resize ([`OndinApp::commit_reset`]'s first point). The context menu's
    /// reset (`OndinApp::reset_tx`) takes it too.
    pub(crate) fn items_stated(&self, mut tx: Transaction) -> Transaction {
        let doc = &self.session.doc;
        let mut stated: Vec<Operation> = Vec::new();
        for op in &tx.0 {
            let Operation::SetGeometry { id, geometry } = op else {
                continue;
            };
            let named =
                |o: &Operation| matches!(o, Operation::SetLayoutItem { id: i, .. } if i == id);
            if !geometry.resizes() || tx.0.iter().any(named) || stated.iter().any(named) {
                continue;
            }
            if let Some(n) = doc.get(*id) {
                stated.push(Operation::SetLayoutItem {
                    id: *id,
                    item: *n.item(),
                });
            }
        }
        tx.0.extend(stated);
        tx
    }

    /// Commit an instance's reset from the inspector (§15 D981) — a field's ↺, a
    /// card header's chip, the Component card's resets. Through `commit_edit`, as
    /// every inspector edit that changes ink is, and two things more. (The context
    /// menu's commits through `reset_selection`, as the menu's verbs do: it gets the
    /// first through `reset_tx`, and not the second — a live text session owns the
    /// pointer, so that menu does not open over one.)
    ///
    /// - **A reset is not a resize.** `build::flex_holds` reads a size written with
    ///   no item beside it as the hand's resize and holds it — growth to 0, a
    ///   percentage or `fit-content` back to px — so a W reset on an instance root
    ///   growing in a flex row stopped its growth, its placement being its own
    ///   (§5.3d), and on a nested copy made an item override that was not there.
    ///   Each layer the reset resizes has its item stated in the same transaction:
    ///   the reset's own where it resets the item, else the layer's unchanged.
    ///   `6210953`'s insets, for the other door's other rewrite.
    /// - **A live text session on a layer the reset restyles adopts it**
    ///   (`text_session_restyled_after`), as every other Type card write does —
    ///   or the editor went on laying its text out in the style the reset took
    ///   away. **Not on a content reset** (`SetText`): the editor holds content a
    ///   commit ahead of the document's, and spans keyed to the reset's string are
    ///   not keyed to its.
    ///
    /// Both found by `arch-scribe` reading the batch; the restyle is tested since
    /// 2026-10-06 (`a_type_reset_restyles_a_live_text_session`, on the editor's
    /// layout — the node's style cannot see it).
    pub(crate) fn commit_reset(&mut self, tx: Transaction) {
        let tx = self.items_stated(tx);
        let editing = self.text.as_ref().map(|s| s.id);
        let restyles = editing.is_some_and(|e| {
            tx.0.iter().any(|op| {
                op.overwrites() == Some(e)
                    && matches!(
                        op,
                        Operation::SetTextStyle { .. }
                            | Operation::SetParagraphStyle { .. }
                            | Operation::SetBlockStyle { .. }
                            | Operation::SetGeometry { .. }
                    )
            })
        });
        self.commit_edit(tx.clone());
        if restyles {
            self.text_session_restyled_after(&tx);
        }
    }

    /// A **sub-field's** override mark over `subjects` (§15 D981): one number or
    /// choice inside a struct payload — a layout item's grow, a container's gap, a
    /// text style's size, one inset — which `reset::overrides` compares and resets
    /// only as a whole value.
    ///
    /// `theirs` reads the source's whole value out of one of this frame's field
    /// resets ([`OndinApp::field_overrides`]), so a field the comparison skips — an
    /// instance root's own placement, a kind its source does not share — is never
    /// marked, by the same rule that never counts it. `mine` is the copy's whole
    /// value, `get` the sub-field, `put` writes a sub-field into a whole value,
    /// `write` makes the operation and `say` the tooltip's value.
    ///
    /// Over several subjects the mark shows if any of them differs, and its reset
    /// writes each one that does — D981's *"the dot shows, and a reset resets every
    /// one of them"*. The tooltip names the first one's main value.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sub_mark<T: Clone + PartialEq, V: PartialEq + Clone>(
        &self,
        subjects: &[NodeId],
        theirs: impl Fn(&Operation) -> Option<T>,
        mine: impl Fn(&ondin_core::Node) -> Option<T>,
        get: impl Fn(&T) -> V,
        put: impl Fn(&mut T, V),
        write: impl Fn(NodeId, T) -> Operation,
        say: impl Fn(&V) -> String,
    ) -> Option<OverrideMark> {
        let mut ops = Vec::new();
        let mut tip = None;
        for id in subjects {
            // A card's subject outside the selection — the Mask card's masks inside
            // a selected group — is compared here rather than in the frame's cache.
            let computed: Vec<Operation>;
            let resets = match self.field_overrides.iter().find(|(n, _)| n == id) {
                Some((_, r)) => r.as_slice(),
                None => {
                    computed = ondin_core::reset::overrides(&self.session.doc, *id)
                        .into_iter()
                        .map(|o| o.reset)
                        .collect();
                    computed.as_slice()
                }
            };
            let (Some(src), Some(mut cur)) = (
                resets.iter().find_map(&theirs),
                self.session.doc.get(*id).and_then(&mine),
            ) else {
                continue;
            };
            let want = get(&src);
            if get(&cur) == want {
                continue;
            }
            // A reset that would write nothing marks nothing: a sub-field the
            // source's value has no place for — a flex field where the main is a
            // grid — differs, and `put` has nowhere to take it from. The layout's
            // own mark is the one that says so.
            let before = cur.clone();
            let shown = say(&want);
            put(&mut cur, want);
            if cur == before {
                continue;
            }
            tip.get_or_insert_with(|| format!("Reset to main · {shown}"));
            ops.push(write(*id, cur));
        }
        Some(OverrideMark {
            tip: tip?,
            tx: Transaction(ops),
        })
    }

    /// The overrides `title`'s header shows, if it has any.
    pub(super) fn card_override(&self, title: &str) -> Option<(usize, &[Operation])> {
        self.card_overrides
            .iter()
            .find(|(t, ..)| *t == title)
            .map(|(_, n, ops)| (*n, ops.as_slice()))
    }

    /// The card's face for the selection, or `None` where it draws nothing.
    fn component_face(&mut self) -> Option<Face> {
        let ids = ondin_core::build::outermost(&self.session.doc, self.session.selection.ids());
        let doc = &self.session.doc;
        let name = |id: NodeId| {
            doc.get(id)
                .map(|n| n.name().to_string())
                .unwrap_or_default()
        };
        let main_of = |root: NodeId| component::main_of(doc, root);
        if let [one] = ids.as_slice() {
            let node = doc.get(*one)?;
            if node.set().is_some() {
                return Some(Face::Set { set: *one });
            }
            if node.component() {
                let instances = component::instances_of(doc, *one).len();
                if let Some(set) = ondin_core::variant::set_of(doc, *one) {
                    return Some(Face::Variant {
                        main: *one,
                        set,
                        set_name: name(set),
                        instances,
                    });
                }
                return Some(Face::Main {
                    name: node.name().to_string(),
                    instances,
                });
            }
            let Some(root) = component::instance_root(doc, *one) else {
                return ondin_core::variant::owner_above(doc, *one)
                    .map(|_| Face::InMain { node: *one });
            };
            let main = main_of(root)?;
            let main_name = name(main);
            if root != *one {
                return Some(Face::Child {
                    main_name,
                    linked: node.link().is_some(),
                });
            }
            let drift = self.drift_of(root);
            let props = prop_drift(&self.session.doc, root);
            return Some(Face::Instance {
                main,
                main_name,
                drift,
                props,
            });
        }
        let roots: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|id| component::instance_root(doc, *id) == Some(*id))
            .collect();
        if roots.is_empty() {
            return None;
        }
        if roots.len() < ids.len() {
            return Some(Face::Mixed {
                others: ids.len() - roots.len(),
                roots,
            });
        }
        let mut mains: Vec<NodeId> = roots.iter().filter_map(|r| main_of(*r)).collect();
        mains.sort();
        mains.dedup();
        let main = match mains.as_slice() {
            [m] => Some((*m, name(*m))),
            _ => None,
        };
        let count = roots.len();
        // One owner — one main, or variants of one set — is one component (4F).
        let mut owners: Vec<NodeId> = mains
            .iter()
            .filter_map(|m| ondin_core::variant::owner_of_main(doc, *m))
            .collect();
        owners.sort();
        owners.dedup();
        let shared = (owners.len() == 1).then(|| roots.clone());
        // Variants of one set read as instances of the set (4E), named for it.
        let main = main.or_else(|| {
            let o = *owners.first().filter(|_| owners.len() == 1)?;
            doc.get(o)?.set()?;
            Some((o, name(o)))
        });
        let props = roots
            .iter()
            .map(|r| prop_drift(doc, *r))
            .fold(PropDrift::default(), |a, b| PropDrift {
                props: a.props + b.props,
                units: a.units + b.units,
                defined: a.defined || b.defined,
            });
        let drifts: Vec<Drift> = roots.iter().map(|r| self.drift_of(*r)).collect();
        Some(Face::Instances {
            count,
            main,
            mains: mains.len(),
            drifted: drifts.iter().filter(|d| d.any()).count(),
            drift: drifts.into_iter().fold(Drift::default(), sum),
            props,
            shared,
        })
    }

    /// The Component card (§15 D981) — see the module doc.
    pub(super) fn inspector_component(&mut self, ui: &mut egui::Ui) {
        let Some(face) = self.component_face() else {
            return;
        };
        let mut act = None;
        match &face {
            Face::Child { main_name, linked } => {
                slim_row(ui, |ui| child_line(ui, main_name, *linked, &mut act));
            }
            Face::Mixed { roots, others } => {
                slim_row(ui, |ui| mixed_line(ui, roots, *others, &mut act));
            }
            Face::InMain { node } => self.bind_line(ui, *node),
            Face::Set { set } => {
                let set = *set;
                self.panel(ui, "Variants", None, |app, ui| app.set_body(ui, set));
            }
            Face::Main { .. }
            | Face::Variant { .. }
            | Face::Instance { .. }
            | Face::Instances { .. } => {
                self.panel(ui, "Component", None, |app, ui| match &face {
                    Face::Main { name, instances } => main_body(ui, name, *instances, &mut act),
                    // 3D: *Variant in Button* over the derived, read-only name, a
                    // dropdown per property, then the main's count and verbs.
                    Face::Variant {
                        main,
                        set,
                        set_name,
                        instances,
                    } => {
                        let name = app
                            .session
                            .doc
                            .get(*main)
                            .map(|n| n.name().to_string())
                            .unwrap_or_default();
                        let link = Some((*set, set_name.as_str()));
                        heading(ui, true, "Variant in", link, None, "", &mut act);
                        ui.label(
                            egui::RichText::new(name)
                                .size(13.0)
                                .color(theme::text::STRONG),
                        );
                        app.variant_rows(ui, *main);
                        main_tail(ui, *instances, &mut act);
                    }
                    Face::Instance {
                        main,
                        main_name,
                        drift,
                        props,
                    } => {
                        let summary = drift_summary(*drift, *props);
                        let link = Some((*main, main_name.as_str()));
                        heading(ui, false, "Instance of", link, None, &summary, &mut act);
                        let roots = app.session.selection.ids().to_vec();
                        app.instance_rows(ui, &roots);
                        reset_row(ui, *drift, *props, true, &mut act);
                    }
                    Face::Instances {
                        count,
                        main,
                        mains,
                        drifted,
                        drift,
                        props,
                        shared,
                    } => {
                        let summary = match drifted {
                            0 => String::new(),
                            n => format!("{n} with changes"),
                        };
                        match main {
                            Some((m, n)) => {
                                let caption = format!("{count} instances of");
                                heading(
                                    ui,
                                    false,
                                    &caption,
                                    Some((*m, n)),
                                    None,
                                    &summary,
                                    &mut act,
                                );
                                if let Some(roots) = shared {
                                    app.instance_rows(ui, roots);
                                }
                                reset_row(ui, *drift, *props, true, &mut act);
                            }
                            // 3F: no single main to go to, so no name link and no
                            // overflow of counted resets.
                            None => {
                                let title = format!("Instances of {mains} components");
                                heading(ui, false, "", None, Some(&title), &summary, &mut act);
                                reset_row(ui, *drift, *props, false, &mut act);
                            }
                        }
                    }
                    Face::Child { .. }
                    | Face::Mixed { .. }
                    | Face::InMain { .. }
                    | Face::Set { .. } => {}
                });
            }
        }
        // A main's or a set's own properties, under its card (3G).
        self.inspector_properties(ui);
        match act {
            None => {}
            Some(Act::ResetProperties) => {
                let doc = &self.session.doc;
                let roots: Vec<NodeId> =
                    ondin_core::build::outermost(doc, self.session.selection.ids())
                        .into_iter()
                        .filter(|id| component::instance_root(doc, *id) == Some(*id))
                        .collect();
                let ops: Vec<Operation> = roots
                    .iter()
                    .flat_map(|r| {
                        ondin_core::variant::instance_properties(doc, *r)
                            .into_iter()
                            .flat_map(|p| ondin_core::variant::reset_property(doc, &[*r], &p))
                    })
                    .collect();
                if !ops.is_empty() {
                    self.commit_reset(Transaction(ops));
                }
            }
            // *Reset fields* leaves the properties alone (4C): their fields are
            // counted, and reset, as properties.
            Some(Act::Reset(Kind::Fields)) => {
                if let Some(tx) = self.reset_tx(Kind::Fields) {
                    let doc = &self.session.doc;
                    use ondin_core::variant::PropKind;
                    let bound: std::collections::HashSet<(NodeId, PropKind)> = self
                        .session
                        .selection
                        .ids()
                        .iter()
                        .filter_map(|id| component::instance_root(doc, *id))
                        .flat_map(|r| ondin_core::variant::property_fields(doc, r))
                        .collect();
                    let kept: Vec<Operation> =
                        tx.0.into_iter()
                            .filter(|op| match op {
                                Operation::SetVisible { id, .. } => {
                                    !bound.contains(&(*id, PropKind::Boolean))
                                }
                                Operation::SetText { id, .. } => {
                                    !bound.contains(&(*id, PropKind::Text))
                                }
                                Operation::SetSwap { id, .. } => {
                                    !bound.contains(&(*id, PropKind::Swap))
                                }
                                _ => true,
                            })
                            .collect();
                    if !kept.is_empty() {
                        self.commit_reset(Transaction(kept));
                    }
                }
            }
            Some(Act::Select(ids)) => self.session.selection.set(ids),
            Some(Act::GoToMain(main)) => {
                self.session.selection.set_one(main);
                self.reveal_selection();
            }
            Some(Act::GoToSource) => self.go_to_main(),
            // Through `commit_reset`, which is `commit_edit` as every inspector edit
            // that changes ink is (`OndinApp::reset_tx`), restyling a live text
            // session too. *Detach* below changes no pixel and commits as its verb
            // does — *Group*'s case in `commit_edit`'s doc.
            Some(Act::Reset(kind)) => {
                if let Some(tx) = self.reset_tx(kind) {
                    self.commit_reset(tx);
                }
            }
            Some(Act::Detach) => self.detach_instances(),
            Some(Act::SelectAllInstances) => self.select_all_instances(),
            Some(Act::DuplicateAsComponent) => self.duplicate_as_component(),
        }
    }

    /// Whether the instance layer `id` removed any of its source's items from the
    /// list `read` takes — the ghost rows (§15 D981, 4D) that keep a list card with
    /// no item of its own from reading, and closing, as empty. `false` for `None`
    /// and outside an instance.
    pub(super) fn has_ghosts<T: Clone>(
        &self,
        id: Option<NodeId>,
        read: impl Fn(&ondin_core::Node) -> &[ondin_core::Keyed<T>],
    ) -> bool {
        let doc = &self.session.doc;
        let Some(cur) = id.and_then(|id| doc.get(id)) else {
            return false;
        };
        ondin_core::reset::source_of(doc, cur.id())
            .and_then(|s| doc.get(s))
            .is_some_and(|src| !ondin_core::reset::removed_items(read(src), read(cur)).is_empty())
    }

    /// The list `read` takes from the node `id` is compared with — its source's
    /// (`reset::source_of`) — or `None` outside an instance. What a list card
    /// reads its rows' trailing slots and its ghost rows against (§15 D981, 4D).
    pub(super) fn source_list<T: Clone>(
        &self,
        id: NodeId,
        read: impl Fn(&ondin_core::Node) -> &[ondin_core::Keyed<T>],
    ) -> Option<Vec<ondin_core::Keyed<T>>> {
        let doc = &self.session.doc;
        ondin_core::reset::source_of(doc, id)
            .and_then(|s| doc.get(s))
            .map(|s| read(s).to_vec())
    }
}

/// A list item's reserved 14-pt trailing slot (§15 D981, 4D), laid out by a
/// right-to-left row: empty while the item follows its main, the dot when it
/// differs, `+` when it is the instance's own. On an overridden item whose row is
/// `hot` — the pointer anywhere on it — the dot becomes ↺; answers whether that
/// was clicked.
///
/// ⚠️ **`hot` comes from the caller**, because this sits in a right-to-left run
/// whose own box begins past the row's label: asked of that box, a pointer on the
/// label was not on the row, and the ↺ never showed (measured by
/// `a_fill_list_marks_restores_and_resets_item_by_item`).
pub(super) fn item_slot(ui: &mut egui::Ui, state: ondin_core::reset::ItemState, hot: bool) -> bool {
    use ondin_core::reset::ItemState;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(14.0, ui.max_rect().height().min(ui::CONTROL_H)),
        egui::Sense::click(),
    );
    let p = ui.painter();
    match state {
        ItemState::Follows => false,
        ItemState::Local => {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon::PLUS,
                theme::icon_font(11.0),
                theme::text::MUTED,
            );
            resp.on_hover_text("This instance's own — not in the main");
            false
        }
        ItemState::Overridden if hot => {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon::ARROW_COUNTER_CLOCKWISE,
                theme::icon_font(12.0),
                theme::text::STRONG,
            );
            resp.on_hover_text("Reset to main").clicked()
        }
        ItemState::Overridden => {
            ui::override_dot(p, rect.center());
            false
        }
    }
}

/// A **ghost row**: an item the instance removed while its main still has it,
/// drawn dashed and dim with *Restore* (§15 D981, 4D) — the list half of the
/// reset family. `label` is the item's own words (a hex, *Linear*, *Drop
/// shadow*). Answers whether *Restore* was clicked.
///
/// ⚠️ **Lists only.** A deleted child *layer* gets no ghost row in the layers
/// panel — D981's asymmetry, ruled: only the card's *Restore removed children*.
pub(super) fn ghost_row(ui: &mut egui::Ui, label: &str, width: f32) -> bool {
    let h = ui::CONTROL_H;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
    let p = ui.painter();
    let dash = egui::Stroke::new(1.0, theme::text::FAINT);
    let r = rect.shrink(0.5);
    for (a, b) in [
        (r.left_top(), r.right_top()),
        (r.right_top(), r.right_bottom()),
        (r.right_bottom(), r.left_bottom()),
        (r.left_bottom(), r.left_top()),
    ] {
        p.extend(egui::Shape::dashed_line(&[a, b], dash, 3.0, 3.0));
    }
    let chip = egui::Rect::from_center_size(
        egui::pos2(rect.left() + ui::FIELD_PAD_X + 8.0, rect.center().y),
        egui::vec2(12.0, 12.0),
    );
    p.rect_filled(chip, 2.0, theme::text::FAINT.gamma_multiply(0.5));
    p.text(
        egui::pos2(chip.right() + 9.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        theme::text::FAINT,
    );
    let word = "Restore";
    let w = ui::action_button_w(ui.ctx(), word) - 8.0;
    let at = egui::Rect::from_min_size(
        egui::pos2(rect.right() - w - 4.0, rect.top() + 4.0),
        egui::vec2(w, h - 8.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(at));
    ui::action_button(
        &mut child,
        icon::ARROW_COUNTER_CLOCKWISE,
        word,
        FieldButton::Off,
        at.size(),
    )
    .on_hover_text("Put the main's item back")
    .clicked()
}

/// The inspector card that shows the field `op` writes — its title, as `panel`
/// takes it — or `None` for a field no card header owns: the name and visibility
/// (the identity card), a path's or a boolean's shape, the text's content and its
/// rail, the mask flag and the fill rule (identity-row toggles).
///
/// Wildcard-free on the operation, so a new field operation is placed here by
/// whoever adds it.
fn card_of(op: &Operation) -> Option<&'static str> {
    use Operation as O;
    use ondin_core::GeometryPatch as G;
    match op {
        O::SetTransform { .. } | O::SetPivot { .. } => Some("Transform"),
        O::SetGeometry { geometry, .. } => match geometry {
            G::Size(_) | G::LineEnd(_) => Some("Transform"),
            G::CornerRadius(_) | G::CornerRadii(_) | G::Sides(_) | G::InnerRatio(_) => {
                Some("Appearance")
            }
            G::TextSizing(_) => Some("Type"),
            G::Path { .. }
            | G::TextPath(_)
            | G::TextPathFlip(_)
            | G::TextPathOffset(_)
            | G::BoolOp(_) => None,
        },
        O::SetOpacity { .. } | O::SetClip { .. } => Some("Appearance"),
        O::SetMaskMode { .. } => Some("Mask"),
        O::SetFills { .. } => Some("Fill"),
        O::SetStrokes { .. } => Some("Stroke"),
        O::SetEffects { .. } => Some("Effects"),
        O::SetExports { .. } => Some("Export"),
        O::SetLayoutGrids { .. } => Some("Layout grid"),
        O::SetInsets { .. } => Some("Position"),
        O::SetLayoutItem { .. } => Some("Item"),
        O::SetDisplay { .. } => Some("Container"),
        O::SetTextStyle { .. } | O::SetParagraphStyle { .. } | O::SetBlockStyle { .. } => {
            Some("Type")
        }
        O::SetName { .. }
        | O::SetVisible { .. }
        | O::SetMask { .. }
        | O::SetFillRule { .. }
        | O::SetText { .. }
        | O::SetTextSpans { .. }
        | O::SetParagraphSpans { .. }
        | O::SetLocked { .. }
        | O::SetProportionsLocked { .. }
        | O::SetComponent { .. }
        | O::SetLink { .. }
        | O::SetSwap { .. }
        | O::SetVariantSet { .. }
        | O::SetVariant { .. }
        | O::SetProperties { .. }
        | O::CreateNode { .. }
        | O::DeleteNode { .. }
        | O::InsertSubtree { .. }
        | O::Reparent { .. }
        | O::Reorder { .. }
        | O::SetCanvasBackground { .. }
        | O::AddGuide { .. }
        | O::RemoveGuide { .. }
        | O::SetGuidePosition { .. }
        | O::SetGuideColor { .. }
        | O::SetGuideScope { .. }
        | O::AddImage { .. }
        | O::RemoveImage { .. } => None,
    }
}

/// An overridden dropdown's reset (§15 D981), as the open list's first row —
/// ↺ *Reset to main · Alpha*, then a rule. The mockup's ↺ takes the label's slot,
/// and a dropdown's face is one press target that opens the list, with no label
/// slot of its own to give, so the reset is the first thing the press shows
/// instead. Answers whether it was clicked.
pub(super) fn menu_reset_row(ui: &mut egui::Ui, mark: &OverrideMark) -> bool {
    let hit = ui
        .selectable_label(
            false,
            ui::glyph_and_text(icon::ARROW_COUNTER_CLOCKWISE, &mark.tip),
        )
        .clicked();
    super::inspector::popup_rule(ui);
    hit
}

/// A row's label carrying an override mark (§15 D981, 4C — *"row-labelled
/// controls carry the dot after the label"*): at full brightness with the dot
/// after it, and with the pointer on the label ↺ in the dot's place, the mark's
/// tooltip, and a click that is the reset. Unmarked, the label as it was, in
/// `ink`. Answers whether the reset was clicked.
///
/// `size` is the label's slot, its text centred in it as `add_sized` centred the
/// plain label this replaces; `None` is the text's own width.
pub(super) fn label_mark(
    ui: &mut egui::Ui,
    size: Option<egui::Vec2>,
    text: &str,
    pt: f32,
    ink: egui::Color32,
    mark: Option<&OverrideMark>,
) -> bool {
    let color = match mark {
        Some(_) => theme::text::STRONG,
        None => ink,
    };
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(pt), color);
    let w = galley.size().x;
    // The dot's room beside the text, so a marked label's width does not move
    // what follows it.
    let want = size.unwrap_or(galley.size() + egui::vec2(8.0, 0.0));
    let sense = match mark {
        Some(_) => egui::Sense::click(),
        None => egui::Sense::hover(),
    };
    let (rect, resp) = ui.allocate_exact_size(want, sense);
    let left = match size {
        Some(_) => rect.center().x - w / 2.0,
        None => rect.left(),
    };
    let at = egui::pos2(left, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, color);
    let Some(m) = mark else {
        return false;
    };
    if resp.hovered() {
        ui.painter().text(
            egui::pos2(at.x + w + 6.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon::ARROW_COUNTER_CLOCKWISE,
            theme::icon_font(11.0),
            theme::text::STRONG,
        );
    } else {
        ui::override_dot(ui.painter(), at + egui::vec2(w + 1.5, 3.0));
    }
    resp.on_hover_text(&m.tip).clicked()
}

/// An overridden dropdown's dot, where it has no label to sit after: beside its
/// chevron, at the height a field's dot sits beside its label (§15 D981).
pub(super) fn combo_dot(p: &egui::Painter, face: egui::Rect) {
    ui::override_dot(p, egui::pos2(face.right() - 26.0, face.center().y - 4.0));
}

/// A number as a mark's tooltip names it — two decimals at most, trailing zeros
/// dropped: *Reset to main · 168*.
pub(super) fn mark_num(v: f64) -> String {
    let s = format!("{v:.2}");
    match s.trim_end_matches('0').trim_end_matches('.') {
        "-0" => "0".to_owned(),
        s => s.to_owned(),
    }
}

fn sum(a: Drift, b: Drift) -> Drift {
    Drift {
        fields: a.fields + b.fields,
        removed: a.removed + b.removed,
        order: a.order + b.order,
        local: a.local + b.local,
    }
}

/// *2 properties · 3 overrides · 1 local layer* — empty at zero, which is how it
/// disappears. A property's fields count once, as the property (§15 D982, 4A:
/// *"properties are the knobs you're meant to turn, so they shouldn't read as
/// drift"*).
fn drift_summary(d: Drift, p: PropDrift) -> String {
    let overrides = d.fields.saturating_sub(p.units) + d.removed + d.order;
    let mut parts = Vec::new();
    if p.props > 0 {
        parts.push(match p.props {
            1 => "1 property".to_string(),
            n => format!("{n} properties"),
        });
    }
    if overrides > 0 {
        parts.push(plural(overrides, "override"));
    }
    if d.local > 0 {
        parts.push(plural(d.local, "local layer"));
    }
    parts.join(" · ")
}

fn plural(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}

/// The two-line face of an instance card — or a variant's, with `main` — the
/// hexagon (outline for an instance, filled for a `main`, `paint_hexagon`), a
/// small caption over the main's name as a link (or over a plain `title`), and
/// the grey drift summary on the right.
fn heading(
    ui: &mut egui::Ui,
    main: bool,
    caption: &str,
    link: Option<(NodeId, &str)>,
    title: Option<&str>,
    summary: &str,
    act: &mut Option<Act>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        let slot = glyph_slot(ui);
        let block = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if !caption.is_empty() {
                ui.label(
                    egui::RichText::new(caption)
                        .size(11.0)
                        .color(theme::text::DIM),
                );
            }
            if let Some((main, name)) = link {
                // The name, then the ↗ — one target, so a click on either goes.
                let resp = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let name = ui.add(
                            egui::Label::new(
                                egui::RichText::new(name)
                                    .size(13.0)
                                    .underline()
                                    .color(theme::text::STRONG),
                            )
                            .sense(egui::Sense::click()),
                        );
                        let arrow = ui.add(
                            egui::Label::new(
                                egui::RichText::new(icon::ARROW_SQUARE_OUT)
                                    .font(theme::icon_font(12.0))
                                    .color(theme::text::DIM),
                            )
                            .sense(egui::Sense::click()),
                        );
                        name | arrow
                    })
                    .inner
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Go to main component");
                if resp.clicked() {
                    *act = Some(Act::GoToMain(main));
                }
            }
            if let Some(title) = title {
                ui.label(
                    egui::RichText::new(title)
                        .size(13.0)
                        .color(theme::text::STRONG),
                );
            }
        });
        paint_hexagon(ui, main, slot, block.response.rect);
        if !summary.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(summary)
                        .size(11.0)
                        .color(theme::text::DIM),
                );
            });
        }
    });
}

/// **Reset all**, **Detach**, and — where there is one main to count against —
/// the overflow of counted resets (3B, 3C).
fn reset_row(ui: &mut egui::Ui, d: Drift, p: PropDrift, overflow: bool, act: &mut Option<Act>) {
    let gap = ui::CARD_COL_GAP;
    let h = ui::CONTROL_H;
    let w = ui.available_width();
    let dots = if overflow { h + gap } else { 0.0 };
    let half = ((w - dots - gap) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let any = d.any();
        let state = if any {
            FieldButton::Off
        } else {
            FieldButton::Disabled
        };
        let reset = ui::action_button(
            ui,
            icon::ARROW_COUNTER_CLOCKWISE,
            "Reset all",
            state,
            egui::vec2(half, h),
        )
        .on_hover_text(if any {
            "Put everything back as the main has it — your own layers stay"
        } else {
            "Nothing here differs from the main component"
        });
        if any && reset.clicked() {
            *act = Some(Act::Reset(Kind::All));
        }
        if ui::action_button(
            ui,
            icon::LINK_BREAK,
            "Detach",
            FieldButton::Off,
            egui::vec2(half, h),
        )
        .on_hover_text("Detach instance · Ctrl+Alt+B")
        .clicked()
        {
            *act = Some(Act::Detach);
        }
        if overflow {
            let more = ui::field_button(ui, icon::DOTS_THREE, h, 15.0, FieldButton::Off)
                .on_hover_text("More resets");
            egui::Popup::menu(&more)
                .align(egui::RectAlign::BOTTOM_END)
                .show(|ui| {
                    ui::menu_rows(ui);
                    // 4C: properties and fields count, and reset, separately.
                    // Present wherever the component has properties, and
                    // disabled at zero like its neighbours (D981) — `arch-scribe`
                    // read the first cut hiding it.
                    if p.defined {
                        let count = match p.props {
                            0 => "—".to_string(),
                            n => n.to_string(),
                        };
                        let row = ui::MenuRow::new("", "Reset properties")
                            .accel(Some(&count))
                            .enabled(p.props > 0);
                        if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && p.props > 0 {
                            *act = Some(Act::ResetProperties);
                        }
                    }
                    for (kind, label, n) in [
                        (
                            Kind::Fields,
                            "Reset fields",
                            d.fields.saturating_sub(p.units),
                        ),
                        (Kind::Children, "Restore removed children", d.removed),
                        (Kind::Order, "Reset order", d.order),
                    ] {
                        let count = match n {
                            0 => "—".to_string(),
                            n => n.to_string(),
                        };
                        let row = ui::MenuRow::new("", label)
                            .accel(Some(&count))
                            .enabled(n > 0);
                        if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && n > 0 {
                            *act = Some(Act::Reset(kind));
                        }
                    }
                });
        }
    });
}

/// 3A: the filled hexagon, *Main component* over the name, the instance count
/// with *Select all*, and *Duplicate as component*.
fn main_body(ui: &mut egui::Ui, name: &str, instances: usize, act: &mut Option<Act>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        let slot = glyph_slot(ui);
        let block = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(
                egui::RichText::new("Main component")
                    .size(11.0)
                    .color(theme::text::DIM),
            );
            ui.label(
                egui::RichText::new(name)
                    .size(13.0)
                    .color(theme::text::STRONG),
            );
        });
        paint_hexagon(ui, true, slot, block.response.rect);
    });
    main_tail(ui, instances, act);
}

/// A main's count with *Select all*, and *Duplicate as component* — 3A's lower
/// half, which a variant's face shares (3D).
fn main_tail(ui: &mut egui::Ui, instances: usize, act: &mut Option<Act>) {
    let h = ui::CONTROL_H;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(plural(instances, "instance"))
                .size(12.0)
                .color(theme::text::MUTED),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = "Select all";
            let w = ui::action_button_w(ui.ctx(), label);
            let state = match instances {
                0 => FieldButton::Disabled,
                _ => FieldButton::Off,
            };
            let resp = ui::action_button(ui, icon::SELECTION_ALL, label, state, egui::vec2(w, h))
                .on_hover_text("Select all instances");
            if instances > 0 && resp.clicked() {
                *act = Some(Act::SelectAllInstances);
            }
        });
    });
    let w = ui.available_width();
    if ui::action_button(
        ui,
        icon::COPY,
        "Duplicate as component",
        FieldButton::Off,
        egui::vec2(w, h),
    )
    .on_hover_text("A copy that is a new main, not an instance of this one")
    .clicked()
    {
        *act = Some(Act::DuplicateAsComponent);
    }
}

/// 3D: *In Button instance · Go to main*, or *Local to this instance · no
/// counterpart*.
fn child_line(ui: &mut egui::Ui, main_name: &str, linked: bool, act: &mut Option<Act>) {
    let (glyph, text) = if linked {
        (icon::HEXAGON, format!("In {main_name} instance"))
    } else {
        (icon::PLUS, "Local to this instance".to_string())
    };
    let mut slot = egui::Rect::NOTHING;
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        slot = glyph_slot(ui);
        ui.label(
            egui::RichText::new(text)
                .size(12.0)
                .color(theme::text::STRONG),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if linked {
                let label = "Go to main";
                let w = ui::action_button_w(ui.ctx(), label);
                if ui::action_button(
                    ui,
                    icon::ARROW_SQUARE_OUT,
                    label,
                    FieldButton::Off,
                    egui::vec2(w, ui::CONTROL_H),
                )
                .on_hover_text("Go to main component")
                .clicked()
                {
                    *act = Some(Act::GoToSource);
                }
            } else {
                ui.label(
                    egui::RichText::new("no counterpart")
                        .size(11.0)
                        .color(theme::text::DIM),
                );
            }
        });
    });
    paint_glyph(ui, glyph, slot, row.response.rect);
}

/// 3G: *2 instances · 3 other layers*, and *Select instances* to narrow to them.
fn mixed_line(ui: &mut egui::Ui, roots: &[NodeId], others: usize, act: &mut Option<Act>) {
    let mut slot = egui::Rect::NOTHING;
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        slot = glyph_slot(ui);
        ui.label(
            egui::RichText::new(format!(
                "{} · {}",
                plural(roots.len(), "instance"),
                plural(others, "other layer")
            ))
            .size(12.0)
            .color(theme::text::STRONG),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = "Select instances";
            let w = ui::action_button_w(ui.ctx(), label);
            if ui::action_button(
                ui,
                icon::SELECTION_ALL,
                label,
                FieldButton::Off,
                egui::vec2(w, ui::CONTROL_H),
            )
            .clicked()
            {
                *act = Some(Act::Select(roots.to_vec()));
            }
        });
    });
    paint_glyph(ui, icon::HEXAGON, slot, row.response.rect);
}

/// The leading glyph's column, reserved before the row's other content exists.
///
/// ⚠️ **Painted afterwards ([`paint_glyph`]) rather than laid out as a label**:
/// a horizontal row centres each item against the height the row has *so far*,
/// so a glyph placed first sat centred on its own line and read as top-aligned
/// beside a two-line block — measured at y≈151 against the block's ≈158.
fn glyph_slot(ui: &mut egui::Ui) -> egui::Rect {
    ui.allocate_exact_size(egui::vec2(16.0, 0.0), egui::Sense::hover())
        .0
}

/// The leading glyph, centred on `beside` — the block or the row it leads.
fn paint_glyph(ui: &egui::Ui, glyph: &str, slot: egui::Rect, beside: egui::Rect) {
    ui.painter().text(
        egui::pos2(slot.center().x, beside.center().y),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(16.0),
        theme::text::STRONG,
    );
}

/// The hexagon in a face's glyph slot (§15 D981): **filled** for a main or a
/// variant (`ui::paint_hexagon_filled`, §15 D985), outline for an instance.
fn paint_hexagon(ui: &egui::Ui, filled: bool, slot: egui::Rect, beside: egui::Rect) {
    if filled {
        crate::ui::paint_hexagon_filled(
            ui.painter(),
            egui::pos2(slot.center().x, beside.center().y),
            16.0,
            theme::text::STRONG,
        );
    } else {
        paint_glyph(ui, icon::HEXAGON, slot, beside);
    }
}

/// A card with no header — the one-line faces (3D, 3G).
fn slim_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui::card_at(ui, |ui| add(ui));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, NodeKind, Operation, Placement, Transaction};

    /// A main frame *Button* holding a rect, an instance of it, and a plain rect
    /// beside them — adopted into a headless app.
    struct F {
        app: OndinApp,
        m: NodeId,
        i: NodeId,
        ir: NodeId,
        plain: NodeId,
    }

    fn fixture(ctx: &egui::Context) -> F {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xC4);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [m, r, plain] = [(); 3].map(|_| ids.mint());
        let rect = || NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
        };
        let create = |id, parent, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        doc.apply(&Transaction(vec![
            create(
                m,
                root,
                NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                "Button",
            ),
            create(r, m, rect(), "Label"),
            create(plain, root, rect(), "Plain"),
            Operation::SetComponent {
                id: m,
                component: true,
            },
        ]))
        .expect("a main");
        let (tx, made) = ondin_core::insert_subtrees(
            &doc,
            &mut ids,
            &[Placement {
                nodes: doc.capture_subtree(m).unwrap(),
                parent: root,
                index: None,
            }],
            Default::default(),
        );
        doc.apply(&tx).expect("an instance");
        let i = made[0];
        let ir = doc.get(i).unwrap().children()[0];
        app.session.adopt_document(doc, None);
        F {
            app,
            m,
            i,
            ir,
            plain,
        }
    }

    fn face_of(app: &mut OndinApp, ids: &[NodeId]) -> Option<Face> {
        app.session.selection.set(ids.to_vec());
        app.component_face()
    }

    /// Each selection gets D981's face for it: a main, an instance, a child, two
    /// instances of one main, instances among other layers, and nothing for a
    /// plain layer.
    #[test]
    fn each_selection_gets_its_face() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(matches!(
            face_of(&mut f.app, &[f.m]),
            Some(Face::Main { instances: 1, .. })
        ));
        assert!(matches!(
            face_of(&mut f.app, &[f.i]),
            Some(Face::Instance { main, .. }) if main == f.m
        ));
        assert!(matches!(
            face_of(&mut f.app, &[f.ir]),
            Some(Face::Child { linked: true, .. })
        ));
        assert!(matches!(
            face_of(&mut f.app, &[f.i, f.plain]),
            Some(Face::Mixed { others: 1, .. })
        ));
        assert!(face_of(&mut f.app, &[f.plain]).is_none());
        let second = {
            let doc = &f.app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut f.app.session.ids,
                &[Placement {
                    nodes: doc.capture_subtree(f.m).unwrap(),
                    parent: doc.root(),
                    index: None,
                }],
                Default::default(),
            );
            assert!(f.app.session.commit(tx));
            made[0]
        };
        assert!(matches!(
            face_of(&mut f.app, &[f.i, second]),
            Some(Face::Instances {
                count: 2,
                main: Some(_),
                ..
            })
        ));
    }

    /// The drift cache follows the document: an edit moves the revision and the
    /// count with it. Flip: never clearing the cache leaves the stale zero — and
    /// fails `reset_all_on_the_card_puts_the_instance_back` too, whose *1 override*
    /// is read through it.
    #[test]
    fn the_drift_cache_follows_an_edit() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert_eq!(f.app.drift_of(f.i), Drift::default());
        assert!(f.app.session.commit(Transaction(vec![Operation::SetName {
            id: f.ir,
            name: "Mine".into(),
        }])));
        assert_eq!(f.app.drift_of(f.i).fields, 1);
    }

    /// Every text the frame painted, with where it was painted.
    fn texts(out: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push((t.galley.text().to_string(), t.visual_bounding_rect()))
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        for s in &out.shapes {
            walk(&s.shape, &mut v);
        }
        v
    }

    fn frame(
        app: &mut OndinApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 2400.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            ui.set_max_width(280.0);
            app.inspector_ui(ui)
        })
    }

    /// **The card, drawn and clicked.** An instance with a renamed child shows
    /// *Instance of* over its main's name and *1 override*; a press and release on
    /// *Reset all* puts the name back and the summary goes. Driven through
    /// `RawInput`, so the button's `Response` is the one the card reads.
    #[test]
    fn reset_all_on_the_card_puts_the_instance_back() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(f.app.session.commit(Transaction(vec![Operation::SetName {
            id: f.ir,
            name: "Mine".into(),
        }])));
        f.app.session.selection.set_one(f.i);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = texts(&out);
        // The hexagon is centred on the caption-and-name block, not on its own
        // first line (`glyph_slot`): it measured y≈151 against the block's ≈158
        // laid out as a leading label.
        let at = painted
            .iter()
            .position(|(t, _)| t == "Instance of")
            .unwrap();
        let after = &painted[at..];
        let rect_of = |s: &str| after.iter().find(|(t, _)| t == s).unwrap().1;
        let block = rect_of("Instance of").union(rect_of("Button"));
        let hex = rect_of(icon::HEXAGON);
        assert!(
            (hex.center().y - block.center().y).abs() <= 1.0,
            "hexagon {hex:?} beside block {block:?}"
        );
        let has = |s: &str| painted.iter().any(|(t, _)| t == s);
        assert!(has("Instance of") && has("Button"), "{painted:?}");
        assert!(has("1 override"), "{painted:?}");
        let at = painted
            .iter()
            .find(|(t, _)| t == "Reset all")
            .map(|(_, r)| r.center())
            .expect("the button's word is painted");
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(at)]);
        frame(&mut f.app, &ctx, vec![press(true)]);
        frame(&mut f.app, &ctx, vec![press(false)]);
        assert_eq!(f.app.session.doc.get(f.ir).unwrap().name(), "Label");
        let out = frame(&mut f.app, &ctx, Vec::new());
        assert!(
            !texts(&out).iter().any(|(t, _)| t.contains("override")),
            "the summary is gone at zero"
        );
    }

    /// Every text the frame painted with its fallback colour — what a galley
    /// painted in one colour carries.
    fn inks(out: &egui::FullOutput) -> Vec<(String, egui::Rect, egui::Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect, egui::Color32)>) {
            match shape {
                egui::Shape::Text(t) => out.push((
                    t.galley.text().to_string(),
                    t.visual_bounding_rect(),
                    t.fallback_color,
                )),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        for s in &out.shapes {
            walk(&s.shape, &mut v);
        }
        v
    }

    /// **A field's mark** (§15 D981, 4A–4B). The instance's rect is moved along
    /// x only: its X label is painted bright, its Y dim — compared on the stored
    /// local translation, since the card's numbers are world ones and the copy sits
    /// elsewhere than its main with nothing overridden. A click on X's label slot
    /// writes the main's x back and leaves y alone. Flip: comparing the whole
    /// transform rather than one coefficient in `transform_marks` fails *"Y
    /// follows"* — Y painted bright.
    #[test]
    fn an_overridden_field_is_bright_and_its_label_resets_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let main_rect = f.app.session.doc.get(f.m).unwrap().children()[0];
        let moved = ondin_core::kurbo::Affine::translate((7.0, 0.0));
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: f.ir,
                    transform: moved,
                }]))
        );
        f.app.session.selection.set_one(f.ir);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = inks(&out);
        let ink_of = |s: &str| {
            painted
                .iter()
                .find(|(t, ..)| t == s)
                .map(|(_, r, c)| (*r, *c))
                .unwrap_or_else(|| panic!("no {s} in {painted:?}"))
        };
        let (x_at, x_ink) = ink_of("X");
        let (_, y_ink) = ink_of("Y");
        assert_eq!(x_ink, theme::text::STRONG, "X is overridden");
        assert_eq!(y_ink, theme::text::FAINT, "Y follows");
        let at = x_at.center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(at)]);
        frame(&mut f.app, &ctx, vec![press(true)]);
        frame(&mut f.app, &ctx, vec![press(false)]);
        let doc = &f.app.session.doc;
        assert_eq!(
            doc.get(f.ir).unwrap().transform(),
            doc.get(main_rect).unwrap().transform(),
            "the main's x is back"
        );
    }

    /// **The Appearance card's marks** (§15 D981): the instance's rect has its
    /// opacity and one corner's radius overridden. Opacity's drop glyph is bright
    /// and the radius field marked — the whole radius differs — while each corner
    /// is marked alone: the top left and no other, its reset writing that corner
    /// and leaving the other three as the copy holds them. A click on opacity's
    /// glyph puts the main's 100% back. Flip: dropping `sub_mark`'s equality skip
    /// marks every corner and fails *"the top right follows"*.
    #[test]
    fn appearance_marks_each_field_and_each_corner() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let mut radii = ondin_core::kurbo::RoundedRectRadii::from_single_radius(0.0);
        radii.top_left = 4.0;
        assert!(f.app.session.commit(Transaction(vec![
            Operation::SetOpacity {
                id: f.ir,
                opacity: 0.5,
            },
            Operation::SetGeometry {
                id: f.ir,
                geometry: ondin_core::GeometryPatch::CornerRadii(radii),
            },
        ])));
        f.app.session.selection.set_one(f.ir);
        f.app.gather_card_overrides();
        let marks = f.app.appearance_marks(f.ir);
        assert!(marks.opacity.is_some() && marks.radius.is_some());
        assert!(marks.corners[0].is_some(), "the top left is overridden");
        assert!(marks.corners[1].is_none(), "the top right follows");
        assert_eq!(
            marks.corners[0].as_ref().unwrap().tx,
            Transaction(vec![Operation::SetGeometry {
                id: f.ir,
                geometry: ondin_core::GeometryPatch::CornerRadii(Default::default()),
            }]),
            "the corner's reset writes that corner alone"
        );
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = inks(&out);
        let (drop_at, drop_ink) = painted
            .iter()
            .find(|(t, ..)| t == icon::DROP_HALF)
            .map(|(_, r, c)| (*r, *c))
            .expect("the opacity field");
        assert_eq!(drop_ink, theme::text::STRONG, "opacity is overridden");
        let at = drop_at.center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(at)]);
        frame(&mut f.app, &ctx, vec![press(true)]);
        frame(&mut f.app, &ctx, vec![press(false)]);
        assert_eq!(f.app.session.doc.get(f.ir).unwrap().opacity(), 1.0);
    }

    /// **An instance that removed every item still shows its ghost rows** (§15
    /// D981, 4D). The main's rect has a fill and an export; the instance removes
    /// both, leaving each list empty. Both cards stay open and each draws its
    /// ghost row — *Restore* twice — and each *Restore* puts the main's item back
    /// by its id. Flips, both run: `has_ghosts` answering `false` collapses both
    /// cards as empty and fails *"two ghost rows"* with none; the Fill card's
    /// empty-list check put back to `fills.is_empty()` alone draws *No fill* in
    /// the open card and fails the same assertion with one, the export's.
    #[test]
    fn an_emptied_list_still_draws_its_ghost_rows() {
        use ondin_core::{ExportSpec, Fill, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let fills = keyed_by_position([Fill {
            brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(10, 2, 30)),
            visible: true,
        }]);
        let exports = keyed_by_position([ExportSpec::new(
            ondin_core::ExportFormat::Png,
            ondin_core::ExportScale::Times(2.0),
        )]);
        assert!(f.app.session.commit(Transaction(vec![
            Operation::SetFills {
                id: r,
                fills: fills.clone(),
            },
            Operation::SetExports {
                id: r,
                exports: exports.clone(),
            },
        ])));
        assert!(f.app.session.commit(Transaction(vec![
            Operation::SetFills {
                id: f.ir,
                fills: Vec::new(),
            },
            Operation::SetExports {
                id: f.ir,
                exports: Vec::new(),
            },
        ])));
        f.app.session.selection.set_one(f.ir);
        let restores = |app: &mut OndinApp| {
            let mut out = frame(app, &ctx, Vec::new());
            for _ in 0..3 {
                out = frame(app, &ctx, Vec::new());
            }
            texts(&out)
                .into_iter()
                .filter(|(t, _)| t == "Restore")
                .map(|(_, r)| r.center())
                .collect::<Vec<_>>()
        };
        let found = restores(&mut f.app);
        assert_eq!(found.len(), 2, "two ghost rows");
        let click = |app: &mut OndinApp, at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(app, &ctx, vec![egui::Event::PointerMoved(at)]);
            frame(app, &ctx, vec![press(true)]);
            frame(app, &ctx, vec![press(false)]);
        };
        click(&mut f.app, found[0]);
        let found = restores(&mut f.app);
        assert_eq!(found.len(), 1, "one restored, one left");
        click(&mut f.app, found[0]);
        let node = f.app.session.doc.get(f.ir).unwrap();
        assert_eq!(node.paint().fills, fills, "the fill back by its id");
        assert_eq!(
            node.exports(),
            exports.as_slice(),
            "the export back by its id"
        );
    }

    /// **A list reads item by item against the main's** (§15 D981, 4D). The main's
    /// rect has two fills; the instance recolours the first, removes the second and
    /// adds one of its own. The removed one draws as a ghost row whose *Restore*
    /// puts it back by its id; the own one shows `+`; and the recoloured one's
    /// slot, under the pointer, is ↺, whose click takes the main's colour back.
    #[test]
    fn a_fill_list_marks_restores_and_resets_item_by_item() {
        use ondin_core::{Fill, Keyed, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let solid = |g: u8| Fill {
            brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(10, g, 30)),
            visible: true,
        };
        let main = keyed_by_position([solid(1), solid(2)]);
        assert!(f.app.session.commit(Transaction(vec![Operation::SetFills {
            id: r,
            fills: main.clone(),
        }])));
        assert_eq!(fills_of(&f.app, f.ir), main, "the fixture: it propagated");
        let own = Keyed::new(f.app.session.ids.mint_item(), solid(3));
        assert!(f.app.session.commit(Transaction(vec![Operation::SetFills {
            id: f.ir,
            fills: vec![main[0].map(|_| solid(9)), own.clone()],
        }])));
        f.app.session.selection.set_one(f.ir);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = texts(&out);
        let gone_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 2, 30));
        assert!(painted.iter().any(|(t, _)| *t == gone_hex), "a ghost row");
        let pluses = painted.iter().filter(|(t, _)| t == icon::PLUS).count();
        assert!(
            pluses >= 2,
            "the header's + and the own fill's: {painted:?}"
        );
        // *Restore* on the ghost row.
        let click = |app: &mut OndinApp, at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(app, &ctx, vec![egui::Event::PointerMoved(at)]);
            frame(app, &ctx, vec![press(true)]);
            frame(app, &ctx, vec![press(false)]);
        };
        let restore = painted
            .iter()
            .find(|(t, _)| t == "Restore")
            .map(|(_, r)| r.center())
            .expect("the ghost row's Restore");
        click(&mut f.app, restore);
        let now = fills_of(&f.app, f.ir);
        assert!(now.iter().any(|k| k.id == main[1].id), "restored by its id");
        assert!(now.contains(&own), "the own fill stays");
        // ↺ on the recoloured row.
        let changed_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 9, 30));
        let out = frame(&mut f.app, &ctx, Vec::new());
        let row = texts(&out)
            .into_iter()
            .find(|(t, _)| *t == changed_hex)
            .map(|(_, r)| r)
            .expect("the recoloured row");
        frame(
            &mut f.app,
            &ctx,
            vec![egui::Event::PointerMoved(row.center())],
        );
        let out = frame(&mut f.app, &ctx, Vec::new());
        let undo = texts(&out)
            .into_iter()
            .find(|(t, r)| {
                t == icon::ARROW_COUNTER_CLOCKWISE && (r.center().y - row.center().y).abs() < 4.0
            })
            .map(|(_, r)| r.center())
            .expect("↺ in the hovered row's slot");
        click(&mut f.app, undo);
        let now = fills_of(&f.app, f.ir);
        assert_eq!(now.iter().find(|k| k.id == main[0].id).unwrap(), &main[0]);
    }

    /// The Effects card reads its stack the same way (§15 D981, 4D–4E's *Inner
    /// shadow ↺*): the instance hides the main's first effect and removes the
    /// second; the second is a ghost row whose *Restore* brings it back, and the
    /// first's ↺ under the pointer takes the main's back.
    #[test]
    fn an_effect_stack_restores_and_resets_item_by_item() {
        use ondin_core::{Effect, EffectKind, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let [a, b, ..] = EffectKind::all_defaults();
        let main = keyed_by_position([Effect::new(a.clone()), Effect::new(b.clone())]);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetEffects {
                    id: r,
                    effects: main.clone(),
                }]))
        );
        let mut hidden = main[0].clone();
        hidden.visible = false;
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetEffects {
                    id: f.ir,
                    effects: vec![hidden],
                }]))
        );
        f.app.collapsed_panels.remove("Effects");
        f.app.session.selection.set_one(f.ir);
        let effects_of = |app: &OndinApp| app.session.doc.get(f.ir).unwrap().effects().to_vec();
        let click = |app: &mut OndinApp, at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(app, &ctx, vec![egui::Event::PointerMoved(at)]);
            frame(app, &ctx, vec![press(true)]);
            frame(app, &ctx, vec![press(false)]);
        };
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let restore = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Restore")
            .map(|(_, r)| r.center())
            .unwrap_or_else(|| panic!("no ghost row in {:?}", texts(&out)));
        click(&mut f.app, restore);
        assert!(
            effects_of(&f.app).iter().any(|k| k.id == main[1].id),
            "restored"
        );
        let out = frame(&mut f.app, &ctx, Vec::new());
        let row = texts(&out)
            .into_iter()
            .find(|(t, _)| t == a.label())
            .map(|(_, r)| r)
            .expect("the hidden effect's row");
        frame(
            &mut f.app,
            &ctx,
            vec![egui::Event::PointerMoved(row.center())],
        );
        let out = frame(&mut f.app, &ctx, Vec::new());
        let undo = texts(&out)
            .into_iter()
            .find(|(t, r)| {
                t == icon::ARROW_COUNTER_CLOCKWISE && (r.center().y - row.center().y).abs() < 4.0
            })
            .map(|(_, r)| r.center())
            .expect("↺ in the hovered effect row");
        click(&mut f.app, undo);
        assert_eq!(
            effects_of(&f.app)
                .iter()
                .find(|k| k.id == main[0].id)
                .unwrap(),
            &main[0]
        );
    }

    /// The main's rect pinned top-right and the instance's copy moved along x
    /// from where it is drawn — `keep_insets` turning the move into new insets on
    /// the copy. Answers the main's insets.
    fn pinned_and_moved(f: &mut F) -> ondin_core::Insets {
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let pin = ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(12.0)),
            top: Some(ondin_core::LengthPct::Px(8.0)),
            ..Default::default()
        };
        // Pinned with the placement written beside the insets, as the app's own
        // pin does (`toggle_pin` writes `baked_placement` with the `SetInsets`):
        // right 12 and top 8 in a 100-wide frame put
        // the 10-wide rect at (78, 8). Pinned by insets alone the stored transform
        // stays (0, 0), the copy's moved transform then differs from it on y as
        // well, and the X reset leaves that one unit behind — a fixture the app
        // cannot make, measured the first time this test ran.
        assert!(f.app.session.commit(Transaction(vec![
            Operation::SetInsets { id: r, insets: pin },
            Operation::SetTransform {
                id: r,
                transform: ondin_core::kurbo::Affine::translate((78.0, 8.0)),
            },
        ])));
        // **The instance wider than its main** — the case that needs the insets
        // in a reset. At the main's size, the main's stored transform is where
        // the main draws the rect, and `keep_insets` re-derives the main's insets
        // from it with or without them; at 260 wide the same point is `right
        // 172`, and only an explicit `SetInsets` keeps the pin at 12.
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetGeometry {
                    id: f.i,
                    geometry: ondin_core::GeometryPatch::Size(ondin_core::kurbo::Size::new(
                        260.0, 100.0
                    )),
                }]))
        );
        // Moved from where it is *drawn*, as a tool moves it — the stored
        // transform of a pinned layer is not its placement.
        let placed = f
            .app
            .session
            .resolved
            .used_local_of(f.app.session.doc.get(f.ir).unwrap());
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: f.ir,
                    transform: ondin_core::kurbo::Affine::translate((-17.25, 0.0)) * placed,
                }]))
        );
        assert_ne!(
            f.app.session.doc.get(f.ir).unwrap().insets(),
            &pin,
            "the fixture: the move re-pinned the copy"
        );
        pin
    }

    /// A press and release at `at`, the pointer arriving first.
    fn click_at(app: &mut OndinApp, ctx: &egui::Context, at: egui::Pos2) {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(app, ctx, vec![egui::Event::PointerMoved(at)]);
        frame(app, ctx, vec![press(true)]);
        frame(app, ctx, vec![press(false)]);
    }

    /// **A field reset on a pinned layer comes back exact** — the exact-equality
    /// risk asked of the field marks (§5.3d). X's reset writes the main's x **and
    /// the main's horizontal insets**, so `keep_insets` leaves the layer to it.
    /// Flip: drop the `SetInsets` from `transform_marks`' resets and *"the insets
    /// came back"* fails — the bare transform re-pinned where it lands in the
    /// wider instance.
    ///
    /// ⚠️ **Two first readings, both of the fixture.** Pinned by insets alone (the
    /// main's stored transform left at the origin, which the app's pin never does)
    /// the bare reset came back `right 90, top 0`; pinned honestly but at the
    /// main's own size, it came back exact *without* the insets, because the
    /// main's stored transform is then where the main draws it — so the flip
    /// stayed green. The instance has to be a different size for the insets to
    /// matter, which is why `pinned_and_moved` widens it.
    #[test]
    fn an_x_reset_on_a_pinned_layer_leaves_no_drift() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let pin = pinned_and_moved(&mut f);
        f.app.session.selection.set_one(f.ir);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let at = inks(&out)
            .into_iter()
            .find(|(t, _, c)| t == "X" && *c == theme::text::STRONG)
            .map(|(_, r, _)| r.center())
            .expect("a marked X");
        click_at(&mut f.app, &ctx, at);
        let doc = &f.app.session.doc;
        assert_eq!(
            doc.get(f.ir).unwrap().insets(),
            &pin,
            "the insets came back"
        );
        // The root's own size is the fixture's override; the copy has none left.
        assert_eq!(ondin_core::reset::overrides(doc, f.ir), Vec::new());
    }

    /// **And the Transform card's header reset**, which `gather_card_overrides`
    /// gives the main's insets for the same reason. Flip: dropping that append
    /// fails *"the insets came back"* with `right 172` — the main's point, pinned
    /// against the wider instance.
    #[test]
    fn a_transform_header_reset_on_a_pinned_layer_leaves_no_drift() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let pin = pinned_and_moved(&mut f);
        f.app.session.selection.set_one(f.ir);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let head = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "TRANSFORM")
            .map(|(_, r)| r.center())
            .expect("the Transform header");
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(head)]);
        let out = frame(&mut f.app, &ctx, Vec::new());
        let chip = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Reset transform")
            .map(|(_, r)| r.center())
            .expect("the hovered header's chip");
        click_at(&mut f.app, &ctx, chip);
        let doc = &f.app.session.doc;
        assert_eq!(
            doc.get(f.ir).unwrap().insets(),
            &pin,
            "the insets came back"
        );
        assert_eq!(ondin_core::reset::overrides(doc, f.ir), Vec::new());
    }

    /// **An instance root's insets are its own** — where it sits in its own
    /// parent (`propagate::is_placement`). The instance is pinned inside a frame
    /// and resized wider than its main; *Reset transform* puts the main's size back
    /// and must leave the instance's pin alone. Found by `arch-scribe` reading
    /// `c292b50`: that commit gave every Transform-card reset the **main's**
    /// insets, which for a root describe where the main sits in *its* parent — the
    /// instance was unpinned. Failed first with the main's (unset) insets written.
    #[test]
    fn a_transform_reset_keeps_an_instance_roots_own_pin() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let host = f.app.session.ids.mint();
        let root = f.app.session.doc.root();
        let pin = ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(5.0)),
            top: Some(ondin_core::LengthPct::Px(5.0)),
            ..Default::default()
        };
        assert!(f.app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: host,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(500.0, 500.0),
                },
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: f.i,
                new_parent: host,
                index: 0,
            },
        ])));
        assert!(f.app.session.commit(Transaction(vec![
            Operation::SetInsets {
                id: f.i,
                insets: pin
            },
            Operation::SetTransform {
                id: f.i,
                transform: ondin_core::kurbo::Affine::translate((395.0, 5.0)),
            },
            Operation::SetGeometry {
                id: f.i,
                geometry: ondin_core::GeometryPatch::Size(Size::new(260.0, 100.0)),
            },
        ])));
        f.app.session.selection.set_one(f.i);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let head = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "TRANSFORM")
            .map(|(_, r)| r.center())
            .expect("the Transform header");
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(head)]);
        let out = frame(&mut f.app, &ctx, Vec::new());
        let chip = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Reset transform")
            .map(|(_, r)| r.center())
            .expect("the size override offers the reset");
        click_at(&mut f.app, &ctx, chip);
        let node = f.app.session.doc.get(f.i).unwrap();
        assert!(
            matches!(node.kind(), NodeKind::Artboard { size } if *size == Size::new(100.0, 100.0)),
            "the main's size is back"
        );
        assert_eq!(node.insets(), &pin, "and the instance's own pin stays");
    }

    /// **A collapsed card keeps its count and offers no chip.** A collapsed card is
    /// one whole-card target registered after its header, which would cover the
    /// chip — hovered, it drew *Reset appearance* and a click expanded the card
    /// and reset nothing (`arch-scribe`, reading `panel_badged`). Flip: dropping
    /// `sense &&` in `section_head_full` draws the chip again.
    #[test]
    fn a_collapsed_card_counts_but_offers_no_reset() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetOpacity {
                    id: f.ir,
                    opacity: 0.5,
                }]))
        );
        f.app.collapsed_panels.insert("Appearance");
        f.app.session.selection.set_one(f.ir);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = texts(&out);
        let head = painted
            .iter()
            .position(|(t, _)| t == "APPEARANCE")
            .expect("the collapsed header");
        assert_eq!(painted[head + 1].0, "1", "the count stays");
        frame(
            &mut f.app,
            &ctx,
            vec![egui::Event::PointerMoved(painted[head].1.center())],
        );
        let out = frame(&mut f.app, &ctx, Vec::new());
        assert!(
            !texts(&out).iter().any(|(t, _)| t == "Reset appearance"),
            "no chip on a collapsed card"
        );
    }

    fn fills_of(app: &OndinApp, id: NodeId) -> Vec<ondin_core::Keyed<ondin_core::Fill>> {
        app.session.doc.get(id).unwrap().paint().fills.clone()
    }

    /// **A card header counts its overrides and resets them** (§15 D981, 4E). The
    /// instance's rect has its opacity overridden: the Appearance header shows `1`
    /// at rest; with the pointer on the header the count becomes *Reset
    /// appearance*, and a click there puts the main's opacity back — and leaves the
    /// card open, the header's own toggle lying under the chip. Flip: `card_of`
    /// mapping opacity nowhere fails *"the count after the label"* (the header has
    /// none, and the next text is the field's `50`). ⚠️ Dropping
    /// `section_head_full`'s `clicks.toggled = false` stays **green**: egui gives
    /// the press to the topmost target only, the chip registered after the header,
    /// so that line is a belt and the registration order is what holds it.
    #[test]
    fn a_card_header_counts_and_resets_its_overrides() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetOpacity {
                    id: f.ir,
                    opacity: 0.5,
                }]))
        );
        f.app.session.selection.set_one(f.ir);
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let painted = texts(&out);
        let head = painted
            .iter()
            .position(|(t, _)| t == "APPEARANCE")
            .unwrap_or_else(|| panic!("no Appearance header in {painted:?}"));
        assert_eq!(painted[head + 1].0, "1", "the count after the label");
        let on_head = painted[head].1.center();
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(on_head)]);
        let out = frame(&mut f.app, &ctx, Vec::new());
        let at = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Reset appearance")
            .map(|(_, r)| r.center())
            .expect("the hovered header offers the reset");
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut f.app, &ctx, vec![egui::Event::PointerMoved(at)]);
        frame(&mut f.app, &ctx, vec![press(true)]);
        frame(&mut f.app, &ctx, vec![press(false)]);
        assert_eq!(f.app.session.doc.get(f.ir).unwrap().opacity(), 1.0);
        assert!(
            !f.app.collapsed_panels.contains("Appearance"),
            "still open: the reset is the press, not the toggle"
        );
    }

    /// A set `Button` (`Size: Small, Large`) of two variants, each a frame holding
    /// a text `Label` bound to the set's text property *Label text*, and an
    /// instance of Small — adopted into a headless app (§15 D982).
    struct V {
        app: OndinApp,
        set: NodeId,
        small: NodeId,
        large: NodeId,
        label: NodeId,
        i: NodeId,
        ilabel: NodeId,
    }

    fn text_kind(content: &str) -> NodeKind {
        NodeKind::Text {
            content: content.into(),
            style: Box::new(ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 12.0,
                ..Default::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: ondin_core::TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    fn variants_fixture(ctx: &egui::Context) -> V {
        use ondin_core::variant::{PropKind, Property, VariantProp, VariantSet};
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xC6);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [set, small, label, large, llabel] = [(); 5].map(|_| ids.mint());
        let frame = |w| NodeKind::Artboard {
            size: Size::new(w, 30.0),
        };
        let create = |id, parent, index, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        let item = ids.mint_item();
        doc.apply(&Transaction(vec![
            create(set, root, 0, frame(300.0), "Button"),
            create(small, set, 0, frame(80.0), "Small"),
            create(label, small, 0, text_kind("Go"), "Label"),
            create(large, set, 1, frame(120.0), "Large"),
            create(llabel, large, 0, text_kind("Go"), "Label"),
            Operation::SetComponent {
                id: small,
                component: true,
            },
            Operation::SetComponent {
                id: large,
                component: true,
            },
            Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![VariantProp {
                        name: "Size".into(),
                        values: vec!["Small".into(), "Large".into()],
                    }],
                }),
            },
            Operation::SetVariant {
                id: small,
                values: vec!["Small".into()],
            },
            Operation::SetVariant {
                id: large,
                values: vec!["Large".into()],
            },
            Operation::SetProperties {
                id: set,
                props: vec![ondin_core::Keyed::new(
                    item,
                    Property {
                        name: "Label text".into(),
                        kind: PropKind::Text,
                        bound: vec![label, llabel],
                        filter: String::new(),
                    },
                )],
            },
        ]))
        .expect("a set of two variants");
        let (tx, made) = ondin_core::insert_subtrees(
            &doc,
            &mut ids,
            &[Placement {
                nodes: doc.capture_subtree(small).unwrap(),
                parent: root,
                index: None,
            }],
            Default::default(),
        );
        doc.apply(&tx).expect("an instance of Small");
        let i = made[0];
        let ilabel = doc.get(i).unwrap().children()[0];
        app.session.adopt_document(doc, None);
        V {
            app,
            set,
            small,
            large,
            label,
            i,
            ilabel,
        }
    }

    fn content(app: &OndinApp, id: NodeId) -> String {
        match app.session.doc.get(id).unwrap().kind() {
            NodeKind::Text { content, .. } => content.clone(),
            _ => panic!("not text"),
        }
    }

    /// Step 7's faces (§15 D982): a set, a variant, a layer inside a variant, and
    /// an instance of a variant.
    #[test]
    fn each_variant_selection_gets_its_face() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        assert!(matches!(face_of(&mut v.app, &[v.set]), Some(Face::Set { set }) if set == v.set));
        assert!(matches!(
            face_of(&mut v.app, &[v.small]),
            Some(Face::Variant { set, instances: 1, .. }) if set == v.set
        ));
        assert!(matches!(
            face_of(&mut v.app, &[v.label]),
            Some(Face::InMain { node }) if node == v.label
        ));
        assert!(matches!(
            face_of(&mut v.app, &[v.i]),
            Some(Face::Instance { main, .. }) if main == v.small
        ));
    }

    /// **A property counts as a property, not as an override** (4A): the
    /// instance's label text set through the property reads *1 property* in the
    /// summary and nothing as an override; dimming the label as well (opacity
    /// 0.5) adds *1 override*. Flip, run: dropping `PropDrift::units` from `drift_summary`'s
    /// subtraction paints *1 property · 1 override* in the first case.
    #[test]
    fn the_summary_counts_a_property_apart_from_the_overrides() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        let p = ondin_core::variant::instance_properties(&v.app.session.doc, v.i)[0]
            .value
            .clone();
        let ops = ondin_core::variant::set_property(
            &v.app.session.doc,
            &[v.i],
            &p,
            &ondin_core::variant::PropValue::Text("Sign up".into()),
        );
        assert!(v.app.session.commit(Transaction(ops)));
        assert_eq!(content(&v.app, v.ilabel), "Sign up");
        v.app.session.selection.set_one(v.i);
        let mut out = frame(&mut v.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut v.app, &ctx, Vec::new());
        }
        let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(painted.contains(&"1 property".to_string()), "{painted:?}");
        assert!(
            painted.contains(&"Size".to_string()),
            "the variant dropdown's label"
        );
        assert!(
            painted.contains(&"Label text".to_string()),
            "the property row"
        );

        assert!(
            v.app
                .session
                .commit(Transaction(vec![Operation::SetOpacity {
                    id: v.ilabel,
                    opacity: 0.5,
                }]))
        );
        for _ in 0..3 {
            out = frame(&mut v.app, &ctx, Vec::new());
        }
        let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(
            painted.contains(&"1 property · 1 override".to_string()),
            "{painted:?}"
        );
    }

    /// The layers panel's two marks (§15 D982, 2A and 2C): a collapsed set's row
    /// carries its variant count where a frame's carries its size, and a layer
    /// bound to a property carries `{}`. Flip, run: answering `false` for `bound`
    /// fails the second assertion.
    #[test]
    fn the_layers_panel_counts_a_folded_set_and_marks_a_bound_layer() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        let tree = |app: &mut OndinApp| {
            let mut out = None;
            for _ in 0..3 {
                out = Some(ctx.run_ui(Default::default(), |ui| app.layers_tree(ui)));
            }
            texts(&out.expect("drawn"))
                .into_iter()
                .map(|(t, _)| t)
                .collect::<Vec<_>>()
        };
        v.app.collapsed.insert(v.set);
        let painted = tree(&mut v.app);
        assert!(painted.contains(&"2 variants".to_string()), "{painted:?}");
        v.app.collapsed.clear();
        let painted = tree(&mut v.app);
        assert!(
            painted
                .iter()
                .any(|t| t == crate::theme::icon::BRACKETS_CURLY),
            "{painted:?}"
        );
    }

    /// **A Type reset restyles a live text session** (§15 D981's amendment —
    /// `commit_reset`'s second point, fixed in session 49 and owed a test since):
    /// a copy's overridden size reset while its text is being edited reaches the
    /// **editor**, so its next keystroke draws in the main's size and not the one
    /// the reset took away.
    ///
    /// ⚠️ **Two cuts of this test whose flips did not bite**, both reading the
    /// node's *style*: straight after the reset (a session that has not drawn has
    /// no preview, so the canvas read the document), and after `preview_session`
    /// (whose `SetText` writes content and spans, the style staying the
    /// document's). The stale thing is the **editor's own layout** — what
    /// `preview_session` installs as the shaped text, and what the caret moves
    /// through — so that is what this reads. Flip, run: `commit_reset` without its
    /// `text_session_restyled_after` fails at the last assertion, the height
    /// unchanged at 30pt's.
    #[test]
    fn a_type_reset_restyles_a_live_text_session() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        let label = v.ilabel;
        let size_of = |app: &OndinApp| match app.session.display_node(label).unwrap().kind() {
            NodeKind::Text { style, .. } => style.font_size,
            _ => panic!("the copy is text"),
        };
        let style = match v.app.session.doc.get(label).unwrap().kind() {
            NodeKind::Text { style, .. } => (**style).clone(),
            _ => panic!("the copy is text"),
        };
        assert!(
            v.app
                .session
                .commit(Transaction(vec![Operation::SetTextStyle {
                    id: label,
                    style: ondin_core::TextStyle {
                        font_size: 30.0,
                        ..style
                    },
                    spans: None,
                }]))
        );
        v.app.session.selection.set_one(label);
        v.app.begin_edit_text(Some(label), None);
        assert!(v.app.text.is_some(), "the fixture: a live session");
        assert_eq!(size_of(&v.app), 30.0, "the fixture: the override on screen");
        let reset: Vec<Operation> = ondin_core::reset::overrides(&v.app.session.doc, label)
            .into_iter()
            .map(|o| o.reset)
            .filter(|op| matches!(op, Operation::SetTextStyle { .. }))
            .collect();
        assert_eq!(reset.len(), 1, "the size's reset");
        let height = |app: &OndinApp| app.text.as_ref().unwrap().editor.text_layout().size.height;
        let before = height(&v.app);
        v.app.commit_reset(Transaction(reset));
        assert_eq!(size_of(&v.app), 12.0, "the document took the main's size");
        // What the editor lays out — and so what its next keystroke draws
        // (`preview_session` installs the editor's own shaped layout).
        assert!(
            height(&v.app) < before,
            "the editor laid out at 12, not 30: {} against {before}",
            height(&v.app)
        );
    }

    /// **The layers panel's component marks** (§15 D981): an instance's row wears
    /// the outline hexagon (a main's is the filled one, painted rather than set,
    /// so it is not text); an override puts a dot on the row that holds it, and
    /// on a **collapsed** instance the dot bubbles up to it; a layer of the
    /// instance's own wears `+`; and an untouched instance wears neither.
    ///
    /// Flip: the collapsed arm reading only the row's own overrides leaves the
    /// collapsed instance without its dot.
    #[test]
    fn the_layers_panel_marks_overrides_and_local_layers() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        // Each mark's shapes: a dot is a 2pt circle, a `+` two 1.2pt strokes.
        let marks = |app: &mut OndinApp| {
            let mut out = None;
            for _ in 0..3 {
                out = Some(ctx.run_ui(Default::default(), |ui| app.layers_tree(ui)));
            }
            let out = out.expect("drawn");
            fn walk(s: &egui::Shape, dots: &mut usize, strokes: &mut usize) {
                match s {
                    egui::Shape::Circle(c) if c.radius == 2.0 => *dots += 1,
                    egui::Shape::LineSegment { stroke, .. } if stroke.width == 1.2 => *strokes += 1,
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, dots, strokes)),
                    _ => {}
                }
            }
            let (mut dots, mut strokes) = (0, 0);
            for s in &out.shapes {
                walk(&s.shape, &mut dots, &mut strokes);
            }
            let hexagons = texts(&out)
                .into_iter()
                .filter(|(t, _)| t == crate::theme::icon::HEXAGON)
                .count();
            (dots, strokes / 2, hexagons)
        };
        let (dots, pluses, hexagons) = marks(&mut v.app);
        assert_eq!((dots, pluses), (0, 0), "an untouched instance");
        assert_eq!(hexagons, 1, "one instance row, the outline hexagon");
        assert!(v.app.session.commit(Transaction(vec![Operation::SetName {
            id: v.ilabel,
            name: "Mine".into(),
        }])));
        assert_eq!(marks(&mut v.app).0, 1, "the override's own row");
        v.app.collapsed.insert(v.i);
        assert_eq!(marks(&mut v.app).0, 1, "bubbled to the collapsed instance");
        v.app.collapsed.clear();
        let extra = v.app.session.ids.mint();
        assert!(
            v.app
                .session
                .commit(Transaction(vec![Operation::CreateNode {
                    id: extra,
                    parent: v.i,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: ondin_core::kurbo::Size::new(4.0, 4.0),
                        corner_radii: Default::default(),
                    },
                    transform: None,
                    name: None,
                }]))
        );
        assert_eq!(marks(&mut v.app).1, 1, "a layer of the instance's own");
    }

    /// The set's card draws its counts and its property, and the context menu's
    /// verbs land: *Add variant* after a third value takes it, and *Reset Label
    /// text* puts the property back.
    #[test]
    fn the_set_card_and_the_menu_verbs() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        v.app.session.selection.set_one(v.set);
        let mut out = frame(&mut v.app, &ctx, Vec::new());
        for _ in 0..3 {
            out = frame(&mut v.app, &ctx, Vec::new());
        }
        let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(
            painted.contains(&"Component set".to_string()),
            "{painted:?}"
        );
        assert!(
            painted.contains(&"2 variants · 1 instance".to_string()),
            "{painted:?}"
        );
        assert!(
            painted.contains(&"PROPERTIES".to_string()),
            "the set's own properties card"
        );

        let tx = ondin_core::variant::add_value(&v.app.session.doc, v.set, 0, "Huge").unwrap();
        assert!(v.app.session.commit(tx));
        v.app.session.selection.set_one(v.large);
        v.app.add_variant();
        let new = v.app.session.selection.single().unwrap();
        assert_eq!(v.app.session.doc.get(new).unwrap().variant(), ["Huge"]);
        assert_eq!(v.app.session.doc.get(new).unwrap().name(), "Huge");

        let p = ondin_core::variant::instance_properties(&v.app.session.doc, v.i)[0]
            .value
            .clone();
        let ops = ondin_core::variant::set_property(
            &v.app.session.doc,
            &[v.i],
            &p,
            &ondin_core::variant::PropValue::Text("Sign up".into()),
        );
        assert!(v.app.session.commit(Transaction(ops)));
        v.app.session.selection.set_one(v.ilabel);
        assert_eq!(
            v.app.overridden_property_of_selection().map(|p| p.name),
            Some("Label text".into())
        );
        v.app.reset_selected_property();
        assert_eq!(content(&v.app, v.ilabel), "Go");
        assert_eq!(v.app.overridden_property_of_selection(), None);
    }

    /// *Combine as variants* on two loose mains makes a set and selects it — and
    /// the context menu no longer offers *Create component* on them, which would
    /// nest mains in a main and be refused.
    #[test]
    fn combining_two_mains_selects_the_new_set() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.m);
        f.app.duplicate_as_component();
        let copy = f.app.session.selection.single().unwrap();
        f.app.session.selection.set(vec![f.m, copy]);
        assert!(f.app.combinable_mains().is_some());
        f.app.combine_as_variants();
        let set = f.app.session.selection.single().unwrap();
        assert!(ondin_core::variant::is_set(&f.app.session.doc, set));
        assert_eq!(
            ondin_core::variant::variants(&f.app.session.doc, set).len(),
            2
        );
        // The instance of the first main is an instance of a variant now, named as
        // it was.
        assert_eq!(f.app.session.doc.get(f.i).unwrap().link(), Some(f.m));
    }
}
