//! The **Component** card — what the selection is to the components machinery
//! (`architecture.md` §5.3d): a main's instance count and its two verbs, an
//! instance's link to its main, its drift and the reset family, and the summaries
//! several selected instances get. A layer *inside* an instance gets one line, not
//! a card, drawn as a row of the identity card (`ComponentPart::Identity`), as are
//! a mixed selection's line and the binding line.
//!
//! **Counts tell you what each reset will do; a reset with nothing to do is
//! disabled, not hidden.** The counts are `reset::Drift`, cached per session
//! revision (`OndinApp::drift_cache`), since the card is read every frame and a
//! drift is a walk of the instance; the property readings sit beside them on the
//! same key ([`PropCache`], `OndinApp::prop_cache`).
//!
//! ⚠️ **The drift summary counts every difference a *Reset all* would undo** —
//! fields, removed children and order. Counting fields alone would hide a removed
//! child, and an instance whose only drift is a deleted layer would read as
//! untouched.
//!
//! **The main's glyph is the filled hexagon**, drawn by `ui::paint_hexagon_filled`
//! since the bundled Phosphor Regular has none; a main's and a variant's face carry
//! it, an instance's the outline one.

use crate::app::OndinApp;
use crate::theme::{self, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::component;
use ondin_core::reset::{Drift, Kind};
use ondin_core::{NodeId, Operation, Transaction};

/// One overridden field's mark: its tooltip, *Reset to main · 168*, and the
/// transaction that writes the main's value back into that field alone.
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

/// The card's faces, one per kind of selection.
enum Face {
    /// A main component, and how many instances it has.
    Main { name: String, instances: usize },
    /// A component set — its own card, *Variants*.
    Set { set: NodeId },
    /// A variant: a main inside a set, named by its values.
    Variant {
        main: NodeId,
        set: NodeId,
        set_name: String,
        instances: usize,
    },
    /// A layer inside a main, which can be bound to its properties.
    InMain { node: NodeId },
    /// One instance root — `root`, which the face was decided on: the card's
    /// rows read it rather than the raw selection, which may hold a layer inside
    /// it too, in either order.
    Instance {
        root: NodeId,
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
        /// set: what earns them the variant and property rows.
        shared: Option<Vec<NodeId>>,
    },
    /// Instances among ordinary layers — summarised, with a way to narrow to them.
    Mixed { roots: Vec<NodeId>, others: usize },
}

/// An instance's **properties** apart from its other overrides (*2 properties ·
/// 1 override*): how many of its component properties differ from the main, and
/// how many of `Drift::fields`' units are those properties' fields — counted once,
/// as properties, and not again as overrides.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PropDrift {
    pub(crate) props: usize,
    pub(crate) units: usize,
    /// Whether the main defines any property at all — what puts *Reset
    /// properties* in the overflow (disabled at zero), and leaves it out for a
    /// component with none.
    pub(crate) defined: bool,
}

/// [`PropDrift`] for the instance rooted at `root`.
pub(crate) fn prop_drift(doc: &ondin_core::Document, root: NodeId) -> PropDrift {
    use ondin_core::variant::{self, PropKind};
    #[cfg(test)]
    count(|r| r.drift += 1);
    let defined = !variant::instance_properties(doc, root).is_empty();
    // The instance's own properties, then the shown rows of the nested instances
    // its card shows, each counting as a property.
    let props = variant::instance_properties(doc, root)
        .iter()
        .filter(|p| variant::property_state(doc, root, p).is_some_and(|(_, o)| o))
        .count()
        + variant::shown_rows_overridden(doc, root);
    let units = variant::property_fields(doc, root)
        .into_iter()
        .filter(|(c, kind)| {
            ondin_core::reset::overrides(doc, *c)
                .iter()
                .any(|o| match kind {
                    PropKind::Boolean => matches!(
                        o.reset,
                        Operation::SetVisible { .. } | Operation::SetVisibleVar { .. }
                    ),
                    // The content's binding counts as the visibility's does: a copy
                    // bound apart from its main differs by it alone. Not the
                    // family's, which rides in the same op.
                    PropKind::Text => match &o.reset {
                        Operation::SetText { .. } => true,
                        Operation::SetTextVars { vars, .. } => {
                            doc.get(*c).is_some_and(|n| n.text_vars()[0] != vars[0])
                        }
                        _ => false,
                    },
                    PropKind::Swap => matches!(o.reset, Operation::SetSwap { .. }),
                    PropKind::Nested => false,
                })
        })
        .count();
    PropDrift {
        props,
        units,
        defined,
    }
}

/// What the Component card computed on this thread: `prop_drift`, and the
/// property rows' `variant::property_state` and `variant::reset_property` —
/// counted so a test can say an idle frame computes none of them. A count and not
/// a clock, which would not hold as a timing assertion.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Reads {
    pub(crate) drift: usize,
    pub(crate) state: usize,
    pub(crate) reset: usize,
}

#[cfg(test)]
thread_local! {
    /// This thread's `Reads` so far.
    pub(crate) static READS: std::cell::Cell<Reads> = std::cell::Cell::new(Reads::default());
}

/// Add to this thread's `Reads`.
#[cfg(test)]
pub(crate) fn count(f: impl FnOnce(&mut Reads)) {
    READS.with(|r| {
        let mut v = r.get();
        f(&mut v);
        r.set(v);
    });
}

/// A property row's chip as [`PropCache::chips`] holds it: the label and the
/// tooltip, `None` where no chip is drawn.
pub(super) type PropChip = Option<(String, String)>;

/// The Component card's **property readings**, as of the session revision they
/// were read at — the twin of `OndinApp::drift_cache`: each reading is a walk of
/// an instance and the card is drawn every frame. Keyed on the revision, so an
/// edit, undo or redo empties it; a preview never writes the document.
#[derive(Default)]
pub(crate) struct PropCache {
    /// The revision the readings below are of; `None` before the first.
    rev: Option<u64>,
    /// [`prop_drift`] per instance root.
    drift: std::collections::HashMap<NodeId, PropDrift>,
    /// `variant::property_state` per root and property.
    pub(super) states: std::collections::HashMap<
        (NodeId, ondin_core::ItemId),
        Option<(ondin_core::variant::PropValue, bool)>,
    >,
    /// An overridden property row's mark — its tooltip and its reset — per set
    /// of roots and property: built only for a row that is overridden.
    pub(super) marks:
        std::collections::HashMap<(Vec<NodeId>, ondin_core::ItemId), (String, Vec<Operation>)>,
    /// A boolean or text property row's chip — its label (the variable's name,
    /// or *Mixed* over several) and its tooltip, or `None` unless every layer it
    /// views is bound — per set of roots and property: a walk of every root, as
    /// the states are.
    pub(super) chips: std::collections::HashMap<(Vec<NodeId>, ondin_core::ItemId), PropChip>,
}

impl PropCache {
    /// The cache as of revision `rev`, emptied first if it holds another's.
    pub(super) fn at(&mut self, rev: u64) -> &mut Self {
        if self.rev != Some(rev) {
            *self = Self {
                rev: Some(rev),
                ..Self::default()
            };
        }
        self
    }
}

/// Which half of the components inspector a call draws: the rows that
/// sit **inside the identity card** — a child's *In Card instance* or *Local to
/// this instance*, a mixed selection's line, and the binding line — or the
/// **cards** under it, *Component*, *Variants* and *Properties*.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ComponentPart {
    Identity,
    Cards,
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
    /// A kit instance's ⋯ rows (§5.3e): open the kit at its main, accept the
    /// waiting update of the instance's own kit, and reveal the vendored main as a
    /// local one, after a confirmation.
    GoToKitMain,
    AcceptKitUpdate(String),
    MakeLocal(NodeId),
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

    /// [`prop_drift`] at instance `root`, from [`PropCache`] while the document
    /// is unchanged.
    pub(crate) fn prop_drift_of(&mut self, root: NodeId) -> PropDrift {
        let rev = self.session.revision();
        let doc = &self.session.doc;
        *self
            .prop_cache
            .at(rev)
            .drift
            .entry(root)
            .or_insert_with(|| prop_drift(doc, root))
    }

    /// Gather [`OndinApp::card_overrides`] for this frame: every selected linked
    /// layer's overrides (`reset::overrides`), grouped by the card that shows them.
    /// Over several instances a card counts and resets every one of them.
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
        // (`OndinApp::transform_marks`), or `keep_insets` re-pins the write where it
        // lands. ⚠️ **Not an instance root's**: its insets are its own placement
        // (`propagate::is_placement`), written back unchanged. And only a node whose
        // ops **place** it: a pivot alone moves nothing. The append is
        // `placement_stated`'s.
        if let Some((_, _, ops)) = cards.iter_mut().find(|(t, ..)| *t == "Transform") {
            *ops = placement_stated(doc, Transaction(std::mem::take(ops))).0;
        }
        self.card_overrides = cards;
        self.field_overrides = fields;
    }

    /// An overridden field put back: `mark`'s reset, committed when its
    /// ↺ was clicked (`hit`), through [`OndinApp::commit_reset`].
    pub(super) fn reset_marked(&mut self, hit: bool, mark: &Option<OverrideMark>) {
        if let (true, Some(m)) = (hit, mark) {
            self.commit_reset(m.tx.clone());
        }
    }

    /// `tx`, a reset, with the item of every layer it resizes stated beside the
    /// size — the reset's own where it resets the item, else the layer's unchanged
    /// — so `build::flex_holds` does not take the reset for the hand's resize
    /// ([`OndinApp::commit_reset`]'s first point). The context menu's reset
    /// (`OndinApp::reset_tx`) takes it too.
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

    /// `tx`, a reset, with every placed copy's insets stated beside it — the
    /// module's `placement_stated`, which says why. The context menu's and the
    /// Component card's resets take it (`OndinApp::reset_tx`), as the Transform
    /// header's does.
    pub(crate) fn placement_stated(&self, tx: Transaction) -> Transaction {
        placement_stated(&self.session.doc, tx)
    }

    /// Commit an instance's reset from the inspector (a field's ↺, a card header's
    /// chip, the Component card's resets) through `commit_edit`, plus two things:
    /// - **A reset is not a resize.** `build::flex_holds` holds a size written with
    ///   no item beside it, so the item is stated in the same transaction
    ///   ([`OndinApp::items_stated`]).
    /// - **A live text session on a layer the reset restyles adopts it**
    ///   (`text_session_restyled_after`), **but not on a content reset**
    ///   (`SetText`): the editor holds content a commit ahead of the document's.
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
        // A caller that knows the field has named the step already.
        if !self.session.has_step_name() {
            self.session
                .name_step("Reset", icon::ARROW_COUNTER_CLOCKWISE);
        }
        self.commit_edit(tx.clone());
        if restyles {
            self.text_session_restyled_after(&tx);
        }
    }

    /// Layer `id`'s field resets — the `reset` operations of its
    /// `reset::overrides`, which a mark reads its source's value out of: this
    /// frame's cache ([`OndinApp::field_overrides`]) for a selected layer, else
    /// computed — a card's subject outside the selection, the Mask card's masks
    /// inside a selected group.
    pub(super) fn resets_of(&self, id: NodeId) -> std::borrow::Cow<'_, [Operation]> {
        match self.field_overrides.iter().find(|(n, _)| *n == id) {
            Some((_, r)) => std::borrow::Cow::Borrowed(r.as_slice()),
            None => std::borrow::Cow::Owned(
                ondin_core::reset::overrides(&self.session.doc, id)
                    .into_iter()
                    .map(|o| o.reset)
                    .collect(),
            ),
        }
    }

    /// A **sub-field's** override mark over `subjects`: one number or choice inside
    /// a struct payload, which `reset::overrides` compares only as a whole value.
    /// `theirs` reads the source's whole value out of this frame's field resets
    /// ([`OndinApp::field_overrides`]), so a field the comparison skips is never
    /// marked. Over several subjects the mark shows if any differs and its reset
    /// writes each that does; the tooltip names the first one's main value.
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
            let resets = self.resets_of(*id);
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
            // source has no place for (a flex field where the main is a grid)
            // differs, and `put` has nowhere to take it from.
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
            let props = self.prop_drift_of(root);
            return Some(Face::Instance {
                root,
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
        // One owner — one main, or variants of one set — is one component.
        let mut owners: Vec<NodeId> = mains
            .iter()
            .filter_map(|m| ondin_core::variant::owner_of_main(doc, *m))
            .collect();
        owners.sort();
        owners.dedup();
        let shared = (owners.len() == 1).then(|| roots.clone());
        // Variants of one set read as instances of the set, named for it.
        let main = main.or_else(|| {
            let o = *owners.first().filter(|_| owners.len() == 1)?;
            doc.get(o)?.set()?;
            Some((o, name(o)))
        });
        let props =
            roots
                .iter()
                .map(|r| self.prop_drift_of(*r))
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

    /// The Component card — see the module doc.
    pub(super) fn inspector_component(&mut self, ui: &mut egui::Ui, part: ComponentPart) {
        let Some(face) = self.component_face() else {
            return;
        };
        let mut act = None;
        if part == ComponentPart::Identity {
            // **Inside the identity card, under its second row**: each of these
            // is a fact about what the layer *is*, which is that card's subject.
            match &face {
                Face::Child { main_name, linked } => {
                    child_line(ui, main_name, *linked, &mut act);
                }
                Face::Mixed { roots, others } => mixed_line(ui, roots, *others, &mut act),
                Face::InMain { node } => self.bind_line(ui, *node),
                _ => {}
            }
            // A nested instance inside a main is an instance root, so it takes the
            // instance's face — and its binding line too, which is where a swap
            // property is made.
            if let Face::Instance { .. } = &face
                && let Some(root) = self.session.selection.single()
                && ondin_core::variant::owner_above(&self.session.doc, root).is_some()
            {
                self.bind_line(ui, root);
            }
            self.component_act(act);
            return;
        }
        match &face {
            Face::Child { .. } | Face::Mixed { .. } | Face::InMain { .. } => {}
            Face::Set { set } => {
                let set = *set;
                // The set's variant count sits in the card's title, *Variants 4*.
                let n = ondin_core::variant::variants(&self.session.doc, set).len();
                self.panel_head(ui, "Variants", Some(n), None, None, |app, ui| {
                    app.set_body(ui, set)
                });
            }
            Face::Main { .. }
            | Face::Variant { .. }
            | Face::Instance { .. }
            | Face::Instances { .. } => {
                self.panel(ui, "Component", None, |app, ui| match &face {
                    Face::Main { name, instances } => {
                        if name_line(ui, None, name, Some(*instances)) {
                            act = Some(Act::SelectAllInstances);
                        }
                        if let Some(id) = app.session.selection.single() {
                            app.description_field(ui, id);
                        }
                        main_tail(ui, &mut act);
                    }
                    // *Variant in Button* over the derived, read-only name, a
                    // dropdown per property, then *Duplicate*; the name carries
                    // its instance count.
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
                        if name_and_count(ui, &name, Some(*instances)).clicked {
                            act = Some(Act::SelectAllInstances);
                        }
                        app.description_field(ui, *main);
                        app.variant_rows(ui, *main);
                        main_tail(ui, &mut act);
                    }
                    Face::Instance {
                        root,
                        main,
                        main_name,
                        drift,
                        props,
                    } => {
                        let summary = drift_summary(*drift, *props);
                        let link = Some((*main, main_name.as_str()));
                        // One line, the main's name alone; the outline hexagon says
                        // it is an instance.
                        heading(ui, false, "", link, None, &summary, &mut act);
                        description_note(&app.session.doc, *main, ui);
                        // A kit instance names where its main lives.
                        let kit = app.kit_line(*main);
                        if let Some(k) = &kit {
                            crate::kits::card::source_line(ui, k);
                        }
                        // The root the face was decided on, not the selection.
                        app.chosen_by_note(ui, *root);
                        app.instance_rows(ui, &[*root]);
                        app.show_switch(ui, *root);
                        reset_row(ui, *drift, *props, true, kit.as_ref(), &mut act);
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
                                reset_row(ui, *drift, *props, true, None, &mut act);
                            }
                            // No single main to go to, so no name link and no
                            // overflow of counted resets.
                            None => {
                                let title = format!("Instances of {mains} components");
                                heading(ui, false, "", None, Some(&title), &summary, &mut act);
                                reset_row(ui, *drift, *props, false, None, &mut act);
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
        // A main's or a set's own properties, under its card.
        self.inspector_properties(ui);
        self.component_act(act);
    }

    /// A main's or a set's description, typed under its face: wraps as it grows,
    /// buffered in `desc_edit` and committed once, on defocus.
    pub(super) fn description_field(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(saved) = self
            .session
            .doc
            .get(id)
            .map(|n| n.description().to_string())
        else {
            return;
        };
        let mut text = match &self.desc_edit {
            Some((d, buf)) if *d == id => buf.clone(),
            _ => saved.clone(),
        };
        ui.add_space(DESC_GAP);
        let resp = ui::field_frame()
            .inner_margin(egui::Margin::symmetric(ui::FIELD_PAD_X as i8, 5))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .id(ui.id().with(("description", id)))
                        .frame(egui::Frame::NONE)
                        .desired_rows(1)
                        .desired_width(f32::INFINITY)
                        .font(egui::FontId::proportional(DESC_PT))
                        .hint_text(egui::RichText::new("Add a description").size(DESC_PT)),
                )
            })
            .inner;
        if resp.gained_focus() || resp.changed() {
            self.desc_edit = Some((id, text.clone()));
        }
        // The `take` sits before `defocus_commits`, so Escape still drops the buffer.
        if resp.lost_focus()
            && let Some((d, val)) = self.desc_edit.take()
            && d == id
            && ui::defocus_commits(&resp)
        {
            let text = val.trim().to_string();
            if text != saved {
                self.session.name_step("Description", icon::PENCIL_SIMPLE);
                self.commit_edit(Transaction(vec![Operation::SetDescription {
                    of: ondin_core::Described::Node(id),
                    text,
                }]));
            }
        }
    }

    /// What a click on either part of the components inspector asked for, done.
    fn component_act(&mut self, act: Option<Act>) {
        match act {
            None => {}
            Some(Act::ResetProperties) => {
                let doc = &self.session.doc;
                let roots: Vec<NodeId> =
                    ondin_core::build::outermost(doc, self.session.selection.ids())
                        .into_iter()
                        .filter(|id| component::instance_root(doc, *id) == Some(*id))
                        .collect();
                // Shown rows included.
                let ops: Vec<Operation> = roots
                    .iter()
                    .flat_map(|r| ondin_core::variant::reset_all_properties(doc, *r))
                    .collect();
                if !ops.is_empty() {
                    self.commit_reset(Transaction(ops));
                }
            }
            // *Reset fields* leaves the properties alone: their fields are
            // counted, and reset, as properties.
            Some(Act::Reset(Kind::Fields)) => {
                if let Some(tx) = self.reset_tx(Kind::Fields) {
                    let doc = &self.session.doc;
                    use ondin_core::variant::PropKind;
                    // The roots `reset_tx` scoped (the outermost selected), not every
                    // layer's nearest instance, or a nested copy filters out the
                    // very reset the row counted.
                    let bound: std::collections::HashSet<(NodeId, PropKind)> =
                        ondin_core::build::outermost(doc, self.session.selection.ids())
                            .into_iter()
                            .filter_map(|id| component::instance_root(doc, id))
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
                    if kept.is_empty() {
                        // Every difference was a property's, which *Reset fields*
                        // leaves alone: said, not a click that does nothing.
                        self.session.info("Only component properties differ here");
                    } else {
                        self.commit_reset(Transaction(kept));
                    }
                }
            }
            Some(Act::Select(ids)) => self.session.selection.set(ids),
            Some(Act::GoToMain(main)) => self.select_main(main),
            Some(Act::GoToSource) => self.go_to_main(),
            // Through `commit_reset`, which restyles a live text session too.
            // *Detach* below changes no pixel and commits as its verb does.
            Some(Act::Reset(kind)) => {
                if let Some(tx) = self.reset_tx(kind) {
                    self.commit_reset(tx);
                }
            }
            Some(Act::Detach) => self.detach_instances(),
            Some(Act::SelectAllInstances) => self.select_all_instances(),
            Some(Act::DuplicateAsComponent) => self.duplicate_as_component(),
            Some(Act::GoToKitMain) => self.go_to_kit_main(),
            Some(Act::AcceptKitUpdate(kit)) => {
                // The instance's own kit, not every kit waiting: the row is on
                // one instance's card.
                for w in self.kits_waiting(false) {
                    if w.kit.id == kit {
                        self.kit_shelf.ask(&w.kit);
                    }
                }
                self.accept_kit_updates(Some(&kit));
            }
            Some(Act::MakeLocal(top)) => self.making_local = Some(top),
        }
    }

    /// Whether the instance layer `id` removed any of its source's items from the
    /// list `read` takes — the ghost rows that keep a list card with no item of
    /// its own from reading, and closing, as empty. `false` for `None`
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
    /// reads its rows' trailing slots and its ghost rows against.
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

/// A list item's reserved 14-pt trailing slot: empty while the item follows its
/// main, the dot when it differs, `+` when it is the instance's own, ↺ on an
/// overridden item whose row is `hot`; answers whether that was clicked.
/// ⚠️ **`hot` comes from the caller**: this right-to-left run's own box begins
/// past the row's label.
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

/// What leads a [`ghost_row`]'s label — the live row's own lead, so the two line
/// up down a list.
#[derive(Clone, Copy)]
pub(super) enum GhostLead<'a> {
    /// A paint row's: the grip's column left blank, then a dimmed chip where the
    /// swatch sits.
    Chip,
    /// An effect row's: its kind's glyph.
    Glyph(&'a str),
    /// Nothing: the label at the field's inset (an export, a layout grid).
    Plain,
}

/// A **ghost row**: an item the instance removed while its main still has it,
/// drawn dashed and dim with a restore button in the eye's column (a
/// [`GhostLead::Plain`] row gets one too), laid out as the live row is with 3px
/// corners (the live field's are 5). Answers whether restore was clicked.
/// ⚠️ **Lists only**: a deleted child *layer* has only the card's *Restore
/// removed children*.
pub(super) fn ghost_row(ui: &mut egui::Ui, lead: GhostLead<'_>, label: &str) -> bool {
    let h = ui::CONTROL_H;
    let gap = ui::CARD_COL_GAP;
    let width = ui.available_width() - h - gap;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
        let p = ui.painter();
        let dash = egui::Stroke::new(1.0, theme::text::FAINT);
        p.extend(egui::Shape::dashed_line(
            &rounded_outline(rect.shrink(0.5), 3.0),
            dash,
            3.0,
            3.0,
        ));
        // The field's content edge: its hairline, then `FIELD_PAD_X`.
        let x0 = rect.left() + 1.0 + ui::FIELD_PAD_X;
        let y = rect.center().y;
        let text_x = match lead {
            GhostLead::Chip => {
                // The grip's 11pt and the row's 9pt spacing, then the 16pt chip.
                let chip = egui::Rect::from_min_size(
                    egui::pos2(x0 + 11.0 + 9.0, y - 8.0),
                    egui::vec2(16.0, 16.0),
                );
                p.rect_filled(chip, 4.0, theme::text::FAINT.gamma_multiply(0.5));
                chip.right() + 9.0
            }
            GhostLead::Glyph(g) => {
                let r = p.text(
                    egui::pos2(x0, y),
                    egui::Align2::LEFT_CENTER,
                    g,
                    theme::icon_font(15.0),
                    theme::text::FAINT,
                );
                r.right() + 9.0
            }
            GhostLead::Plain => x0,
        };
        p.text(
            egui::pos2(text_x, y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.0),
            theme::text::FAINT,
        );
        ui::field_button(ui, icon::ARROW_COUNTER_CLOCKWISE, h, 14.0, FieldButton::Off)
            .on_hover_text("Restore — put the main's item back")
            .clicked()
    })
    .inner
}

/// The closed outline of `r` with corners of radius `radius`, as points for
/// [`egui::Shape::dashed_line`] — which has no rounded-rect form of its own.
fn rounded_outline(r: egui::Rect, radius: f32) -> Vec<egui::Pos2> {
    const STEPS: usize = 4;
    let corners = [
        (r.right_top() + egui::vec2(-radius, radius), -90.0_f32),
        (r.right_bottom() + egui::vec2(-radius, -radius), 0.0),
        (r.left_bottom() + egui::vec2(radius, -radius), 90.0),
        (r.left_top() + egui::vec2(radius, radius), 180.0),
    ];
    let mut pts = vec![egui::pos2(r.left() + radius, r.top())];
    for (c, start) in corners {
        for i in 0..=STEPS {
            let a = (start + 90.0 * i as f32 / STEPS as f32).to_radians();
            pts.push(c + radius * egui::vec2(a.cos(), a.sin()));
        }
    }
    pts.push(pts[0]);
    pts
}

/// `tx`, a reset, with the **placement stated** beside every write that places a
/// copy (a `SetTransform`, or a `SetGeometry` that resizes) and has no `SetInsets`
/// in `tx`: the insets of its **slot source** (`reset::slot_source_of`), or its
/// own for an instance root. The commit door reads a bare placement as the hand's
/// (`keep_insets` re-pins it, `kept_flow_translations` drops it); a `SetInsets`
/// stands both down. ⚠️ **The slot, not `source_of`, for a swapped copy.**
/// ⚠️ **One helper for every reset door** (`reset_tx`, `gather_card_overrides`);
/// a field's own reset (`transform_marks`) states its insets per axis.
fn placement_stated(doc: &ondin_core::Document, mut tx: Transaction) -> Transaction {
    let mut placed: Vec<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetTransform { id, .. } => Some(*id),
                Operation::SetGeometry { id, geometry } if geometry.resizes() => Some(*id),
                _ => None,
            })
            .filter(|id| {
                !tx.0
                    .iter()
                    .any(|op| matches!(op, Operation::SetInsets { id: i, .. } if i == id))
            })
            .collect();
    placed.sort();
    placed.dedup();
    for id in placed {
        let from = if ondin_core::reset::placement_is_own(doc, id) {
            Some(id)
        } else {
            ondin_core::reset::slot_source_of(doc, id)
        };
        if let Some(n) = from.and_then(|s| doc.get(s)) {
            tx.0.push(Operation::SetInsets {
                id,
                insets: *n.insets(),
            });
        }
    }
    tx
}

/// The inspector card that shows the field `op` writes, by title, or `None` for a
/// field no card header owns (the identity card's name and visibility, a path's
/// or boolean's shape, the text's content and rail, the mask flag, the fill rule).
/// Wildcard-free on the operation, so a new field operation is placed here by
/// whoever adds it.
fn card_of(op: &Operation) -> Option<&'static str> {
    use Operation as O;
    use ondin_core::GeometryPatch as G;
    match op {
        // A width's or a height's binding is the W or H field's.
        O::SetTransform { .. } | O::SetPivot { .. } | O::SetSizeVars { .. } => Some("Transform"),
        // A content's or a family's binding is the Type card's Content or family
        // row.
        O::SetTextVars { .. } => Some("Type"),
        O::SetGeometry { geometry, .. } => match geometry {
            G::Size(_) | G::LineEnd(_) => Some("Transform"),
            G::CornerRadius(_)
            | G::CornerRadii(_)
            | G::CornerSmoothing(_)
            | G::Sides(_)
            | G::InnerRatio(_) => {
                Some("Appearance")
            }
            G::TextSizing(_) => Some("Type"),
            G::Path { .. }
            | G::TextPath(_)
            | G::TextPathFlip(_)
            | G::TextPathOffset(_)
            | G::BoolOp(_) => None,
        },
        // A corner's binding is the radius field's, an opacity's the opacity field's.
        O::SetOpacity { .. }
        | O::SetOpacityVar { .. }
        | O::SetClip { .. }
        | O::SetCornerVars { .. } => Some("Appearance"),
        O::SetMaskMode { .. } => Some("Mask"),
        // A card's style is that card's: *Reset fill* restores the main's style
        // with the main's list.
        O::SetFills { .. } | O::SetFillStyle { .. } => Some("Fill"),
        O::SetStrokes { .. } | O::SetStrokeStyle { .. } => Some("Stroke"),
        O::SetEffects { .. } | O::SetEffectsStyle { .. } => Some("Effects"),
        O::SetExports { .. } => Some("Export"),
        O::SetLayoutGrids { .. } => Some("Layout grid"),
        O::SetInsets { .. } => Some("Position"),
        O::SetLayoutItem { .. } => Some("Item"),
        // A gap's or a padding's binding, and the style, are the Container card's.
        O::SetDisplay { .. } | O::SetSpacingVars { .. } | O::SetLayoutStyle { .. } => {
            Some("Container")
        }
        O::SetTextStyle { .. }
        | O::SetParagraphStyle { .. }
        | O::SetBlockStyle { .. }
        | O::SetTypeStyle { .. } => Some("Type"),
        O::SetName { .. }
        | O::SetVisible { .. }
        | O::SetVisibleVar { .. }
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
        | O::SetPageGround { .. }
        | O::SetPageDraft { .. }
        | O::AddPage { .. }
        | O::RemovePage { .. }
        | O::MovePage { .. }
        | O::AddGuide { .. }
        | O::RemoveGuide { .. }
        | O::SetGuidePosition { .. }
        | O::SetGuideColor { .. }
        | O::SetGuideScope { .. }
        | O::AddImage { .. }
        | O::RemoveImage { .. }
        | O::OpenVault { .. }
        | O::CloseVault { .. }
        | O::SetOrigin { .. }
        // A frame's mode is a row of the identity card, under its name.
        | O::SetMode { .. }
        | O::AddVariable { .. }
        | O::RemoveVariable { .. }
        | O::SetVariableName { .. }
        | O::SetVariableCell { .. }
        | O::SetVariable { .. }
        | O::SetKitModes { .. }
        | O::AddMode { .. }
        | O::RemoveMode { .. }
        | O::SetModeName { .. }
        | O::AddStyle { .. }
        | O::RemoveStyle { .. }
        | O::SetStyleName { .. }
        | O::SetStyleOrigin { .. }
        | O::SetStyleValue { .. }
        // A main's description is the main's, never an instance's override.
        | O::SetDescription { .. } => None,
    }
}

/// An overridden dropdown's reset, as the open list's first row — ↺ *Reset to
/// main · Alpha*, then a rule. A dropdown's face is one press target that opens
/// the list, with no label slot to give the ↺, so the reset is the first thing
/// the press shows. Answers whether it was clicked.
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

/// A row's label carrying an override mark: at full brightness with the dot after
/// it, and with the pointer on the label ↺ in the dot's place, the mark's
/// tooltip, and a click that is the reset. Unmarked, the label in `ink`. `size`
/// is the label's slot, its text centred in it; `None` is the text's own width.
/// Answers whether the reset was clicked.
pub(super) fn label_mark(
    ui: &mut egui::Ui,
    size: Option<egui::Vec2>,
    text: &str,
    pt: f32,
    ink: egui::Color32,
    mark: Option<&OverrideMark>,
) -> bool {
    label_mark_in(ui, size, egui::Align::Center, text, pt, ink, mark)
}

/// The room a right-aligned [`label_mark_in`] keeps after its text for the dot,
/// and for the ↺ that takes its place on hover — 12pt, so neither reaches the
/// control beside it. The centred [`label_mark`] does not read it.
pub(super) const MARK_ROOM: f32 = 12.0;

/// [`label_mark`] with the text placed by `align` in its slot: centred, or —
/// [`egui::Align::Max`] — **right-aligned against the control it labels**, with
/// [`MARK_ROOM`] kept for the mark and a label too long for the slot cut with an
/// ellipsis, its whole name in the tooltip. The Component card's label column
/// uses it.
pub(super) fn label_mark_in(
    ui: &mut egui::Ui,
    size: Option<egui::Vec2>,
    align: egui::Align,
    text: &str,
    pt: f32,
    ink: egui::Color32,
    mark: Option<&OverrideMark>,
) -> bool {
    let color = match mark {
        Some(_) => theme::text::STRONG,
        None => ink,
    };
    let right = align == egui::Align::Max && size.is_some();
    let (galley, cut) = match size.filter(|_| right) {
        Some(slot) => fitted_label(ui, text, pt, color, (slot.x - MARK_ROOM).max(0.0)),
        None => (
            ui.painter()
                .layout_no_wrap(text.to_owned(), egui::FontId::proportional(pt), color),
            false,
        ),
    };
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
        Some(_) if right => rect.right() - MARK_ROOM - w,
        Some(_) => rect.center().x - w / 2.0,
        None => rect.left(),
    };
    let at = egui::pos2(left, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, color);
    let Some(m) = mark else {
        if cut {
            resp.on_hover_text(text);
        }
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
    // A cut label names itself in the tooltip too, over the mark's reset.
    match cut {
        true => resp.on_hover_text(format!("{text}\n{}", m.tip)),
        false => resp.on_hover_text(&m.tip),
    }
    .clicked()
}

/// `text` laid out on one line no wider than `max_w`, cut with an ellipsis, and
/// whether it was cut — what decides that a label names itself in a tooltip.
/// 🚨 **Asked of `Galley::elided`, never of the text**: `Galley::text()` is the
/// *job's* whole text, so comparing it with `text` is false for every label.
fn fitted_label(
    ui: &egui::Ui,
    text: &str,
    pt: f32,
    color: egui::Color32,
    max_w: f32,
) -> (std::sync::Arc<egui::Galley>, bool) {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(egui::FontId::proportional(pt), color),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: max_w,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let cut = galley.elided;
    (galley, cut)
}

/// An overridden dropdown's dot, where it has no label to sit after: beside its
/// chevron, at the height a field's dot sits beside its label.
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
/// disappears. A property's fields count once, as the property: properties are
/// the knobs meant to be turned, so they do not read as drift.
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

/// The face of an instance card — or a variant's, with `main` — the hexagon
/// (outline for an instance, filled for a `main`), an optional small caption over
/// the main's name as a link (or over a plain `title`), and the grey drift
/// summary on the right. One line when `caption` is empty; several instances
/// keep their count as the caption, and a variant its set.
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
        // **The name is cut to the room the row leaves it**: a plain label in a
        // horizontal row never wraps, and egui grows the parent to its widest row,
        // so a long name pushes every card below off the screen. The whole name in
        // the tooltip. (A variant's derived name is `name_and_count`'s.)
        let summary_w = if summary.is_empty() {
            0.0
        } else {
            let font = egui::FontId::proportional(11.0);
            ui.painter()
                .layout_no_wrap(summary.to_owned(), font, theme::text::DIM)
                .size()
                .x
                + 9.0
        };
        let arrow_w = ui
            .painter()
            .layout_no_wrap(
                icon::ARROW_UP_RIGHT.to_owned(),
                theme::icon_font(12.0),
                theme::text::DIM,
            )
            .size()
            .x
            + 4.0;
        let room = (ui.available_width() - summary_w).max(24.0);
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
                // The name, then the ↗ — one target. **As tall as the name, not as
                // a control**: a horizontal row opens at `interact_size.y` and
                // would centre the name 4pt below a main's.
                ui.spacing_mut().interact_size.y = 0.0;
                let name_room = (room - arrow_w).max(12.0);
                let cut = ui
                    .painter()
                    .layout_no_wrap(
                        name.to_owned(),
                        egui::FontId::proportional(13.0),
                        theme::text::STRONG,
                    )
                    .size()
                    .x
                    > name_room;
                let resp = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let name = ui
                            .scope(|ui| {
                                ui.set_max_width(name_room);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(name)
                                            .size(13.0)
                                            .underline()
                                            .color(theme::text::STRONG),
                                    )
                                    .truncate()
                                    // Named in the row's own tooltip below, one
                                    // tooltip rather than two stacked.
                                    .show_tooltip_when_elided(false)
                                    .sense(egui::Sense::click()),
                                )
                            })
                            .inner;
                        let arrow = ui.add(
                            egui::Label::new(
                                egui::RichText::new(icon::ARROW_UP_RIGHT)
                                    .font(theme::icon_font(12.0))
                                    .color(theme::text::DIM),
                            )
                            .sense(egui::Sense::click()),
                        );
                        name | arrow
                    })
                    .inner
                    .on_hover_text(match cut {
                        true => format!("{name}\nGo to main component"),
                        false => "Go to main component".to_owned(),
                    });
                if resp.clicked() {
                    *act = Some(Act::GoToMain(main));
                }
            }
            if let Some(title) = title {
                ui.scope(|ui| {
                    ui.set_max_width(room);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title)
                                .size(13.0)
                                .color(theme::text::STRONG),
                        )
                        .truncate(),
                    );
                });
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
/// the overflow of counted resets.
fn reset_row(
    ui: &mut egui::Ui,
    d: Drift,
    p: PropDrift,
    overflow: bool,
    kit: Option<&crate::kits::card::KitLine>,
    act: &mut Option<Act>,
) {
    // A kit instance always has the ⋯, for its kit rows.
    let overflow = overflow || kit.is_some();
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
                .on_hover_text("More");
            egui::Popup::menu(&more)
                .align(egui::RectAlign::BOTTOM_END)
                .gap(ui::MENU_GAP)
                .show(|ui| {
                    ui::menu_rows(ui);
                    // **As wide as its longest row, not as the screen**: `menu_row`
                    // takes the available width, which in a popup's `Area` is the
                    // whole screen.
                    let ctx = ui.ctx().clone();
                    let label_w = |t: &str| {
                        ctx.fonts_mut(|f| {
                            f.layout_no_wrap(
                                t.to_string(),
                                egui::FontId::proportional(11.5),
                                egui::Color32::WHITE,
                            )
                            .size()
                            .x
                        })
                    };
                    // The labels once, measured here and drawn below, so a row
                    // renamed longer cannot outgrow the width set for it.
                    const PROPS: &str = "Reset properties";
                    let rows = [
                        (
                            Kind::Fields,
                            "Reset fields",
                            d.fields.saturating_sub(p.units),
                        ),
                        (Kind::Children, "Restore removed children", d.removed),
                        (Kind::Order, "Reset order", d.order),
                    ];
                    // The kit rows count too, with their glyph's room.
                    let kit_labels: Vec<String> = kit
                        .map(|k| {
                            vec![
                                format!("Go to main in {}", k.kit_name),
                                "Accept kit updates".into(),
                                "Make local…".into(),
                            ]
                        })
                        .unwrap_or_default();
                    const GLYPH_ROOM: f32 = 22.0;
                    let widest = std::iter::once(PROPS)
                        .chain(rows.iter().map(|(_, l, _)| *l))
                        .map(label_w)
                        .chain(kit_labels.iter().map(|l| label_w(l) + GLYPH_ROOM))
                        .fold(0.0, f32::max);
                    ui.set_width((8.0 + widest + 24.0 + 16.0 + 8.0).ceil());
                    // A count only where there is something to reset; the dimmed
                    // row already says there is nothing.
                    let count_of = |n: usize| (n > 0).then(|| n.to_string());
                    // Properties and fields count, and reset, separately. Present
                    // wherever the component has properties, and disabled at zero
                    // like its neighbours.
                    if p.defined {
                        let count = count_of(p.props);
                        let row = ui::MenuRow::new("", PROPS)
                            .accel(count.as_deref())
                            .enabled(p.props > 0);
                        if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && p.props > 0 {
                            *act = Some(Act::ResetProperties);
                        }
                    }
                    for (kind, label, n) in rows {
                        let count = count_of(n);
                        let row = ui::MenuRow::new("", label)
                            .accel(count.as_deref())
                            .enabled(n > 0);
                        if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && n > 0 {
                            *act = Some(Act::Reset(kind));
                        }
                    }
                    // The kit rows, after the resets: where the main lives, the
                    // update when one waits, and the way out.
                    if let Some(k) = kit {
                        ui::menu_sep(ui);
                        let go = format!("Go to main in {}", k.kit_name);
                        if ui::menu_item(ui, icon::ARROW_SQUARE_OUT, &go, ui::MENU_ROW_H).clicked()
                        {
                            *act = Some(Act::GoToKitMain);
                        }
                        if k.waiting.is_some()
                            && ui::menu_item(
                                ui,
                                icon::ARROW_CIRCLE_DOWN,
                                "Accept kit updates",
                                ui::MENU_ROW_H,
                            )
                            .clicked()
                        {
                            *act = Some(Act::AcceptKitUpdate(k.kit.clone()));
                        }
                        ui::menu_sep(ui);
                        if ui::menu_item(ui, icon::PACKAGE, "Make local…", ui::MENU_ROW_H).clicked()
                        {
                            *act = Some(Act::MakeLocal(k.top));
                        }
                    }
                });
        }
    });
}

/// A card's one-line face: the glyph in its slot (`None` for the filled hexagon),
/// the name beside it, and — with `instances` — the count after the name
/// ([`count_link`]). A main's face and a set's on the Variants card share this
/// layout. The name is cut first, whole in a tooltip, so the count always shows.
/// Answers whether the count was clicked.
pub(super) fn name_line(
    ui: &mut egui::Ui,
    glyph: Option<&str>,
    name: &str,
    instances: Option<usize>,
) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        let slot = glyph_slot(ui);
        // In a block, as the name alone was: a row's child placed straight into
        // this horizontal is centred in its interact height, 12pt lower.
        let block = ui.vertical(|ui| name_and_count(ui, name, instances)).inner;
        match glyph {
            None => paint_hexagon(ui, true, slot, block.rect),
            Some(g) => paint_glyph(ui, g, slot, block.rect),
        }
        block.clicked
    })
    .inner
}

/// What [`name_and_count`] drew: the name's rect, which a leading glyph centres
/// on, and whether the count was clicked.
struct NameCount {
    rect: egui::Rect,
    clicked: bool,
}

/// A face's name at 13pt and, with `instances`, its count after it at the same
/// size ([`count_link`]) — the name cut to leave the count its room. A variant's
/// derived name on its face draws this bare.
fn name_and_count(ui: &mut egui::Ui, name: &str, instances: Option<usize>) -> NameCount {
    // **No interact height**, as `heading`'s name: a horizontal row opens
    // `interact_size.y` tall and centres in it. Scoped, or every row under a
    // variant's name would lose it too.
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 0.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COUNT_GAP;
            let count_w = instances.map_or(0.0, |n| {
                ui.painter()
                    .layout_no_wrap(
                        plural(n, "instance"),
                        egui::FontId::proportional(NAME_PT),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
                    + COUNT_GAP
            });
            let room = (ui.available_width() - count_w).max(24.0);
            let rect = ui
                .scope(|ui| {
                    ui.set_max_width(room);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(name)
                                .size(NAME_PT)
                                .color(theme::text::STRONG),
                        )
                        .truncate(),
                    )
                    .rect
                })
                .inner;
            let clicked = instances.is_some_and(|n| count_link(ui, n));
            NameCount { rect, clicked }
        })
        .inner
    })
    .inner
}

/// A face's name size, and the count's after it.
const NAME_PT: f32 = 13.0;
/// A description's text size, and the air above it on a card.
const DESC_PT: f32 = 11.5;
const DESC_GAP: f32 = 2.0;

/// An instance's main's description, read-only and dim under its face: the
/// variant's own, else its set's. Nothing when neither has one.
fn description_note(doc: &ondin_core::Document, main: NodeId, ui: &mut egui::Ui) {
    let own = doc.get(main).map(|n| n.description()).unwrap_or_default();
    let text = if own.is_empty() {
        ondin_core::variant::set_of(doc, main)
            .and_then(|s| doc.get(s))
            .map(|s| s.description())
            .unwrap_or_default()
    } else {
        own
    };
    if text.is_empty() {
        return;
    }
    ui.add_space(DESC_GAP);
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(DESC_PT)
                .color(theme::text::DIM),
        )
        .wrap(),
    );
}
/// The air between a face's name and its count.
const COUNT_GAP: f32 = 6.0;

/// *4 instances* after a face's name: the name's size, dimmer, **underlined as a
/// link**, brightening under the pointer, and a click selects them all. At zero
/// there is nothing to select: plain, dimmer still, no underline and no tooltip.
/// Answers whether it was clicked.
fn count_link(ui: &mut egui::Ui, instances: usize) -> bool {
    let text = plural(instances, "instance");
    if instances == 0 {
        ui.label(
            egui::RichText::new(text)
                .size(NAME_PT)
                .color(theme::text::DIM),
        );
        return false;
    }
    let font = egui::FontId::proportional(NAME_PT);
    let size = ui
        .painter()
        .layout_no_wrap(text.clone(), font.clone(), egui::Color32::WHITE)
        .size();
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let color = if resp.hovered() {
        theme::text::STRONG
    } else {
        theme::text::MUTED
    };
    let job = egui::text::LayoutJob::single_section(
        text,
        egui::TextFormat {
            font_id: font,
            color,
            underline: egui::Stroke::new(1.0, color),
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(rect.min, galley, color);
    resp.on_hover_text("Click to select all").clicked()
}

/// *Duplicate*, which a variant's face shares. Shortened from *Duplicate as
/// component* on the card that already says what it is; the context menu keeps
/// the long name, where nothing around it does.
fn main_tail(ui: &mut egui::Ui, act: &mut Option<Act>) {
    let h = ui::CONTROL_H;
    let w = ui.available_width();
    if ui::action_button(
        ui,
        icon::COPY,
        "Duplicate",
        FieldButton::Off,
        egui::vec2(w, h),
    )
    .on_hover_text("A copy that is a new main, not an instance of this one")
    .clicked()
    {
        *act = Some(Act::DuplicateAsComponent);
    }
}

/// *In Button instance · Go to main*, or *Local to this instance · no
/// counterpart* — a row of the identity card. A local layer leads with the
/// **broken link**, not `+`, which at the head of a row reads as a button.
fn child_line(ui: &mut egui::Ui, main_name: &str, linked: bool, act: &mut Option<Act>) {
    let (glyph, text) = if linked {
        (icon::HEXAGON, format!("In {main_name} instance"))
    } else {
        (icon::LINK_BREAK, "Local to this instance".to_string())
    };
    let mut slot = egui::Rect::NOTHING;
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        slot = glyph_slot(ui);
        // **Cut to what *Go to main* leaves**, or a long main name pushes the
        // button and every card below off the screen.
        let button = if linked {
            ui::action_button_w(ui.ctx(), "Go to main") + ui.spacing().item_spacing.x
        } else {
            0.0
        };
        let room = (ui.available_width() - button).max(24.0);
        ui.scope(|ui| {
            ui.set_max_width(room);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(12.0)
                        .color(theme::text::STRONG),
                )
                .truncate(),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if linked {
                let label = "Go to main";
                let w = ui::action_button_w(ui.ctx(), label);
                if ui::action_button(
                    ui,
                    icon::ARROW_UP_RIGHT,
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
    // The hexagon a point above the row's centre, or it sits low against the words.
    let lift = if linked { 1.0 } else { 0.0 };
    paint_glyph(
        ui,
        glyph,
        slot,
        row.response.rect.translate(egui::vec2(0.0, -lift)),
    );
}

/// *2 instances · 3 other layers*, and *Select instances* to narrow to them.
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
/// ⚠️ **Painted afterwards ([`paint_glyph`]) rather than laid out as a label**: a
/// horizontal row centres each item against the height the row has *so far*, so a
/// glyph placed first would sit top-aligned beside a two-line block.
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

/// The hexagon in a face's glyph slot: **filled** for a main or a variant
/// (`ui::paint_hexagon_filled`), outline for an instance.
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
            corner_smoothing: 0.0,
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
            // **Off its main**: over the main, world and local readings agree, and
            // a Transform mark compared on world numbers would pass every test.
            ondin_core::kurbo::Vec2::new(200.0, 40.0),
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

    /// Each selection gets its face: a main, an instance, a child, two instances
    /// of one main, instances among other layers, and nothing for a plain layer.
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
                    parent: doc.first_page(),
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
    /// count with it. Flip: never clearing the cache leaves the stale zero.
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

    /// The restore button of the ghost row labelled `label` — the ↺ on the
    /// label's line, to its right.
    fn ghost_restore(painted: &[(String, egui::Rect)], label: &str) -> Option<egui::Pos2> {
        let row = painted.iter().find(|(t, _)| t == label)?.1;
        painted
            .iter()
            .filter(|(t, r)| {
                t == icon::ARROW_COUNTER_CLOCKWISE
                    && (r.center().y - row.center().y).abs() < 4.0
                    && r.left() > row.right()
            })
            .map(|(_, r)| r.center())
            .next()
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

    /// **The card, drawn and clicked.** An instance with a renamed child shows its
    /// main's name on one line and *1 override*; a click on *Reset all* puts the
    /// name back and the summary goes.
    #[test]
    fn reset_all_on_the_card_puts_the_instance_back() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(f.app.session.commit(Transaction(vec![Operation::SetName {
            id: f.ir,
            name: "Mine".into(),
        }])));
        f.app.session.selection.set_one(f.i);
        let out = settle(&mut f.app, &ctx);
        let painted = texts(&out);
        let has = |s: &str| painted.iter().any(|(t, _)| t == s);
        assert!(!has("Instance of"), "the caption is gone: {painted:?}");
        let hex = painted
            .iter()
            .find(|(t, _)| t == icon::HEXAGON)
            .expect("the outline hexagon")
            .1;
        // The name the hexagon leads: the nearest *Button* to its right — the
        // identity card above paints the layer's name too.
        let name = painted
            .iter()
            .filter(|(t, r)| t == "Button" && r.left() > hex.right())
            .map(|(_, r)| *r)
            .min_by(|a, b| {
                let d = |r: &egui::Rect| (r.center().y - hex.center().y).abs();
                d(a).total_cmp(&d(b))
            })
            .expect("the main's name");
        // Against the name's **cap band**, not its box: the link's underline
        // drags the box's centre down. Inter's cap height is 0.727em, the name 13pt.
        let cap_mid = name.top() + 13.0 * 0.727 / 2.0;
        assert!(
            (hex.center().y - cap_mid).abs() <= 1.0,
            "hexagon {hex:?} beside name {name:?}"
        );
        assert!(has("1 override"), "{painted:?}");
        let at = painted
            .iter()
            .find(|(t, _)| t == "Reset all")
            .map(|(_, r)| r.center())
            .expect("the button's word is painted");
        click(&mut f.app, &ctx, at);
        assert_eq!(f.app.session.doc.get(f.ir).unwrap().name(), "Label");
        let out = frame(&mut f.app, &ctx, Vec::new());
        assert!(
            !texts(&out).iter().any(|(t, _)| t.contains("override")),
            "the summary is gone at zero"
        );
    }

    /// **A main's card offers its description; an instance's card reads it.**
    /// The main's face draws the field's hint until there is one, and its
    /// instance's face draws the text under the main's name.
    ///
    /// flip: `description_note` returning at once → fails at "the instance reads it".
    #[test]
    fn a_mains_description_is_typed_on_its_card_and_read_on_its_instances() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.m);
        let out = settle(&mut f.app, &ctx);
        let has = |out: &egui::FullOutput, s: &str| texts(out).iter().any(|(t, _)| t == s);
        assert!(has(&out, "Add a description"), "{:?}", texts(&out));
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetDescription {
                    of: ondin_core::Described::Node(f.m),
                    text: "The primary action".into(),
                }]))
        );
        let out = settle(&mut f.app, &ctx);
        assert!(has(&out, "The primary action"), "the field holds it");
        f.app.session.selection.set_one(f.i);
        let out = settle(&mut f.app, &ctx);
        assert!(has(&out, "The primary action"), "the instance reads it");
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

    /// **A field's mark.** Moved along x only, the copy's X label is bright and Y
    /// dim; a click on X's label writes the main's x back. Flip: comparing the
    /// whole transform in `transform_marks` fails *"Y follows"*. ⚠️ **A world
    /// reading bites only because `fixture` places the instance off its main.**
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
        let out = settle(&mut f.app, &ctx);
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
        click(&mut f.app, &ctx, x_at.center());
        let doc = &f.app.session.doc;
        assert_eq!(
            doc.get(f.ir).unwrap().transform(),
            doc.get(main_rect).unwrap().transform(),
            "the main's x is back"
        );
    }

    /// **A marked field's ↺ reads as a button, not as a scrub handle**: the cursor
    /// over X's label is the plain arrow (an unmarked label shows the scrub's
    /// arrows) and a tooltip opens naming the reset. Flips, run: the strip's cursor
    /// left at the arrows fails *"the marked label's cursor"*; dropping its
    /// `on_hover_text` fails *"a tooltip is open"*.
    #[test]
    fn a_marked_fields_reset_shows_an_arrow_and_a_tooltip() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: f.ir,
                    transform: ondin_core::kurbo::Affine::translate((7.0, 0.0)),
                }]))
        );
        f.app.session.selection.set_one(f.ir);
        let out = settle(&mut f.app, &ctx);
        let painted = inks(&out);
        let at_of = |s: &str| {
            painted
                .iter()
                .find(|(t, ..)| t == s)
                .map(|(_, r, _)| r.center())
                .unwrap_or_else(|| panic!("no {s} in {painted:?}"))
        };
        let (x, y) = (at_of("X"), at_of("Y"));
        let rest = |app: &mut OndinApp, at: egui::Pos2| {
            let mut out = frame(app, &ctx, vec![egui::Event::PointerMoved(at)]);
            // Past egui's 0.5s tooltip delay: a pass with no time advances 1/60s.
            for _ in 0..45 {
                out = frame(app, &ctx, Vec::new());
            }
            out
        };
        let out = rest(&mut f.app, x);
        assert_eq!(
            out.platform_output.cursor_icon,
            egui::CursorIcon::Default,
            "the marked label's cursor"
        );
        let tooltip = ctx.memory(|m| {
            m.areas()
                .visible_layer_ids()
                .iter()
                .any(|l| l.order == egui::Order::Tooltip)
        });
        assert!(tooltip, "a tooltip is open over the marked label");
        let out = rest(&mut f.app, y);
        assert_eq!(
            out.platform_output.cursor_icon,
            egui::CursorIcon::ResizeHorizontal,
            "an unmarked label still scrubs"
        );
    }

    /// **The Appearance card's marks**: opacity's glyph is bright, the radius field
    /// marked, and only the overridden top-left corner marked, its reset writing
    /// that corner alone.
    ///
    /// ⚠️ **Two guards in `sub_mark` each keep an untouched corner unmarked**: the
    /// equality skip and *"a reset that would write nothing marks nothing"*. Flips,
    /// run: the equality skip dropped leaves this test green and fails where `put`
    /// is lossy (`layout::tests::an_instance_marks_its_own_layout_fields_and_no_others`);
    /// the write-nothing guard dropped alone fails only that layout test; both
    /// dropped fail here at *"the top right follows"*.
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
        let out = settle(&mut f.app, &ctx);
        let painted = inks(&out);
        let (drop_at, drop_ink) = painted
            .iter()
            .find(|(t, ..)| t == icon::DROP_HALF)
            .map(|(_, r, c)| (*r, *c))
            .expect("the opacity field");
        assert_eq!(drop_ink, theme::text::STRONG, "opacity is overridden");
        click(&mut f.app, &ctx, drop_at.center());
        assert_eq!(f.app.session.doc.get(f.ir).unwrap().opacity(), 1.0);
    }

    /// **A copy's corner binding is a radius override**: the main's rect bound to
    /// *radius/md*, the copy's rebound to *radius/lg* marks the radius field, its
    /// tip naming the main's variable and its ↺ putting that binding back; one
    /// corner rebound marks that corner alone. Flip, run: `Corner::var_slot` as
    /// `self as usize` fails *"the bottom right is the corner rebound"*.
    #[test]
    fn a_copys_corner_binding_marks_the_radius_and_its_reset_rebinds() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let doc = &f.app.session.doc;
        let r = doc.get(f.m).unwrap().children()[0];
        let mode = f.app.session.ids.mint_mode();
        let (md, lg) = (f.app.session.ids.mint_var(), f.app.session.ids.mint_var());
        let number = |id, name: &str, n| ondin_core::variable::Variable {
            id,
            name: name.into(),
            kind: ondin_core::variable::VarKind::Number,
            cells: [(mode, ondin_core::variable::Cell::Number(n))].into(),
            origin: None,
            forked: None,
            description: String::new(),
        };
        let mut ops = vec![
            Operation::AddMode {
                mode: ondin_core::variable::Mode {
                    id: mode,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: number(md, "radius/md", 6.0),
            },
            Operation::AddVariable {
                var: number(lg, "radius/lg", 12.0),
            },
        ];
        ops.extend(ondin_core::bind_corners_ops(doc, r, [Some(md); 4]));
        assert!(f.app.session.commit(Transaction(ops)));
        assert_eq!(
            f.app.session.doc.get(f.ir).unwrap().corner_vars(),
            [Some(md); 4],
            "the copy follows its main's binding"
        );
        let rebind = ondin_core::bind_corners_ops(&f.app.session.doc, f.ir, [Some(lg); 4]);
        assert!(f.app.session.commit(Transaction(rebind)));
        f.app.session.selection.set_one(f.ir);
        f.app.gather_card_overrides();
        let marks = f.app.appearance_marks(f.ir);
        let radius = marks.radius.expect("the binding is an override");
        assert_eq!(radius.tip, "Reset to main · radius/md");
        assert!(f.app.session.commit(radius.tx));
        assert_eq!(
            f.app.session.doc.get(f.ir).unwrap().corner_vars(),
            [Some(md); 4],
            "the reset puts the main's binding back"
        );

        let one = ondin_core::bind_corners_ops(
            &f.app.session.doc,
            f.ir,
            [Some(md), Some(md), Some(lg), Some(md)],
        );
        assert!(f.app.session.commit(Transaction(one)));
        f.app.gather_card_overrides();
        let marks = f.app.appearance_marks(f.ir);
        // `marks.corners` is in reading order, TL TR BL BR; `CornerVars` is TL TR BR BL.
        let (bl, br) = (2, 3);
        assert!(
            marks.corners[br].is_some(),
            "the bottom right is the corner rebound"
        );
        assert!(marks.corners[bl].is_none(), "the bottom left follows");
    }

    /// **A copy's width bound as its main's draws the chip; resized, it detaches
    /// and W is marked; the ↺ binds it back.** The main's rect is bound to *size/w*
    /// (30); a width written on the copy detaches it (`keep_size_bindings`).
    /// Flip, run: `transform_marks`' reset without its `SetSizeVars` fails *"the
    /// reset puts the main's binding back"*.
    #[test]
    fn a_copys_detached_width_marks_w_and_its_reset_rebinds() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let doc = &f.app.session.doc;
        let r = doc.get(f.m).unwrap().children()[0];
        let (mode, w) = (f.app.session.ids.mint_mode(), f.app.session.ids.mint_var());
        let mut ops = vec![
            Operation::AddMode {
                mode: ondin_core::variable::Mode {
                    id: mode,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: ondin_core::variable::Variable {
                    id: w,
                    name: "size/w".into(),
                    kind: ondin_core::variable::VarKind::Number,
                    cells: [(mode, ondin_core::variable::Cell::Number(30.0))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ];
        ops.extend(ondin_core::bind_size_ops(doc, r, [Some(w), None]));
        assert!(f.app.session.commit(Transaction(ops)));
        let drawn = |f: &F| {
            let doc = &f.app.session.doc;
            f.app
                .session
                .resolved
                .used_kind(doc, f.ir)
                .and_then(|k| k.box_size())
                .unwrap()
        };
        assert_eq!(
            f.app.session.doc.get(f.ir).unwrap().size_vars(),
            [Some(w), None],
            "the copy follows its main's binding"
        );
        assert_eq!(drawn(&f).width, 30.0);
        let out = settled(&mut f.app, &ctx, f.ir);
        assert!(
            texts(&out).iter().any(|(t, _)| t == "size/w"),
            "W is a chip naming the variable"
        );

        let h = drawn(&f).height;
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetGeometry {
                    id: f.ir,
                    geometry: ondin_core::GeometryPatch::Size(Size::new(18.0, h)),
                }]))
        );
        assert_eq!(
            f.app.session.doc.get(f.ir).unwrap().size_vars(),
            [None, None],
            "a width written past the binding detaches it"
        );
        let out = settled(&mut f.app, &ctx, f.ir);
        let at = inks(&out)
            .into_iter()
            .find(|(t, _, c)| t == "W" && *c == theme::text::STRONG)
            .map(|(_, r, _)| r.center())
            .expect("a marked W");
        click(&mut f.app, &ctx, at);
        assert_eq!(
            f.app.session.doc.get(f.ir).unwrap().size_vars(),
            [Some(w), None],
            "the reset puts the main's binding back"
        );
        assert_eq!(drawn(&f).width, 30.0, "and the copy draws its number again");
    }

    /// **A copy's own mode is an override, marked on its *Mode* row**: the label is
    /// bright with a dot, and a click puts the copy back to inheriting. Flip, run:
    /// `frame_mode_row` without its `reset_marked` fails *"the reset puts the copy
    /// back to inheriting"*.
    #[test]
    fn a_copys_own_mode_is_marked_on_its_mode_row() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let (light, dark) = (f.app.session.ids.mint_mode(), f.app.session.ids.mint_mode());
        let add = |id, name: &str, index| Operation::AddMode {
            mode: ondin_core::variable::Mode {
                id,
                name: name.into(),
            },
            index,
            cells: vec![],
        };
        assert!(f.app.session.commit(Transaction(vec![
            add(light, "Light", 0),
            add(dark, "Dark", 1),
            Operation::SetMode {
                id: f.i,
                mode: Some(dark),
            },
        ])));
        f.app.session.selection.set_one(f.i);
        f.app.gather_card_overrides();
        let out = settle(&mut f.app, &ctx);
        let (label, ink) = inks(&out)
            .into_iter()
            .find(|(t, ..)| t == "Mode")
            .map(|(_, r, c)| (r, c))
            .expect("the Mode row is drawn");
        assert_eq!(ink, theme::text::STRONG, "the label is bright");
        assert!(
            dots(&out)
                .iter()
                .any(|d| (d.y - label.center().y).abs() < 4.0),
            "with a dot on its line"
        );
        click(&mut f.app, &ctx, label.center());
        assert_eq!(
            f.app.session.doc.get(f.i).unwrap().mode(),
            None,
            "the reset puts the copy back to inheriting"
        );
    }

    /// The centre of every override dot the frame painted.
    fn dots(out: &egui::FullOutput) -> Vec<egui::Pos2> {
        ui::painted_override_dots(out)
    }

    /// The inspector drawn, then three more frames for it to settle — a widget's
    /// state is last frame's.
    fn settle(app: &mut OndinApp, ctx: &egui::Context) -> egui::FullOutput {
        let mut out = frame(app, ctx, Vec::new());
        for _ in 0..3 {
            out = frame(app, ctx, Vec::new());
        }
        out
    }

    /// Press and release the primary button at `at`, the pointer moved there first.
    fn click(app: &mut OndinApp, ctx: &egui::Context, at: egui::Pos2) {
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

    /// **The Position card's centre buttons carry a mark**: only the horizontal one
    /// draws the dot, and its click resets the main's whole horizontal axis
    /// without writing the vertical. Flips: `centred` reading the wrong axis'
    /// margins fails *"one dot"*; `centre_mark`'s `put` copying the main's whole
    /// insets fails *"the vertical axis untouched"*.
    #[test]
    fn a_centre_button_marks_a_centring_the_main_does_not_share() {
        use ondin_core::container::AutoMargins;
        use ondin_core::{Insets, LengthPct};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let main_rect = f.app.session.doc.get(f.m).unwrap().children()[0];
        let centred = Insets {
            left: Some(LengthPct::Px(30.0)),
            right: Some(LengthPct::Px(60.0)),
            margin_auto: AutoMargins {
                left: true,
                right: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(f.app.session.commit(Transaction(vec![Operation::SetInsets {
            id: main_rect,
            insets: centred,
        }])));
        assert_eq!(
            *f.app.session.doc.get(f.ir).unwrap().insets(),
            centred,
            "the fixture's copy follows its main"
        );
        // Pinned top 20 of its own, against the main's unset top: the vertical
        // axis has to *differ* for "untouched" to say anything.
        let pinned = Insets {
            left: Some(LengthPct::Px(5.0)),
            top: Some(LengthPct::Px(20.0)),
            ..Default::default()
        };
        assert!(f.app.session.commit(Transaction(vec![Operation::SetInsets {
            id: f.ir,
            insets: pinned,
        }])));
        f.app.session.selection.set_one(f.ir);
        let out = settle(&mut f.app, &ctx);
        let painted = inks(&out);
        // The last of each: the align row above draws the same two glyphs.
        let glyph = |g: &str| {
            painted
                .iter()
                .rfind(|(t, ..)| t == g)
                .map(|(_, r, _)| *r)
                .unwrap_or_else(|| panic!("no {g} glyph"))
        };
        let h = glyph(icon::ALIGN_CENTER_VERTICAL);
        let v = glyph(icon::ALIGN_CENTER_HORIZONTAL);
        let row: Vec<_> = dots(&out)
            .into_iter()
            .filter(|d| (d.y - h.center().y).abs() < 14.0)
            .collect();
        assert_eq!(row.len(), 1, "one dot on the centre buttons' row: {row:?}");
        assert!(
            row[0].x > h.center().x && row[0].x < v.center().x,
            "the horizontal button's: {row:?} between {h:?} and {v:?}"
        );
        click(&mut f.app, &ctx, h.center());
        let now = *f.app.session.doc.get(f.ir).unwrap().insets();
        assert_eq!(
            (now.left, now.right, now.margin_auto),
            (centred.left, centred.right, centred.margin_auto),
            "the main's horizontal axis is back"
        );
        assert_eq!(
            (now.top, now.bottom),
            (Some(LengthPct::Px(20.0)), None),
            "the vertical axis untouched"
        );
    }

    /// **The shared radius over several layers carries a mark.** The instance and a
    /// plain rect are selected; the copy's radius is 6 where its main's is 0. The
    /// glyph is bright and a click writes the main's radius into the copy alone.
    /// Flip: subjects taken as the selection, not its subtree, reach only the
    /// instance's frame and fail *"the radius is overridden"*.
    #[test]
    fn the_shared_radius_marks_an_override_in_the_selections_subtree() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let six = ondin_core::kurbo::RoundedRectRadii::from_single_radius(6.0);
        let radius = |id| Operation::SetGeometry {
            id,
            geometry: ondin_core::GeometryPatch::CornerRadii(six),
        };
        assert!(
            f.app
                .session
                .commit(Transaction(vec![radius(f.ir), radius(f.plain)]))
        );
        f.app.session.selection.set(vec![f.i, f.plain]);
        let out = settle(&mut f.app, &ctx);
        let (at, ink) = inks(&out)
            .into_iter()
            .find(|(t, ..)| t == icon::CORNERS_OUT)
            .map(|(_, r, c)| (r, c))
            .expect("the multi-selection radius field");
        assert_eq!(ink, theme::text::STRONG, "the radius is overridden");
        click(&mut f.app, &ctx, at.center());
        let radii = |id| match f.app.session.doc.get(id).unwrap().kind() {
            NodeKind::Rect { corner_radii, .. } => *corner_radii,
            _ => unreachable!("a rect"),
        };
        assert_eq!(radii(f.ir), Default::default(), "the main's radius is back");
        assert_eq!(
            radii(f.plain),
            six,
            "the plain rect has no main to go back to"
        );
    }

    /// **An instance that removed every item still shows its ghost rows**, and
    /// each *Restore* puts the main's item back by its id. Flips, run:
    /// `has_ghosts` answering `false` fails *"two ghost rows"* with none; the Fill
    /// card's empty-list check put back to `fills.is_empty()` alone fails it with
    /// one.
    #[test]
    fn an_emptied_list_still_draws_its_ghost_rows() {
        use ondin_core::{ExportSpec, Fill, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let fills = keyed_by_position([Fill {
            brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(10, 2, 30)).into(),
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
        let gone_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 2, 30));
        let gone_export = format!("{} {}", exports[0].scale.label(), exports[0].format.label());
        let restores = |app: &mut OndinApp| {
            let painted = texts(&settle(app, &ctx));
            [gone_hex.as_str(), gone_export.as_str()]
                .iter()
                .filter_map(|l| ghost_restore(&painted, l))
                .collect::<Vec<_>>()
        };
        let found = restores(&mut f.app);
        assert_eq!(found.len(), 2, "two ghost rows");
        let click = |app: &mut OndinApp, at: egui::Pos2| click(app, &ctx, at);
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

    /// **A list reads item by item against the main's.** The instance recolours the
    /// main's first fill, removes the second and adds its own: a ghost row, a `+`,
    /// a ↺ that takes the recoloured item back alone, and a restore that puts the
    /// whole list back as the main has it.
    #[test]
    fn a_fill_list_marks_restores_and_resets_item_by_item() {
        use ondin_core::{Fill, Keyed, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let solid = |g: u8| Fill {
            brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(10, g, 30)).into(),
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
        let out = settle(&mut f.app, &ctx);
        let painted = texts(&out);
        let gone_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 2, 30));
        assert!(painted.iter().any(|(t, _)| *t == gone_hex), "a ghost row");
        let own_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 3, 30));
        let rect_of = |s: &str| painted.iter().find(|(t, _)| t == s).map(|(_, r)| *r);
        // **The own fill's `+` is on its own row**, not counted among the Fill and
        // Stroke headers' own `+`s. Flip, run: `item_slot`'s `Local` arm painting
        // `""` for `icon::PLUS` fails *"a + on the own fill's row"*.
        let own_row = rect_of(&own_hex).expect("the own fill's row");
        assert!(
            painted.iter().any(|(t, r)| {
                t == icon::PLUS
                    && (r.center().y - own_row.center().y).abs() < 4.0
                    && r.left() > own_row.right()
            }),
            "a + on the own fill's row: {painted:?}"
        );
        let click = |app: &mut OndinApp, at: egui::Pos2| click(app, &ctx, at);
        // **The ghost row is laid out as the live one**: its hex starts where a
        // live row's hex does, and its restore stands in the eye's column. Flips,
        // both run: the label at a 12pt chip at the field's inset fails *"the
        // hex's x"*; the restore inside the field fails *"the eye's column"*.
        let (ghost, live) = (rect_of(&gone_hex).unwrap(), rect_of(&own_hex).unwrap());
        assert!(
            (ghost.left() - live.left()).abs() <= 1.0,
            "the hex's x: ghost {ghost:?}, live {live:?}"
        );
        let ghost_at = ghost_restore(&painted, &gone_hex).expect("the ghost row's restore");
        let eye = painted
            .iter()
            .find(|(t, r)| t == icon::EYE && (r.center().y - live.center().y).abs() < 4.0)
            .map(|(_, r)| r.center())
            .expect("the live row's eye");
        assert!(
            (ghost_at.x - eye.x).abs() <= 1.0,
            "the eye's column: restore {ghost_at:?}, eye {eye:?}"
        );
        // ↺ on the recoloured row first: that item alone.
        let changed_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 9, 30));
        let row = rect_of(&changed_hex).expect("the recoloured row");
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
        assert_eq!(now, vec![main[0].clone(), own.clone()], "that item alone");
        // Then the ghost row's restore: **the whole list as the main has it**, the
        // own fill gone with the rest. Flip, run: restoring the one item
        // (`reset_item`) fails *"the main's list"*.
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..2 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let restore =
            ghost_restore(&texts(&out), &gone_hex).expect("the ghost row's restore, still there");
        click(&mut f.app, restore);
        assert_eq!(fills_of(&f.app, f.ir), main, "the main's list");
    }

    /// The Effects card reads its stack the same way: the instance hides the main's
    /// first effect and removes the second; the first's ↺ under the pointer takes
    /// the main's back alone, and the second is a ghost row whose restore brings the
    /// whole stack back.
    #[test]
    fn an_effect_stack_restores_and_resets_item_by_item() {
        use ondin_core::{Effect, EffectKind, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let [a, b, c, ..] = EffectKind::all_defaults();
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
        // An effect of the instance's own besides, or a restore of the one missing
        // item would read as the whole stack.
        let own = ondin_core::Keyed::new(f.app.session.ids.mint_item(), Effect::new(c));
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetEffects {
                    id: f.ir,
                    effects: vec![hidden, own.clone()],
                }]))
        );
        f.app.collapsed_panels.remove("Effects");
        f.app.session.selection.set_one(f.ir);
        let effects_of = |app: &OndinApp| app.session.doc.get(f.ir).unwrap().effects().to_vec();
        let click = |app: &mut OndinApp, at: egui::Pos2| click(app, &ctx, at);
        let settle = |app: &mut OndinApp| texts(&settle(app, &ctx));
        // The hidden effect's ↺ first: that item alone.
        let row = settle(&mut f.app)
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
            effects_of(&f.app),
            vec![main[0].clone(), own],
            "that item alone"
        );
        // Then the ghost row's restore: the whole stack as the main has it.
        let painted = settle(&mut f.app);
        let restore = ghost_restore(&painted, b.label())
            .unwrap_or_else(|| panic!("no ghost row in {painted:?}"));
        click(&mut f.app, restore);
        assert_eq!(effects_of(&f.app), main, "the main's stack");
    }

    /// The rects of every `glyph` painted below the card header `header`, top to
    /// bottom — a row found by the one control each of its rows carries once.
    fn glyphs_below(
        painted: &[(String, egui::Rect)],
        header: &str,
        glyph: &str,
    ) -> Vec<egui::Rect> {
        let Some(head) = painted.iter().find(|(t, _)| t == header).map(|(_, r)| *r) else {
            return Vec::new();
        };
        let mut v: Vec<egui::Rect> = painted
            .iter()
            .filter(|(t, r)| t == glyph && r.top() > head.bottom())
            .map(|(_, r)| *r)
            .collect();
        v.sort_by(|a, b| a.top().total_cmp(&b.top()));
        v
    }

    /// The ↺ in the trailing slot of the row at `row`, with the pointer moved onto
    /// it — an overridden item's slot shows its dot at rest and ↺ only while the
    /// row is hot (`item_slot`).
    fn row_reset(app: &mut OndinApp, ctx: &egui::Context, row: egui::Rect) -> Option<egui::Pos2> {
        frame(app, ctx, vec![egui::Event::PointerMoved(row.center())]);
        let out = frame(app, ctx, Vec::new());
        texts(&out)
            .into_iter()
            .find(|(t, r)| {
                t == icon::ARROW_COUNTER_CLOCKWISE && (r.center().y - row.center().y).abs() < 4.0
            })
            .map(|(_, r)| r.center())
    }

    /// **The Stroke card reads its list as the Fill card does** (↺ one item,
    /// *Restore* the whole list). Flips, run: the restore arm handed
    /// `reset_item(src, strokes, gone)` fails *"the main's list"*; the ↺ arm
    /// writing nothing fails *"that item alone"*.
    #[test]
    fn a_stroke_list_restores_and_resets_item_by_item() {
        use ondin_core::{Keyed, Stroke, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let rgb = |g: u8| ondin_core::peniko::Color::from_rgb8(10, g, 30);
        let stroke = |g: u8| Stroke {
            brush: ondin_core::Brush::Solid(rgb(g)).into(),
            ..Default::default()
        };
        let strokes_of =
            |app: &OndinApp| app.session.doc.get(f.ir).unwrap().paint().strokes.clone();
        let main = keyed_by_position([stroke(1), stroke(2)]);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetStrokes {
                    id: r,
                    strokes: main.clone(),
                }]))
        );
        assert_eq!(strokes_of(&f.app), main, "the fixture: it propagated");
        let own = Keyed::new(f.app.session.ids.mint_item(), stroke(3));
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetStrokes {
                    id: f.ir,
                    strokes: vec![main[0].map(|_| stroke(9)), own.clone()],
                }]))
        );
        f.app.collapsed_panels.remove("Stroke");
        f.app.session.selection.set_one(f.ir);
        let click = |app: &mut OndinApp, at: egui::Pos2| click(app, &ctx, at);
        let painted = texts(&settle(&mut f.app, &ctx));
        let changed = crate::ui::hex_of(rgb(9));
        let row = painted
            .iter()
            .find(|(t, _)| *t == changed)
            .map(|(_, r)| *r)
            .expect("the recoloured stroke's row");
        let undo = row_reset(&mut f.app, &ctx, row).expect("↺ in the hovered row's slot");
        click(&mut f.app, undo);
        assert_eq!(
            strokes_of(&f.app),
            vec![main[0].clone(), own],
            "that item alone"
        );
        let painted = texts(&settle(&mut f.app, &ctx));
        let restore = ghost_restore(&painted, &crate::ui::hex_of(rgb(2)))
            .unwrap_or_else(|| panic!("no ghost row in {painted:?}"));
        click(&mut f.app, restore);
        assert_eq!(strokes_of(&f.app), main, "the main's list");
    }

    /// **The Layout grid card, on an instance's root frame**, reads its list as the
    /// Fill card does. Flips, run: the card's `list` match answering `None` for a
    /// restore fails *"the main's list"*, and for a ↺ fails *"that item alone"*;
    /// the restore handed `reset_item` fails *"the main's list"*.
    #[test]
    fn a_layout_grid_list_restores_and_resets_item_by_item() {
        use ondin_core::{GridAxis, Keyed, LayoutGrid, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let grid = |count| LayoutGrid {
            count,
            ..LayoutGrid::new(GridAxis::Columns)
        };
        let grids_of = |app: &OndinApp| app.session.doc.get(f.i).unwrap().grids().to_vec();
        let main = keyed_by_position([grid(4), grid(6)]);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetLayoutGrids {
                    id: f.m,
                    grids: main.clone(),
                }]))
        );
        assert_eq!(grids_of(&f.app), main, "the fixture: it propagated");
        let own = Keyed::new(f.app.session.ids.mint_item(), grid(8));
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetLayoutGrids {
                    id: f.i,
                    grids: vec![main[0].map(|_| grid(5)), own],
                }]))
        );
        f.app.collapsed_panels.remove("Layout grid");
        f.app.session.selection.set_one(f.i);
        let click = |app: &mut OndinApp, at: egui::Pos2| click(app, &ctx, at);
        let painted = texts(&settle(&mut f.app, &ctx));
        let rows = glyphs_below(&painted, "LAYOUT GRID", icon::EYE);
        assert_eq!(rows.len(), 2, "two live grids: {painted:?}");
        let undo = row_reset(&mut f.app, &ctx, rows[0]).expect("↺ on the changed grid's row");
        click(&mut f.app, undo);
        assert_eq!(grids_of(&f.app), vec![main[0], own], "that item alone");
        let painted = texts(&settle(&mut f.app, &ctx));
        let restore = ghost_restore(&painted, "Columns · 6")
            .unwrap_or_else(|| panic!("no ghost row in {painted:?}"));
        click(&mut f.app, restore);
        assert_eq!(grids_of(&f.app), main, "the main's list");
    }

    /// **The Export card, on two items**, reads its list as the Fill card does;
    /// `an_emptied_list_still_draws_its_ghost_rows` drives the restore on a list of
    /// one, where the whole list and the one item are the same write. Flips, run:
    /// the ↺ arm answering `None` fails *"that item alone"*; the restore handed
    /// `reset_item(src, &specs, …)` fails *"the main's list"*.
    #[test]
    fn an_export_list_restores_and_resets_item_by_item() {
        use ondin_core::{ExportFormat, ExportScale, ExportSpec, Keyed, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let spec = |format, s| ExportSpec::new(format, ExportScale::Times(s));
        let exports_of = |app: &OndinApp| app.session.doc.get(f.ir).unwrap().exports().to_vec();
        let main = keyed_by_position([spec(ExportFormat::Png, 2.0), spec(ExportFormat::Svg, 1.0)]);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetExports {
                    id: r,
                    exports: main.clone(),
                }]))
        );
        assert_eq!(exports_of(&f.app), main, "the fixture: it propagated");
        let own = Keyed::new(f.app.session.ids.mint_item(), spec(ExportFormat::Jpeg, 4.0));
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetExports {
                    id: f.ir,
                    exports: vec![main[0].map(|_| spec(ExportFormat::Svg, 3.0)), own.clone()],
                }]))
        );
        f.app.collapsed_panels.remove("Export");
        f.app.session.selection.set_one(f.ir);
        let click = |app: &mut OndinApp, at: egui::Pos2| click(app, &ctx, at);
        let painted = texts(&settle(&mut f.app, &ctx));
        let rows = glyphs_below(&painted, "EXPORT", icon::SLIDERS_HORIZONTAL);
        assert_eq!(rows.len(), 2, "two live exports: {painted:?}");
        let undo = row_reset(&mut f.app, &ctx, rows[0]).expect("↺ on the changed export's row");
        click(&mut f.app, undo);
        assert_eq!(
            exports_of(&f.app),
            vec![main[0].clone(), own],
            "that item alone"
        );
        let painted = texts(&settle(&mut f.app, &ctx));
        let gone = format!("{} {}", main[1].scale.label(), main[1].format.label());
        let restore =
            ghost_restore(&painted, &gone).unwrap_or_else(|| panic!("no ghost row in {painted:?}"));
        click(&mut f.app, restore);
        assert_eq!(exports_of(&f.app), main, "the main's list");
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
        // Pinned with the placement beside the insets, as `toggle_pin` writes it:
        // right 12 and top 8 in a 100-wide frame put the rect at (78, 8).
        assert!(f.app.session.commit(Transaction(vec![
            Operation::SetInsets { id: r, insets: pin },
            Operation::SetTransform {
                id: r,
                transform: ondin_core::kurbo::Affine::translate((78.0, 8.0)),
            },
        ])));
        // **The instance wider than its main**, the case that needs the insets in
        // a reset: the same point is `right 172` at 260 wide, and only an explicit
        // `SetInsets` keeps the pin at 12.
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

    /// **A field reset on a pinned layer comes back exact**: X's reset writes the
    /// main's x **and horizontal insets**. Flip: drop the `SetInsets` from
    /// `transform_marks`' resets and *"the insets came back"* fails. ⚠️ **The
    /// instance has to be a different size from its main** (`pinned_and_moved`
    /// widens it) or the flip stays green.
    #[test]
    fn an_x_reset_on_a_pinned_layer_leaves_no_drift() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let pin = pinned_and_moved(&mut f);
        f.app.session.selection.set_one(f.ir);
        let out = settle(&mut f.app, &ctx);
        let at = inks(&out)
            .into_iter()
            .find(|(t, _, c)| t == "X" && *c == theme::text::STRONG)
            .map(|(_, r, _)| r.center())
            .expect("a marked X");
        click(&mut f.app, &ctx, at);
        let doc = &f.app.session.doc;
        assert_eq!(
            doc.get(f.ir).unwrap().insets(),
            &pin,
            "the insets came back"
        );
        // The root's own size is the fixture's override; the copy has none left.
        assert_eq!(ondin_core::reset::overrides(doc, f.ir), Vec::new());
    }

    /// **X's reset writes the horizontal insets and keeps the copy's own vertical
    /// ones.** The copy is pinned `top 20` of its own first, and X's ↺ must leave
    /// it — a vertical pin is an override of its own, not X's. Flip, run: `transform_marks`' X and Y resets
    /// built with `insets(true, true)` fails *"the copy's own top stays"* with
    /// `top 8`.
    #[test]
    fn an_x_reset_keeps_the_copys_own_vertical_pin() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let pin = pinned_and_moved(&mut f);
        let mut own = *f.app.session.doc.get(f.ir).unwrap().insets();
        own.top = Some(ondin_core::LengthPct::Px(20.0));
        assert!(f.app.session.commit(Transaction(vec![Operation::SetInsets {
            id: f.ir,
            insets: own,
        }])));
        f.app.session.selection.set_one(f.ir);
        let out = settle(&mut f.app, &ctx);
        let at = inks(&out)
            .into_iter()
            .find(|(t, _, c)| t == "X" && *c == theme::text::STRONG)
            .map(|(_, r, _)| r.center())
            .expect("a marked X");
        click(&mut f.app, &ctx, at);
        let now = *f.app.session.doc.get(f.ir).unwrap().insets();
        assert_eq!(
            (now.left, now.right),
            (pin.left, pin.right),
            "the main's horizontal pin is back"
        );
        assert_eq!(
            now.top,
            Some(ondin_core::LengthPct::Px(20.0)),
            "the copy's own top stays"
        );
    }

    /// **And the Transform card's header reset**, which `gather_card_overrides`
    /// gives the main's insets for the same reason (through `placement_stated`).
    /// Flip: dropping that append fails *"the insets came back"* with `right 172`.
    #[test]
    fn a_transform_header_reset_on_a_pinned_layer_leaves_no_drift() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let pin = pinned_and_moved(&mut f);
        f.app.session.selection.set_one(f.ir);
        let out = settle(&mut f.app, &ctx);
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
        click(&mut f.app, &ctx, chip);
        let doc = &f.app.session.doc;
        assert_eq!(
            doc.get(f.ir).unwrap().insets(),
            &pin,
            "the insets came back"
        );
        assert_eq!(ondin_core::reset::overrides(doc, f.ir), Vec::new());
    }

    /// **An instance root's insets are its own** — where it sits in its own parent
    /// (`propagate::is_placement`). The instance is pinned inside a frame and
    /// resized wider than its main; *Reset transform* puts the main's size back and
    /// must leave the instance's pin alone.
    #[test]
    fn a_transform_reset_keeps_an_instance_roots_own_pin() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let host = f.app.session.ids.mint();
        let root = f.app.session.doc.first_page();
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
        let out = settle(&mut f.app, &ctx);
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
        click(&mut f.app, &ctx, chip);
        let node = f.app.session.doc.get(f.i).unwrap();
        assert!(
            matches!(node.kind(), NodeKind::Artboard { size } if *size == Size::new(100.0, 100.0)),
            "the main's size is back"
        );
        assert_eq!(node.insets(), &pin, "and the instance's own pin stays");
        // **And through W's own ↺**: widened again, as above, and W's bright label
        // clicked. Flip, run: `transform_marks`' `main_in` read from the source for
        // a root too fails *"W keeps the instance's own pin"*.
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
        let out = settle(&mut f.app, &ctx);
        let w = inks(&out)
            .into_iter()
            .find(|(t, _, c)| t == "W" && *c == theme::text::STRONG)
            .map(|(_, r, _)| r.center())
            .expect("a marked W");
        click(&mut f.app, &ctx, w);
        let node = f.app.session.doc.get(f.i).unwrap();
        assert!(
            matches!(node.kind(), NodeKind::Artboard { size } if *size == Size::new(100.0, 100.0)),
            "W's reset puts the main's width back"
        );
        assert_eq!(node.insets(), &pin, "W keeps the instance's own pin");
    }

    /// **The menu's and the card's *Reset all* state the placement too**: a bare
    /// `SetTransform` on the pinned copy of a wider instance would come back
    /// `right 172`. Driven through `reset_selection` and `Act::Reset(Kind::All)`.
    /// Flip, run: `reset_tx` without `placement_stated` fails *"the menu's reset
    /// keeps the pin"*.
    #[test]
    fn a_reset_all_on_a_pinned_copy_keeps_its_pin() {
        use ondin_core::reset::Kind;
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let main_rect = f.app.session.doc.get(f.m).unwrap().children()[0];
        let pin = ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(12.0)),
            ..Default::default()
        };
        assert!(f.app.session.commit(Transaction(vec![
            Operation::SetInsets {
                id: main_rect,
                insets: pin
            },
            Operation::SetTransform {
                id: main_rect,
                transform: ondin_core::kurbo::Affine::translate((78.0, 8.0)),
            },
        ])));
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetGeometry {
                    id: f.i,
                    geometry: ondin_core::GeometryPatch::Size(Size::new(260.0, 100.0)),
                }]))
        );
        let move_down = |f: &mut F| {
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
                        transform: ondin_core::kurbo::Affine::translate((0.0, 20.0)) * placed,
                    }]))
            );
            let doc = &f.app.session.doc;
            assert_eq!(doc.get(f.ir).unwrap().insets(), &pin, "the fixture");
            assert_eq!(
                ondin_core::reset::overrides(doc, f.ir).len(),
                1,
                "the fixture: the transform alone differs"
            );
        };
        let drawn_x = |f: &F| {
            f.app
                .session
                .resolved
                .used_local_of(f.app.session.doc.get(f.ir).unwrap())
                .translation()
                .x
        };
        move_down(&mut f);
        f.app.session.selection.set_one(f.ir);
        f.app.reset_selection(Kind::All);
        let doc = &f.app.session.doc;
        assert_eq!(
            doc.get(f.ir).unwrap().insets(),
            &pin,
            "the menu's reset keeps the pin"
        );
        assert_eq!(drawn_x(&f), 238.0, "drawn where it was, across");
        assert_eq!(ondin_core::reset::overrides(doc, f.ir), Vec::new());
        move_down(&mut f);
        f.app.session.selection.set_one(f.i);
        f.app.component_act(Some(Act::Reset(Kind::All)));
        let doc = &f.app.session.doc;
        assert_eq!(
            doc.get(f.ir).unwrap().insets(),
            &pin,
            "the card's reset keeps the pin"
        );
        // The root's size is no placement, so it is reset too and the copy, still
        // `right 12`, draws where the main's does.
        assert_eq!(drawn_x(&f), 78.0, "pinned in the main's width");
        assert_eq!(ondin_core::reset::overrides(doc, f.ir), Vec::new());
    }

    /// **An in-flow flex item's *Reset all* lands**: `kept_flow_translations`
    /// would drop a bare write as changing nothing; with the insets stated beside
    /// the transform it stands. Flip, run: `reset_tx` without `placement_stated`
    /// fails *"the override is gone"*.
    #[test]
    fn a_reset_all_on_an_in_flow_item_commits() {
        use ondin_core::container::{Display, Flex};
        use ondin_core::reset::Kind;
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetDisplay {
                    id: f.m,
                    display: Some(Display::Flex(Flex::default())),
                }]))
        );
        assert!(ondin_core::build::is_flex_item(&f.app.session.doc, f.ir));
        let out_of_flow = ondin_core::Insets {
            left: Some(ondin_core::LengthPct::Px(40.0)),
            top: Some(ondin_core::LengthPct::Px(30.0)),
            ..Default::default()
        };
        assert!(f.app.session.commit(Transaction(vec![
            Operation::SetInsets {
                id: f.ir,
                insets: out_of_flow,
            },
            Operation::SetTransform {
                id: f.ir,
                transform: ondin_core::kurbo::Affine::translate((40.0, 30.0)),
            },
        ])));
        assert!(f.app.session.commit(Transaction(vec![Operation::SetInsets {
            id: f.ir,
            insets: Default::default(),
        }])));
        let doc = &f.app.session.doc;
        assert!(
            ondin_core::build::is_flex_item(doc, f.ir),
            "back in the flow"
        );
        assert_eq!(
            ondin_core::reset::overrides(doc, f.ir).len(),
            1,
            "the fixture: the stored translation alone differs"
        );
        f.app.session.selection.set_one(f.i);
        f.app.component_act(Some(Act::Reset(Kind::All)));
        let doc = &f.app.session.doc;
        assert_eq!(
            ondin_core::reset::overrides(doc, f.ir),
            Vec::new(),
            "the override is gone"
        );
        assert!(
            ondin_core::build::is_flex_item(doc, f.ir),
            "still in the flow"
        );
    }

    /// `n`'s copy of the slot swapped to a **Tag** main, 40 × 30 at (300, 40) on
    /// the page — another size and another place than the slot's 30 × 30 at the
    /// Button's origin — with the slot pinned `right 8` in the Button main first.
    /// Answers the Tag main.
    fn swapped_to_a_tag(n: &mut N) -> NodeId {
        let [tag, dot] = [(); 2].map(|_| n.app.session.ids.mint());
        let root = n.app.session.doc.first_page();
        assert!(n.app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: tag,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(40.0, 30.0),
                },
                transform: Some(ondin_core::kurbo::Affine::translate((300.0, 40.0))),
                name: Some("Tag".into()),
            },
            Operation::CreateNode {
                id: dot,
                parent: tag,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(6.0, 6.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Dot".into()),
            },
            Operation::SetComponent {
                id: tag,
                component: true,
            },
        ])));
        assert!(n.app.session.commit(Transaction(vec![
            Operation::SetInsets {
                id: n.slot,
                insets: ondin_core::Insets {
                    right: Some(ondin_core::LengthPct::Px(8.0)),
                    ..Default::default()
                },
            },
            Operation::SetTransform {
                id: n.slot,
                transform: ondin_core::kurbo::Affine::translate((82.0, 0.0)),
            },
        ])));
        let tx = ondin_core::swap::swap(&n.app.session.doc, n.r, tag).expect("a swap");
        assert!(n.app.session.commit(tx));
        let doc = &n.app.session.doc;
        assert_eq!(doc.get(n.r).unwrap().swap(), Some(tag), "the fixture");
        assert_eq!(
            ondin_core::reset::overrides(doc, n.r).len(),
            1,
            "the fixture: the swap alone differs"
        );
        tag
    }

    /// **A swapped slot's Transform marks compare with the slot, not the swap**:
    /// X, Y, W and H are dim, and after a resize *Reset transform* puts the slot's
    /// 30 × 30 back and keeps its `right 8`. Flips, run: `transform_marks` reading
    /// `source_of` fails *"X follows the slot"*; `placement_stated` reading it
    /// fails *"the slot's pin stays"*.
    #[test]
    fn a_swapped_slots_transform_compares_with_the_slot() {
        let ctx = egui::Context::default();
        let mut n = nested_fixture(&ctx);
        swapped_to_a_tag(&mut n);
        n.app.session.selection.set_one(n.r);
        let out = settle(&mut n.app, &ctx);
        let painted = inks(&out);
        for (label, says) in [
            ("X", "X follows the slot"),
            ("Y", "Y follows the slot"),
            ("W", "W follows the slot"),
            ("H", "H follows the slot"),
        ] {
            let ink = painted
                .iter()
                .find(|(t, ..)| t == label)
                .map(|(.., c)| *c)
                .unwrap_or_else(|| panic!("no {label} in {painted:?}"));
            assert_eq!(ink, theme::text::FAINT, "{says}");
        }
        let pin = ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(8.0)),
            ..Default::default()
        };
        assert!(n.app.session.commit(Transaction(vec![
            Operation::SetInsets {
                id: n.r,
                insets: pin
            },
            Operation::SetTransform {
                id: n.r,
                transform: ondin_core::kurbo::Affine::translate((72.0, 0.0)),
            },
            Operation::SetGeometry {
                id: n.r,
                geometry: ondin_core::GeometryPatch::Size(Size::new(40.0, 40.0)),
            },
        ])));
        let out = settle(&mut n.app, &ctx);
        let head = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "TRANSFORM")
            .map(|(_, r)| r.center())
            .expect("the Transform header");
        frame(&mut n.app, &ctx, vec![egui::Event::PointerMoved(head)]);
        let out = frame(&mut n.app, &ctx, Vec::new());
        let chip = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Reset transform")
            .map(|(_, r)| r.center())
            .expect("the size override offers the reset");
        click(&mut n.app, &ctx, chip);
        let node = n.app.session.doc.get(n.r).unwrap();
        assert!(
            matches!(node.kind(), NodeKind::Artboard { size } if *size == Size::new(30.0, 30.0)),
            "the slot's size is back: {:?}",
            node.kind()
        );
        assert_eq!(node.insets(), &pin, "the slot's pin stays");
        assert!(node.swap().is_some(), "and the swap with it");
    }

    /// A layer of `kind` added to `fixture`'s main, and the instance's copy of
    /// it that the commit's structural pass makes: `(in the main, the copy)`.
    fn added_to_main(f: &mut F, kind: NodeKind) -> (NodeId, NodeId) {
        let id = f.app.session.ids.mint();
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::CreateNode {
                    id,
                    parent: f.m,
                    index: 0,
                    kind,
                    transform: None,
                    name: Some("Added".into()),
                }]))
        );
        let doc = &f.app.session.doc;
        let copy = doc
            .get(f.i)
            .unwrap()
            .children()
            .iter()
            .copied()
            .find(|c| doc.get(*c).and_then(|n| n.link()) == Some(id))
            .expect("the instance's copy");
        (id, copy)
    }

    /// The painted text `label`'s centre and its ink.
    fn ink_of(out: &egui::FullOutput, label: &str) -> (egui::Pos2, egui::Color32) {
        inks(out)
            .into_iter()
            .find(|(t, ..)| t == label)
            .map(|(_, r, c)| (r.center(), c))
            .unwrap_or_else(|| panic!("no {label} painted"))
    }

    /// **A text's box width is marked on W.** The main's text wraps at 100 and the
    /// copy's at 180, both auto-height: W is bright, H dim, and W's ↺ writes
    /// `AutoHeight(100)`, the copy's mode kept. Flip, run: the text arm's
    /// `AutoHeight` case removed fails *"W is overridden"*.
    #[test]
    fn a_text_boxs_width_is_marked_on_w() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let mut text = text_kind("Go");
        if let NodeKind::Text { sizing, .. } = &mut text {
            *sizing = ondin_core::TextSizing::AutoHeight(100.0);
        }
        let (_, copy) = added_to_main(&mut f, text);
        let width = |s| Operation::SetGeometry {
            id: copy,
            geometry: ondin_core::GeometryPatch::TextSizing(s),
        };
        assert!(f.app.session.commit(Transaction(vec![width(
            ondin_core::TextSizing::AutoHeight(180.0)
        )])));
        f.app.session.selection.set_one(copy);
        let out = settle(&mut f.app, &ctx);
        let (w, w_ink) = ink_of(&out, "W");
        assert_eq!(w_ink, theme::text::STRONG, "W is overridden");
        assert_eq!(ink_of(&out, "H").1, theme::text::FAINT, "H follows");
        click(&mut f.app, &ctx, w);
        assert!(
            matches!(
                f.app.session.doc.get(copy).unwrap().kind(),
                NodeKind::Text { sizing: ondin_core::TextSizing::AutoHeight(w), .. } if *w == 100.0
            ),
            "the main's width, the mode kept"
        );
    }

    /// **A line's length is marked on `L`, and its direction on R.** The main's
    /// line ends at (50, 0); a copy ending at (30, 0) marks `L`, one at (0, 50)
    /// marks R, and each ↺ writes (50, 0). Flips, run: `L` drawn through the
    /// unmarked `value_field` fails *"L is overridden"*; the direction's mark
    /// removed fails *"R is overridden"*.
    #[test]
    fn a_lines_length_and_direction_are_marked() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let (_, copy) = added_to_main(
            &mut f,
            NodeKind::Line {
                end: ondin_core::kurbo::Point::new(50.0, 0.0),
            },
        );
        let end_at = |f: &mut F, x, y| {
            assert!(
                f.app
                    .session
                    .commit(Transaction(vec![Operation::SetGeometry {
                        id: copy,
                        geometry: ondin_core::GeometryPatch::LineEnd(
                            ondin_core::kurbo::Point::new(x, y)
                        ),
                    }]))
            );
        };
        let end_of = |f: &F| match f.app.session.doc.get(copy).unwrap().kind() {
            NodeKind::Line { end } => *end,
            _ => panic!("a line"),
        };
        // The rotation field's glyph: the first in X's column (the quarter-turn
        // buttons below draw arrows too).
        let rotation = |out: &egui::FullOutput| {
            let x = ink_of(out, "X").0;
            inks(out)
                .into_iter()
                .find(|(t, r, _)| t == icon::ARROW_CLOCKWISE && (r.center().x - x.x).abs() < 8.0)
                .map(|(_, r, c)| (r.center(), c))
                .expect("the rotation field's glyph")
        };
        end_at(&mut f, 30.0, 0.0);
        f.app.session.selection.set_one(copy);
        let out = settle(&mut f.app, &ctx);
        let (l, l_ink) = ink_of(&out, "L");
        assert_eq!(l_ink, theme::text::STRONG, "L is overridden");
        assert_eq!(rotation(&out).1, theme::text::FAINT, "R follows");
        click(&mut f.app, &ctx, l);
        assert_eq!(end_of(&f), ondin_core::kurbo::Point::new(50.0, 0.0));
        end_at(&mut f, 0.0, 50.0);
        let out = settle(&mut f.app, &ctx);
        assert_eq!(ink_of(&out, "L").1, theme::text::FAINT, "L follows");
        let (r, r_ink) = rotation(&out);
        assert_eq!(r_ink, theme::text::STRONG, "R is overridden");
        click(&mut f.app, &ctx, r);
        let end = end_of(&f);
        assert!(
            (end.x - 50.0).abs() < 1e-9 && end.y.abs() < 1e-9,
            "the main's direction: {end:?}"
        );
    }

    /// **Rotation's ↺ turns back about the pivot.** The copy's 10 × 10 rect is
    /// turned 90° about its centre, as the field and the handle turn it — stored
    /// translation (10, 0), its box where it was. R's ↺ writes the main's basis
    /// with the centre held, so the rect is back exactly as the main has it. Flip,
    /// run: R's reset built with `cur[4], cur[5]` fails *"back in place"*.
    #[test]
    fn rotations_reset_turns_back_about_the_pivot() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let main_rect = f.app.session.doc.get(f.m).unwrap().children()[0];
        let turned = ondin_core::kurbo::Affine::rotate_about(
            std::f64::consts::FRAC_PI_2,
            ondin_core::kurbo::Point::new(5.0, 5.0),
        );
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: f.ir,
                    transform: turned,
                }]))
        );
        f.app.session.selection.set_one(f.ir);
        let out = settle(&mut f.app, &ctx);
        let x = ink_of(&out, "X").0;
        let (r, r_ink) = inks(&out)
            .into_iter()
            .find(|(t, rr, _)| t == icon::ARROW_CLOCKWISE && (rr.center().x - x.x).abs() < 8.0)
            .map(|(_, rr, c)| (rr.center(), c))
            .expect("the rotation field's glyph");
        assert_eq!(r_ink, theme::text::STRONG, "R is overridden");
        click(&mut f.app, &ctx, r);
        let doc = &f.app.session.doc;
        let got = doc.get(f.ir).unwrap().transform().as_coeffs();
        let want = doc.get(main_rect).unwrap().transform().as_coeffs();
        assert!(
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-9),
            "back in place: {got:?} against {want:?}"
        );
    }

    /// **A collapsed card keeps its count and offers no chip.** A collapsed card is
    /// one whole-card target registered after its header, which would cover the
    /// chip. Flip: dropping `sense &&` in `section_head_full` draws the chip again.
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
        let out = settle(&mut f.app, &ctx);
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

    /// **A card header counts its overrides and resets them**: `1` at rest, *Reset
    /// appearance* under the pointer, and a click resets and leaves the card open.
    /// Flip: `card_of` mapping opacity nowhere fails *"the count after the label"*.
    /// ⚠️ Dropping `section_head_full`'s `clicks.toggled = false` stays **green**:
    /// egui gives the press to the topmost target only.
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
        let out = settle(&mut f.app, &ctx);
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
        click(&mut f.app, &ctx, at);
        assert_eq!(f.app.session.doc.get(f.ir).unwrap().opacity(), 1.0);
        assert!(
            !f.app.collapsed_panels.contains("Appearance"),
            "still open: the reset is the press, not the toggle"
        );
    }

    /// A set `Button` (`Size: Small, Large`) of two variants, each a frame holding
    /// a text `Label` bound to the set's text property *Label text*, and an
    /// instance of Small — adopted into a headless app.
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
                        preferred: Vec::new(),
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

    /// The variant faces: a set, a variant, a layer inside a variant, and an
    /// instance of a variant.
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

    /// **A property counts as a property, not as an override**: the instance's
    /// label text set through the property reads *1 property* in the summary and
    /// nothing as an override; dimming the label as well adds *1 override*. Flip,
    /// run: dropping `PropDrift::units` from `drift_summary`'s subtraction paints
    /// *1 property · 1 override* in the first case.
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
        let out = settle(&mut v.app, &ctx);
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
        let out = settle(&mut v.app, &ctx);
        let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(
            painted.contains(&"1 property · 1 override".to_string()),
            "{painted:?}"
        );
    }

    /// *Label text* set to "Sign up" on the instance — a property value, not an
    /// override.
    fn signed_up(v: &mut V) {
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
    }

    /// ***Reset fields* leaves a property's value alone, *Reset properties* takes
    /// it back.** The filter is the app's alone; no core test reaches it. Flips,
    /// run: the filter made `true || …` fails *"the property's value stays"*;
    /// `Act::ResetProperties` made a no-op fails *"the main's text"*.
    #[test]
    fn reset_fields_spares_a_property_and_reset_properties_takes_it() {
        use ondin_core::reset::Kind;
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        signed_up(&mut v);
        assert!(
            v.app
                .session
                .commit(Transaction(vec![Operation::SetOpacity {
                    id: v.ilabel,
                    opacity: 0.5,
                }]))
        );
        v.app.session.selection.set_one(v.i);
        v.app.component_act(Some(Act::Reset(Kind::Fields)));
        let doc = &v.app.session.doc;
        assert_eq!(doc.get(v.ilabel).unwrap().opacity(), 1.0, "the field reset");
        assert_eq!(
            content(&v.app, v.ilabel),
            "Sign up",
            "the property's value stays"
        );
        v.app.component_act(Some(Act::ResetProperties));
        assert_eq!(content(&v.app, v.ilabel), "Go", "the main's text");
    }

    /// ***Reset fields* with only a property differing says so** rather than doing
    /// nothing silently: the filtered transaction is empty, so nothing commits and
    /// the status line reads *"Only component properties differ here"*. Flip, run:
    /// the `kept.is_empty()` arm removed fails *"says why"* with an empty status.
    #[test]
    fn reset_fields_with_only_a_property_says_so() {
        use ondin_core::reset::Kind;
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        signed_up(&mut v);
        v.app.session.selection.set_one(v.i);
        let before = v.app.session.doc.clone();
        v.app.component_act(Some(Act::Reset(Kind::Fields)));
        assert!(v.app.session.doc == before, "nothing committed");
        assert_eq!(
            v.app.session.status().text,
            "Only component properties differ here",
            "says why"
        );
    }

    /// **A long main name is cut, and the card keeps its width**: *Go to main*,
    /// the Transform header, *Detach* and the summary stay put. Flips, run: the
    /// child line's label without `.truncate()` fails *"Go to main stays put"*;
    /// the heading's name without it fails *"Detach stays put"*.
    #[test]
    fn a_long_main_name_is_cut_and_the_card_keeps_its_width() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        assert!(f.app.session.commit(Transaction(vec![Operation::SetName {
            id: f.ir,
            name: "Mine".into(),
        }])));
        let at = |f: &mut F, select: NodeId, s: &str| {
            f.app.session.selection.set_one(select);
            let out = settle(&mut f.app, &ctx);
            texts(&out)
                .into_iter()
                .find(|(t, _)| t == s)
                .map(|(_, r)| r)
                .unwrap_or_else(|| panic!("{s} painted"))
        };
        let (i, ir) = (f.i, f.ir);
        let short = [
            at(&mut f, ir, "Go to main"),
            at(&mut f, ir, "TRANSFORM"),
            at(&mut f, i, "Detach"),
            at(&mut f, i, "1 override"),
        ];
        let long = "Large, Secondary, Hover, Disabled, With icon, Dense";
        assert!(long.len() > 50);
        assert!(f.app.session.commit(Transaction(vec![Operation::SetName {
            id: f.m,
            name: long.into(),
        }])));
        assert_eq!(
            at(&mut f, ir, "Go to main"),
            short[0],
            "Go to main stays put"
        );
        assert_eq!(
            at(&mut f, ir, "TRANSFORM"),
            short[1],
            "the cards below keep the column"
        );
        assert_eq!(at(&mut f, i, "Detach"), short[2], "Detach stays put");
        assert_eq!(
            at(&mut f, i, "1 override"),
            short[3],
            "the summary is drawn where it was"
        );
    }

    /// The layers panel's two marks: a collapsed set's row
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

    /// **A Type reset restyles a live text session** (`commit_reset`'s second
    /// point): the reset reaches the **editor**, so its next keystroke draws in the
    /// main's size. ⚠️ **It reads the editor's own layout, not the node's style**,
    /// which stays the document's. Flip, run: `commit_reset` without its
    /// `text_session_restyled_after` fails at the last assertion.
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
        assert!(
            height(&v.app) < before,
            "the editor laid out at 12, not 30: {} against {before}",
            height(&v.app)
        );
    }

    /// **The layers panel's component marks**: an instance's row wears the outline
    /// hexagon; an override puts a dot on its row, bubbling up to a **collapsed**
    /// instance; a layer of the instance's own wears `+`; an untouched instance
    /// wears neither. Flip: the collapsed arm reading only the row's own overrides
    /// leaves the collapsed instance without its dot.
    #[test]
    fn the_layers_panel_marks_overrides_and_local_layers() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        // Each mark's shapes: a dot is the override dot, a `+` two 1.2pt strokes.
        let marks = |app: &mut OndinApp| {
            let mut out = None;
            for _ in 0..3 {
                out = Some(ctx.run_ui(Default::default(), |ui| app.layers_tree(ui)));
            }
            let out = out.expect("drawn");
            fn walk(s: &egui::Shape, strokes: &mut usize) {
                match s {
                    egui::Shape::LineSegment { stroke, .. } if stroke.width == 1.2 => *strokes += 1,
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, strokes)),
                    _ => {}
                }
            }
            let dots = dots(&out).len();
            let mut strokes = 0;
            for s in &out.shapes {
                walk(&s.shape, &mut strokes);
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
                        corner_smoothing: 0.0,
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
        let out = settle(&mut v.app, &ctx);
        let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        // The counts are the title's and the name line's.
        for want in ["VARIANTS 2", "1 instance"] {
            assert!(painted.contains(&want.to_string()), "{want}: {painted:?}");
        }
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

    /// A **Badge** main with a text *Count* bound to a Text property, nested as
    /// `slot` inside a **Button** main, and `b1` an instance of Button whose copy
    /// of the slot is `r`.
    struct N {
        app: OndinApp,
        button: NodeId,
        slot: NodeId,
        b1: NodeId,
        r: NodeId,
    }

    fn nested_fixture(ctx: &egui::Context) -> N {
        use ondin_core::variant::{PropKind, Property};
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xC7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [badge, count, button, label] = [(); 4].map(|_| ids.mint());
        let create = |id, parent, index, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        let frame = |w| NodeKind::Artboard {
            size: Size::new(w, 30.0),
        };
        let item = ids.mint_item();
        doc.apply(&Transaction(vec![
            create(badge, root, 0, frame(30.0), "Badge"),
            create(count, badge, 0, text_kind("1"), "Count"),
            create(button, root, 1, frame(120.0), "Button"),
            create(
                label,
                button,
                0,
                NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                "Label",
            ),
            Operation::SetComponent {
                id: badge,
                component: true,
            },
            Operation::SetProperties {
                id: badge,
                props: vec![ondin_core::Keyed::new(
                    item,
                    Property {
                        name: "Count".into(),
                        kind: PropKind::Text,
                        bound: vec![count],
                        filter: String::new(),
                        preferred: Vec::new(),
                    },
                )],
            },
        ]))
        .expect("a Badge main and a Button frame");
        let place = |doc: &Document, ids: &mut IdSource, main, parent| {
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                ids,
                &[Placement {
                    nodes: doc.capture_subtree(main).unwrap(),
                    parent,
                    index: None,
                }],
                Default::default(),
            );
            (tx, made[0])
        };
        let (tx, slot) = place(&doc, &mut ids, badge, button);
        doc.apply(&tx).expect("a Badge inside the Button");
        doc.apply(&Transaction(vec![Operation::SetComponent {
            id: button,
            component: true,
        }]))
        .expect("the Button a main");
        let (tx, b1) = place(&doc, &mut ids, button, root);
        doc.apply(&tx).expect("an instance of the Button");
        let r = doc
            .get(b1)
            .unwrap()
            .children()
            .iter()
            .copied()
            .find(|c| doc.get(*c).and_then(|n| n.link()) == Some(slot))
            .expect("b1's copy of the Badge");
        app.session.adopt_document(doc, None);
        N {
            app,
            button,
            slot,
            b1,
            r,
        }
    }

    /// **The opt-in, from the nested instance's own card, and the main's list**:
    /// the Badge's card inside the Button main draws the binding line and *Show
    /// properties on instances*; a click shows the slot, and the main's Properties
    /// card lists it under *Shown from nested*. Flips, run: the switch's call
    /// taken out of the instance face fails *"the switch is drawn"*; the binding
    /// line's condition made never true fails *"the binding line"*.
    #[test]
    fn a_nested_instance_in_a_main_opts_in_from_its_card() {
        let ctx = egui::Context::default();
        let mut n = nested_fixture(&ctx);
        n.app.session.selection.set_one(n.slot);
        let out = settle(&mut n.app, &ctx);
        let painted = texts(&out);
        let has = |s: &str| painted.iter().any(|(t, _)| t == s);
        assert!(
            has("Bind to a component property"),
            "the binding line: {painted:?}"
        );
        let at = painted
            .iter()
            .find(|(t, _)| t == "Show properties on instances")
            .map(|(_, r)| r.center())
            .expect("the switch is drawn");
        assert!(has("Count appears on every Button"), "{painted:?}");
        click(&mut n.app, &ctx, at);
        assert!(ondin_core::variant::slot_is_shown(
            &n.app.session.doc,
            n.slot
        ));
        n.app.session.selection.set_one(n.button);
        let out = settle(&mut n.app, &ctx);
        let painted = texts(&out);
        let has = |s: &str| painted.iter().any(|(t, _)| t == s);
        assert!(has("SHOWN FROM NESTED"), "{painted:?}");
        assert!(has("· Count"), "{painted:?}");
    }

    /// **A shown nested instance's rows on the outer instance's card**: `b1`'s
    /// card draws the Badge's group, and the copy's count set to 3 reads as one
    /// **property**, not an override. Flips, run: `instance_rows` not drawing the
    /// groups fails *"the group's row"*; `prop_drift` without the shown rows fails
    /// *"1 property"* with **no summary at all**, since `property_fields` still
    /// subtracts the count's field from the overrides.
    #[test]
    fn a_shown_nested_instance_draws_its_rows_on_the_outer_card() {
        use ondin_core::variant::{self, PropValue};
        let ctx = egui::Context::default();
        let mut n = nested_fixture(&ctx);
        let tx = variant::set_shown(&n.app.session.doc, &mut n.app.session.ids, n.slot, true)
            .expect("a showing");
        assert!(n.app.session.commit(tx));
        let p = variant::instance_properties(&n.app.session.doc, n.r)[0]
            .value
            .clone();
        let ops =
            variant::set_property(&n.app.session.doc, &[n.r], &p, &PropValue::Text("3".into()));
        assert!(n.app.session.commit(Transaction(ops)));
        n.app.session.selection.set_one(n.b1);
        let out = settle(&mut n.app, &ctx);
        let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(
            painted.contains(&"Count".to_string()),
            "the group's row: {painted:?}"
        );
        assert!(
            painted.contains(&"· Badge".to_string()),
            "the group's main: {painted:?}"
        );
        assert!(painted.contains(&"1 property".to_string()), "{painted:?}");
        assert!(
            !painted.iter().any(|t| t.contains("override")),
            "nothing counted as an override: {painted:?}"
        );
    }

    /// **A group whose copy shows a variant names its set** (*· Badges*, not the
    /// variant's derived name). Flip: `shown_groups`' `shown_as` naming the main
    /// itself → fails at *"the group names the set"*.
    #[test]
    fn a_shown_variant_is_named_by_its_set() {
        use ondin_core::variant::{self, VariantProp, VariantSet};
        let ctx = egui::Context::default();
        let mut n = nested_fixture(&ctx);
        let badge = n
            .app
            .session
            .doc
            .get(n.slot)
            .and_then(|s| s.link())
            .expect("the slot is a Badge instance");
        let set = n.app.session.ids.mint();
        let root = n.app.session.doc.first_page();
        let index = n.app.session.doc.get(root).unwrap().children().len();
        assert!(n.app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: set,
                parent: root,
                index,
                kind: NodeKind::Artboard {
                    size: Size::new(200.0, 100.0),
                },
                transform: None,
                name: Some("Badges".into()),
            },
            Operation::Reparent {
                id: badge,
                new_parent: set,
                index: 0,
            },
            Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![VariantProp {
                        name: "Style".into(),
                        values: vec!["Dot".into()],
                    }],
                }),
            },
            Operation::SetVariant {
                id: badge,
                values: vec!["Dot".into()],
            },
        ])));
        assert_eq!(variant::set_of(&n.app.session.doc, badge), Some(set));
        let tx = variant::set_shown(&n.app.session.doc, &mut n.app.session.ids, n.slot, true)
            .expect("a showing");
        assert!(n.app.session.commit(tx));
        n.app.session.selection.set_one(n.b1);
        let out = settle(&mut n.app, &ctx);
        let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(
            painted.contains(&"· Badges".to_string()),
            "the group names the set: {painted:?}"
        );
        assert!(
            !painted.contains(&"· Dot".to_string()),
            "not the variant: {painted:?}"
        );
    }

    /// Every filled rect the frame painted — the fields, the dropdowns' faces and
    /// the switch's track.
    fn fills(out: &egui::FullOutput) -> Vec<(egui::Rect, egui::Color32)> {
        fn walk(s: &egui::Shape, o: &mut Vec<(egui::Rect, egui::Color32)>) {
            match s {
                egui::Shape::Rect(r) if r.fill.a() > 0 => o.push((r.rect, r.fill)),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, o)),
                _ => {}
            }
        }
        let mut o = Vec::new();
        for s in &out.shapes {
            walk(&s.shape, &mut o);
        }
        o
    }

    /// The variants fixture with a boolean property, *Interesante*, bound to the
    /// small variant's label.
    fn with_a_toggle(v: &mut V) {
        use ondin_core::variant::{PropKind, Property};
        let mut props = v.app.session.doc.get(v.set).unwrap().props().to_vec();
        let item = v.app.session.ids.mint_item();
        props.push(ondin_core::Keyed::new(
            item,
            Property {
                name: "Interesante".into(),
                kind: PropKind::Boolean,
                bound: vec![v.label],
                filter: String::new(),
                preferred: Vec::new(),
            },
        ));
        assert!(
            v.app
                .session
                .commit(Transaction(vec![Operation::SetProperties {
                    id: v.set,
                    props,
                }]))
        );
    }

    /// **A text typed into an instance's property field survives a click on the
    /// toggle below it**: both writes land on the one frame. Flip run: `own_rows`
    /// keeping one `Option` of the rows' writes fails *"the typed text survived the
    /// toggle's click"*, the label still *Go*.
    #[test]
    fn a_typed_property_survives_a_click_on_the_toggle_below_it() {
        use ondin_core::variant::{PropValue, property_state};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        with_a_toggle(&mut v);
        let out = settled(&mut v.app, &ctx, v.i);
        let painted = texts(&out);
        let at = |s: &str| {
            painted
                .iter()
                .find(|(t, _)| t == s)
                .unwrap_or_else(|| panic!("{s} painted: {painted:?}"))
                .1
        };
        let rects: Vec<egui::Rect> = fills(&out).into_iter().map(|(r, _)| r).collect();
        let label = at("Label text");
        let field = *rects
            .iter()
            .find(|r| {
                r.height() == ui::CONTROL_H
                    && r.left() > label.right()
                    && (r.center().y - label.center().y).abs() < 4.0
            })
            .expect("the Label text field");
        let toggle = at("Interesante");
        let switch = *rects
            .iter()
            .find(|r| r.size() == ui::SWITCH_SIZE && (r.center().y - toggle.center().y).abs() < 4.0)
            .expect("the switch's track");
        let prop = |v: &V, name: &str| {
            let doc = &v.app.session.doc;
            let p = doc
                .get(v.set)
                .unwrap()
                .props()
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .clone();
            property_state(doc, v.i, &p).unwrap().0
        };
        let before = match prop(&v, "Label text") {
            PropValue::Text(t) => t,
            other => panic!("a text property: {other:?}"),
        };
        assert_eq!(
            prop(&v, "Interesante"),
            PropValue::Boolean(true),
            "on at rest"
        );
        click(&mut v.app, &ctx, field.center());
        frame(
            &mut v.app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::End,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        frame(&mut v.app, &ctx, vec![egui::Event::Text("!".into())]);
        click(&mut v.app, &ctx, switch.center());
        settle(&mut v.app, &ctx);
        assert_eq!(
            prop(&v, "Interesante"),
            PropValue::Boolean(false),
            "the toggle's click landed"
        );
        assert_eq!(
            prop(&v, "Label text"),
            PropValue::Text(format!("{before}!")),
            "the typed text survived the toggle's click"
        );
    }

    /// **A boolean property over a bound layer shows the chip**: the switch sits in
    /// a 38×22 capsule when bound, not when unbound, and its click writes past the
    /// binding (hidden and detached, as the identity card's eye does). Flip, run:
    /// `toggle_row` handed `false` for `bound` fails *"bound: the capsule"*.
    #[test]
    fn a_boolean_property_over_a_bound_layer_shows_the_chip() {
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        with_a_toggle(&mut v);
        let cap = ui::SWITCH_SIZE + egui::vec2(12.0, 6.0);
        let capsules = |out: &egui::FullOutput| {
            fills(out)
                .into_iter()
                .filter(|(r, _)| r.size() == cap)
                .map(|(r, _)| r)
                .collect::<Vec<_>>()
        };
        let out = settled(&mut v.app, &ctx, v.i);
        assert!(capsules(&out).is_empty(), "unbound: no capsule");

        let p = v
            .app
            .session
            .doc
            .get(v.set)
            .unwrap()
            .props()
            .iter()
            .find(|p| p.name == "Interesante")
            .unwrap()
            .value
            .clone();
        let c = ondin_core::variant::counterparts(&v.app.session.doc, v.i, &p)[0];
        let (mode, on) = (v.app.session.ids.mint_mode(), v.app.session.ids.mint_var());
        assert!(v.app.commit_edit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: on,
                    name: "state/on".into(),
                    kind: VarKind::Boolean,
                    cells: [(mode, Cell::Boolean(true))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        let ops = ondin_core::bind_visible_ops(&v.app.session.doc, c, on);
        assert!(v.app.commit_edit(Transaction(ops)));
        assert_eq!(v.app.session.doc.get(c).unwrap().visible_var(), Some(on));

        let out = settled(&mut v.app, &ctx, v.i);
        let found = capsules(&out);
        assert_eq!(found.len(), 1, "bound: the capsule");
        v.app.session.info("");
        click(&mut v.app, &ctx, found[0].center());
        settle(&mut v.app, &ctx);
        let doc = &v.app.session.doc;
        assert!(!doc.shown(c), "the switch's click hid the layer");
        assert_eq!(doc.get(c).unwrap().visible_var(), None, "and detached it");
        // Asked for — the tooltip says so — so said nothing: not
        // `keep_visible_bindings`' *Detached state/on*.
        assert_eq!(
            v.app.session.status().text,
            "",
            "an asked-for detach is silent"
        );
    }

    /// **A Text property over a bound content shows the chip** (the variable's
    /// name) where the field was. `Backspace` on the focused chip detaches the copy
    /// to the string it draws; a typed key starts the replacement text, committed
    /// on `Enter` as an override that detaches. Flips, run: `bound_chip`'s Text
    /// getter answering `None` fails *"bound: the chip"*; the chip's typed key
    /// dropped instead of held under `retype` fails *"the typed text"* at `""`.
    #[test]
    fn a_text_property_over_a_bound_content_shows_the_chip() {
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let named = |out: &egui::FullOutput| {
            texts(out)
                .into_iter()
                .find(|(t, _)| t == "copy/cta")
                .map(|(_, r)| r)
        };
        let out = settled(&mut v.app, &ctx, v.i);
        assert!(named(&out).is_none(), "unbound: no chip");

        let (mode, var) = (v.app.session.ids.mint_mode(), v.app.session.ids.mint_var());
        assert!(v.app.commit_edit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "copy/cta".into(),
                    kind: VarKind::String,
                    cells: [(mode, Cell::String("Sign in".into()))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        let vars = ondin_core::variable::text_vars_at(ondin_core::variable::TEXT_CONTENT, var);
        let ops = ondin_core::build::bind_text_ops(&v.app.session.doc, v.ilabel, vars);
        assert!(v.app.commit_edit(Transaction(ops)));
        assert_eq!(
            v.app.session.doc.get(v.ilabel).unwrap().text_vars()[0],
            Some(var)
        );

        let out = settled(&mut v.app, &ctx, v.i);
        let chip = named(&out).expect("bound: the chip");
        let key = |k| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        click(&mut v.app, &ctx, chip.center());
        frame(&mut v.app, &ctx, vec![key(egui::Key::Backspace)]);
        settle(&mut v.app, &ctx);
        let doc = &v.app.session.doc;
        assert_eq!(doc.get(v.ilabel).unwrap().text_vars()[0], None, "detached");
        assert_eq!(
            content(&v.app, v.ilabel),
            "Sign in",
            "to the string it drew"
        );

        // Bound again, a key typed replaces the text.
        let vars = ondin_core::variable::text_vars_at(ondin_core::variable::TEXT_CONTENT, var);
        let ops = ondin_core::build::bind_text_ops(&v.app.session.doc, v.ilabel, vars);
        assert!(v.app.commit_edit(Transaction(ops)));
        let out = settled(&mut v.app, &ctx, v.i);
        let chip = named(&out).expect("bound again: the chip");
        v.app.session.info("");
        click(&mut v.app, &ctx, chip.center());
        frame(&mut v.app, &ctx, vec![egui::Event::Text("N".into())]);
        settle(&mut v.app, &ctx);
        frame(&mut v.app, &ctx, vec![egui::Event::Text("o".into())]);
        frame(&mut v.app, &ctx, vec![key(egui::Key::Enter)]);
        settle(&mut v.app, &ctx);
        let doc = &v.app.session.doc;
        assert_eq!(content(&v.app, v.ilabel), "No", "the typed text");
        assert_eq!(
            doc.get(v.ilabel).unwrap().text_vars()[0],
            None,
            "and detached"
        );
        // Detached in the edit itself, as the Content chip's retype is, so the
        // status says nothing.
        assert_eq!(
            v.app.session.status().text,
            "",
            "an asked-for detach is silent"
        );
    }

    /// **Over several layers**, one bound and one not draws no capsule; both
    /// bound, to **two** variables, draws it. Flip, run: the chip read as the first
    /// counterpart's variable shared by all fails *"two variables: the capsule"*.
    #[test]
    fn a_boolean_property_over_layers_bound_apart_follows_a4() {
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        with_a_toggle(&mut v);
        let dot = v.app.session.ids.mint();
        assert!(v.app.commit_edit(Transaction(vec![Operation::CreateNode {
            id: dot,
            parent: v.small,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(4.0, 4.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Dot".into()),
        }])));
        let mut props = v.app.session.doc.get(v.set).unwrap().props().to_vec();
        let at = props.iter().position(|p| p.name == "Interesante").unwrap();
        props[at].value.bound.push(dot);
        assert!(
            v.app
                .commit_edit(Transaction(vec![Operation::SetProperties {
                    id: v.set,
                    props: props.clone(),
                }]))
        );
        let p = props[at].value.clone();
        let cps = ondin_core::variant::counterparts(&v.app.session.doc, v.i, &p);
        assert_eq!(
            cps.len(),
            2,
            "the property views two layers of the instance"
        );

        let mode = v.app.session.ids.mint_mode();
        let [a, b] = [(); 2].map(|_| v.app.session.ids.mint_var());
        let var = |id, name: &str| Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Boolean,
                cells: [(mode, Cell::Boolean(true))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        assert!(v.app.commit_edit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            var(a, "state/a"),
            var(b, "state/b"),
        ])));
        let cap = ui::SWITCH_SIZE + egui::vec2(12.0, 6.0);
        let capsules = |out: &egui::FullOutput| {
            fills(out)
                .into_iter()
                .filter(|(r, _)| r.size() == cap)
                .count()
        };

        let ops = ondin_core::bind_visible_ops(&v.app.session.doc, cps[0], a);
        assert!(v.app.commit_edit(Transaction(ops)));
        let out = settled(&mut v.app, &ctx, v.i);
        assert_eq!(capsules(&out), 0, "one bound and one not: no capsule");

        let ops = ondin_core::bind_visible_ops(&v.app.session.doc, cps[1], b);
        assert!(v.app.commit_edit(Transaction(ops)));
        let out = settled(&mut v.app, &ctx, v.i);
        assert_eq!(capsules(&out), 1, "two variables: the capsule");
    }

    /// Click into the field showing `text`, send End and type `typed`, leaving it
    /// focused; then click the × on the row showing `other` — the two commits
    /// falling on one frame.
    fn rename_then_delete(
        app: &mut OndinApp,
        ctx: &egui::Context,
        text: &str,
        typed: &str,
        other: &str,
    ) {
        let out = settle(app, ctx);
        let painted = texts(&out);
        let at = |s: &str| {
            painted
                .iter()
                .find(|(t, _)| t == s)
                .unwrap_or_else(|| panic!("{s} painted: {painted:?}"))
                .1
        };
        let field = at(text);
        let row = at(other);
        let x = painted
            .iter()
            .find(|(t, r)| t == icon::X && (r.center().y - row.center().y).abs() < 4.0)
            .unwrap_or_else(|| panic!("the × on {other}'s row: {painted:?}"))
            .1
            .center();
        click(app, ctx, field.center());
        frame(
            app,
            ctx,
            vec![egui::Event::Key {
                key: egui::Key::End,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        frame(app, ctx, vec![egui::Event::Text(typed.into())]);
        click(app, ctx, x);
        settle(app, ctx);
    }

    /// **A variant property renamed and left focused survives a click on another
    /// property's × on the Variants card.** The set's *Size* typed to *Size2*, then
    /// *Style*'s × clicked: *Style* is gone and the rename landed. Both writes
    /// rewrite the whole `VariantSet`, so the second has to be built against the
    /// document the first left.
    #[test]
    fn a_variant_property_rename_survives_a_click_on_another_delete() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let tx = ondin_core::variant::add_property(&v.app.session.doc, v.set, "Style", "Primary")
            .expect("a second property");
        assert!(v.app.session.commit(tx));
        v.app.session.selection.set_one(v.set);
        rename_then_delete(&mut v.app, &ctx, "Size", "2", "Style");
        let names: Vec<String> = v
            .app
            .session
            .doc
            .get(v.set)
            .unwrap()
            .set()
            .unwrap()
            .props
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(names, ["Size2"], "Style deleted and Size renamed");
    }

    /// **A rename and then a click on its own row's control act on the renamed
    /// thing** (a property's ×, a value's *Move later*). A set's properties and
    /// values have no ids, so the deferred delete and move find theirs by the name
    /// they were drawn with, followed through the frame's renames. Flips, run, each
    /// alone: `Renames::prop` answering the name it is given fails *"Size2 deleted
    /// by its own ×"*; `Renames::value` fails *"Small2 moved later by its own menu"*.
    #[test]
    fn a_rename_then_its_own_rows_control_acts_on_the_renamed_thing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let tx = ondin_core::variant::add_property(&v.app.session.doc, v.set, "Style", "Primary")
            .expect("a second property");
        assert!(v.app.session.commit(tx));
        v.app.session.selection.set_one(v.set);
        let props = |v: &V| {
            v.app
                .session
                .doc
                .get(v.set)
                .unwrap()
                .set()
                .unwrap()
                .props
                .clone()
        };
        rename_then_delete(&mut v.app, &ctx, "Size", "2", "Size");
        let names: Vec<String> = props(&v).iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["Style"], "Size2 deleted by its own ×");

        let mut v = variants_fixture(&ctx);
        let before = props(&v)[0].values.clone();
        assert_eq!(before, ["Small", "Large"], "the fixture's order");
        let painted = texts(&settled(&mut v.app, &ctx, v.set));
        let chip = painted
            .iter()
            .find(|(t, _)| t == "Small")
            .unwrap()
            .1
            .center();
        click(&mut v.app, &ctx, chip);
        // The menu's field is the last *Small* painted; the chip the first.
        let painted = texts(&settle(&mut v.app, &ctx));
        let field = painted
            .iter()
            .rev()
            .find(|(t, _)| t == "Small")
            .unwrap()
            .1
            .center();
        click(&mut v.app, &ctx, field);
        frame(
            &mut v.app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::End,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        frame(&mut v.app, &ctx, vec![egui::Event::Text("2".into())]);
        let painted = texts(&settle(&mut v.app, &ctx));
        let later = painted
            .iter()
            .find(|(t, _)| t == "Move later")
            .unwrap_or_else(|| panic!("the chip's menu: {painted:?}"))
            .1
            .center();
        click(&mut v.app, &ctx, later);
        settle(&mut v.app, &ctx);
        assert_eq!(
            props(&v)[0].values,
            ["Large", "Small2"],
            "Small2 moved later by its own menu"
        );
    }

    /// **A used value renamed in its menu and then *Delete value* clicked asks
    /// about the renamed value**, not the name as drawn. Flip, run: the
    /// confirmation stored as drawn fails *"the modal names Small2"* with *Small*.
    #[test]
    fn a_used_value_renamed_then_deleted_is_asked_about_by_its_new_name() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let painted = texts(&settled(&mut v.app, &ctx, v.set));
        let chip = painted
            .iter()
            .find(|(t, _)| t == "Small")
            .unwrap()
            .1
            .center();
        click(&mut v.app, &ctx, chip);
        let painted = texts(&settle(&mut v.app, &ctx));
        let field = painted
            .iter()
            .rev()
            .find(|(t, _)| t == "Small")
            .unwrap()
            .1
            .center();
        click(&mut v.app, &ctx, field);
        frame(
            &mut v.app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::End,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        frame(&mut v.app, &ctx, vec![egui::Event::Text("2".into())]);
        let painted = texts(&settle(&mut v.app, &ctx));
        let delete = painted
            .iter()
            .find(|(t, _)| t == "Delete value")
            .unwrap_or_else(|| panic!("the chip's menu: {painted:?}"))
            .1
            .center();
        click(&mut v.app, &ctx, delete);
        settle(&mut v.app, &ctx);
        let values = v.app.session.doc.get(v.set).unwrap().set().unwrap().props[0]
            .values
            .clone();
        assert_eq!(values, ["Small2", "Large"], "fixture: the rename landed");
        let asked = v
            .app
            .deleting_value
            .as_ref()
            .expect("a used value is asked about");
        assert_eq!(asked.name, "Small2", "the modal names Small2");
        assert_eq!((asked.prop, asked.value), (0, 0), "and points at it");
    }

    /// **A text property's ↺ clicked with text typed into its field resets it**;
    /// the field must not commit after the reset on the same frame. Flip, run: the
    /// field's commit kept on a reset frame fails *"the reset wins"* with *Hi!*.
    #[test]
    fn a_text_rows_reset_wins_over_its_own_typed_field() {
        use ondin_core::variant::{PropValue, property_state, set_property};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let prop = |v: &V| {
            let doc = &v.app.session.doc;
            let p = doc
                .get(v.set)
                .unwrap()
                .props()
                .iter()
                .find(|p| p.name == "Label text")
                .unwrap()
                .clone();
            (p.clone(), property_state(doc, v.i, &p).unwrap().0)
        };
        let (p, main_value) = prop(&v);
        let tx = set_property(
            &v.app.session.doc,
            &[v.i],
            &p,
            &PropValue::Text("Hi".into()),
        );
        assert!(v.app.session.commit(Transaction(tx)));
        assert_eq!(
            prop(&v).1,
            PropValue::Text("Hi".into()),
            "fixture: overridden"
        );

        let out = settled(&mut v.app, &ctx, v.i);
        let painted = texts(&out);
        let label = painted
            .iter()
            .find(|(t, _)| t == "Label text")
            .unwrap_or_else(|| panic!("the row: {painted:?}"))
            .1;
        let rects: Vec<egui::Rect> = fills(&out).into_iter().map(|(r, _)| r).collect();
        let field = *rects
            .iter()
            .find(|r| {
                r.height() == ui::CONTROL_H
                    && r.left() > label.right()
                    && (r.center().y - label.center().y).abs() < 4.0
            })
            .expect("the Label text field");
        click(&mut v.app, &ctx, field.center());
        frame(
            &mut v.app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::End,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        frame(&mut v.app, &ctx, vec![egui::Event::Text("!".into())]);
        click(&mut v.app, &ctx, label.center());
        settle(&mut v.app, &ctx);
        assert_eq!(prop(&v).1, main_value, "the reset wins");
    }

    /// **A component property renamed and left focused survives a click on another
    /// property's × on the Properties card.** The Button's *Shown* typed to
    /// *Shown2*, then *Words*' × clicked: *Words* is gone and the rename landed —
    /// both writes are the owner's whole list.
    #[test]
    fn a_component_property_rename_survives_a_click_on_another_delete() {
        use ondin_core::variant::{PropKind, define};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut n = nested_fixture(&ctx);
        for (name, kind) in [("Shown", PropKind::Boolean), ("Words", PropKind::Text)] {
            let (tx, _) = define(
                &n.app.session.doc,
                &mut n.app.session.ids,
                n.button,
                name,
                kind,
                Vec::new(),
            )
            .expect("a property");
            assert!(n.app.session.commit(tx));
        }
        n.app.session.selection.set_one(n.button);
        rename_then_delete(&mut n.app, &ctx, "Shown", "2", "Words");
        let names: Vec<String> = n
            .app
            .session
            .doc
            .get(n.button)
            .unwrap()
            .props()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(names, ["Shown2"], "Words deleted and Shown renamed");
    }

    /// The frame after a few settling passes with `id` selected.
    fn settled(app: &mut OndinApp, ctx: &egui::Context, id: NodeId) -> egui::FullOutput {
        app.session.selection.set_one(id);
        settle(app, ctx)
    }

    /// **The Component card's rows are the other cards' rows**: fields and
    /// dropdowns are `CONTROL_H` tall and end at the content edge where the ⋯
    /// ends, the labels end at one x against their controls, the switch starts
    /// where the fields do, and the instance's name sits at the height a main's
    /// does. Flips, run, each alone: `control_height` not called fails *"28 tall"*;
    /// the `- 8.0` back on the variant dropdown fails *"at the content edge"*;
    /// `row_label` back on `label_mark` fails *"one right edge"*; the heading's
    /// `interact_size.y = 0.0` removed fails the name's height.
    #[test]
    fn the_component_cards_rows_line_up_with_the_other_cards() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        with_a_toggle(&mut v);
        let out = settled(&mut v.app, &ctx, v.i);
        let painted = texts(&out);
        let at = |s: &str| {
            painted
                .iter()
                .find(|(t, _)| t == s)
                .unwrap_or_else(|| panic!("{s} painted: {painted:?}"))
                .1
        };
        let rects = fills(&out);
        let more = at(icon::DOTS_THREE);
        let edge = rects
            .iter()
            .map(|(r, _)| *r)
            .find(|r| r.contains(more.center()) && r.height() == ui::CONTROL_H)
            .expect("the ⋯ button's face")
            .right();
        let size = at("Size");
        let column: Vec<egui::Rect> = rects
            .iter()
            .map(|(r, _)| *r)
            .filter(|r| r.left() > size.right() && r.top() >= size.top() - 10.0)
            .filter(|r| r.top() < at("Interesante").top() - 10.0 && r.width() > 100.0)
            .collect();
        assert_eq!(
            column.len(),
            2,
            "the dropdown and the text field: {column:?}"
        );
        for r in &column {
            assert_eq!(r.height(), ui::CONTROL_H, "28 tall: {r:?}");
            assert_eq!(r.right(), edge, "at the content edge: {r:?}");
        }
        let rights = [
            at("Size").right(),
            at("Label text").right(),
            at("Interesante").right(),
        ];
        assert!(
            rights.iter().all(|r| (r - rights[0]).abs() <= 1.0),
            "one right edge: {rights:?}"
        );
        let toggle = at("Interesante");
        let switch = rects
            .iter()
            .map(|(r, _)| *r)
            .find(|r| r.size() == ui::SWITCH_SIZE && (r.center().y - toggle.center().y).abs() < 4.0)
            .expect("the switch's track");
        assert_eq!(
            switch.left(),
            column[0].left(),
            "the switch where the fields begin"
        );
        // The name's top, against a main's in its own card — each the nearest name
        // under the COMPONENT eyebrow.
        let name_top = |out: &egui::FullOutput, name: &str| {
            let painted = texts(out);
            let eyebrow = painted.iter().find(|(t, _)| t == "COMPONENT").unwrap().1;
            painted
                .iter()
                .filter(|(t, r)| t == name && r.top() > eyebrow.bottom())
                .map(|(_, r)| r.top() - eyebrow.bottom())
                .fold(f32::INFINITY, f32::min)
        };
        let instance = name_top(&out, "Small");
        let mut f = fixture(&ctx);
        let main = name_top(&settled(&mut f.app, &ctx, f.m), "Button");
        assert!(
            (instance - main).abs() <= 0.5,
            "the instance's name {instance} under its eyebrow, a main's {main}"
        );
    }

    /// **Deleting a value variants use asks first, in a modal** that counts the
    /// variants and instances; `Escape` cancels and keeps the value; *Delete*
    /// deletes the value and its variant. Flip, run: the `using.is_empty()` arm
    /// taken for every value fails *"nothing deleted yet"*.
    #[test]
    fn deleting_a_used_value_asks_in_a_modal() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let values = |app: &OndinApp| {
            app.session.doc.get(v.set).unwrap().set().unwrap().props[0]
                .values
                .clone()
        };
        let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 2400.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                ui.set_max_width(280.0);
                app.inspector_ui(ui);
                app.value_delete_confirmation(ui.ctx());
            })
        };
        let out = settled(&mut v.app, &ctx, v.set);
        let chip = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Small")
            .filter(|(_, r)| r.top() > 100.0)
            .expect("the Small chip")
            .1
            .center();
        click(&mut v.app, &ctx, chip);
        let out = run(&mut v.app, Vec::new());
        let row = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Delete value")
            .expect("the menu's row")
            .1
            .center();
        click(&mut v.app, &ctx, row);
        assert_eq!(values(&v.app), ["Small", "Large"], "nothing deleted yet");
        assert!(v.app.modal_is_up(), "the modal is up");
        // A new area is sized on its first pass and drawn from its second.
        run(&mut v.app, Vec::new());
        let out = run(&mut v.app, Vec::new());
        let words: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(
            words
                .iter()
                .any(|t| t
                    == "1 variant uses “Small”, and is deleted with it. 1 instance will detach."),
            "{words:?}"
        );
        let key = |pressed| egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        run(&mut v.app, vec![key(true)]);
        run(&mut v.app, vec![key(false)]);
        assert!(!v.app.modal_is_up(), "Escape cancelled");
        assert_eq!(values(&v.app), ["Small", "Large"], "and kept the value");

        v.app.deleting_value = Some(crate::panels::ValueDelete {
            set: v.set,
            prop: 0,
            value: 0,
            name: "Small".into(),
            variants: 1,
            instances: 1,
        });
        run(&mut v.app, Vec::new());
        let out = run(&mut v.app, Vec::new());
        let delete = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Delete")
            .expect("the modal's Delete")
            .1
            .center();
        let press = |pressed| egui::Event::PointerButton {
            pos: delete,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(&mut v.app, vec![egui::Event::PointerMoved(delete)]);
        run(&mut v.app, vec![press(true)]);
        run(&mut v.app, vec![press(false)]);
        assert_eq!(values(&v.app), ["Large"], "the value is gone");
        assert!(v.app.session.doc.get(v.small).is_none(), "with its variant");
        assert!(!v.app.modal_is_up());
    }

    /// **A typed edit commits when a click on the canvas takes the selection
    /// away** (a text property, and the identity card's name), through
    /// `eframe::App::ui`. The canvas runs before the inspector, so the click that
    /// deselects has the inspector draw no field to report its blur. Flip, run:
    /// `inspector_panel`'s hold never applied (`held` always `None`) fails *"the
    /// property's text"* with `Go`.
    #[test]
    fn a_typed_edit_commits_when_a_canvas_click_takes_the_selection() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let mut wf = eframe::Frame::_new_kittest();
        let mut pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut wf))
        };
        let click = |app: &mut OndinApp,
                     at: egui::Pos2,
                     pass: &mut dyn FnMut(&mut OndinApp, Vec<egui::Event>) -> egui::FullOutput| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            pass(app, vec![egui::Event::PointerMoved(at)]);
            pass(app, vec![press(true)]);
            pass(app, vec![press(false)]);
        };
        let end = egui::Event::Key {
            key: egui::Key::End,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        // Empty canvas, left of the inspector and clear of the layers.
        let canvas = egui::pos2(600.0, 700.0);
        /// Where on the painted texts to click into the field.
        type Pick<'a> = &'a dyn Fn(&[(String, egui::Rect)]) -> egui::Pos2;
        let mut type_and_leave = |app: &mut OndinApp, pick: Pick<'_>| {
            app.session.selection.set_one(v.i);
            let mut out = pass(app, Vec::new());
            for _ in 0..3 {
                out = pass(app, Vec::new());
            }
            click(app, pick(&texts(&out)), &mut pass);
            pass(app, vec![end.clone()]);
            pass(app, vec![egui::Event::Text("XY".into())]);
            pass(app, Vec::new());
            click(app, canvas, &mut pass);
            for _ in 0..3 {
                pass(app, Vec::new());
            }
        };
        type_and_leave(&mut v.app, &|t| {
            t.iter()
                .find(|(s, _)| s == "Go")
                .expect("the field")
                .1
                .center()
        });
        assert_eq!(content(&v.app, v.ilabel), "GoXY", "the property's text");
        assert!(
            v.app.session.selection.ids().is_empty(),
            "the click deselected"
        );
        assert!(v.app.inspector_hold.is_none(), "and the hold let go");
        // The identity card's name: the topmost *Small* in the inspector's column.
        type_and_leave(&mut v.app, &|t| {
            t.iter()
                .filter(|(s, r)| s == "Small" && r.left() > 900.0)
                .min_by(|a, b| a.1.top().total_cmp(&b.1.top()))
                .expect("the name")
                .1
                .center()
        });
        assert_eq!(
            v.app.session.doc.get(v.i).unwrap().name(),
            "SmallXY",
            "the layer's name"
        );
    }

    /// `owner` selected and its Properties card's header `+` clicked: the text the
    /// next settled frame paints, the menu open in it.
    fn open_props_menu(
        app: &mut OndinApp,
        ctx: &egui::Context,
        owner: NodeId,
    ) -> Vec<(String, egui::Rect)> {
        let out = settled(app, ctx, owner);
        let painted = texts(&out);
        let head = painted
            .iter()
            .find(|(t, _)| t == "PROPERTIES")
            .unwrap_or_else(|| panic!("the Properties card: {painted:?}"))
            .1;
        let plus = painted
            .iter()
            .find(|(t, r)| {
                t == icon::PLUS
                    && (r.center().y - head.center().y).abs() < 4.0
                    && r.left() > head.right()
            })
            .expect("the header's +")
            .1
            .center();
        click(app, ctx, plus);
        texts(&settle(app, ctx))
    }

    /// The `+` menu's row labelled `label`, and the text on its line to its right
    /// (what it binds). **Only what the menu paints**, the tail of the frame,
    /// found from its first row's accelerator (`menu_row` paints that before its
    /// label): the cards below the menu have glyphs on the same lines.
    fn menu_row_named(painted: &[(String, egui::Rect)], label: &str) -> (egui::Rect, Vec<String>) {
        let start = painted
            .iter()
            .rposition(|(t, _)| t == "Boolean")
            .unwrap_or_else(|| panic!("the menu's first row: {painted:?}"))
            - 1;
        let painted = &painted[start..];
        let row = painted
            .iter()
            .find(|(t, _)| t == label)
            .unwrap_or_else(|| panic!("the menu's {label}: {painted:?}"))
            .1;
        let named = painted
            .iter()
            .filter(|(_, r)| (r.center().y - row.center().y).abs() < 4.0 && r.left() > row.right())
            .map(|(t, _)| t.clone())
            .collect();
        (row, named)
    }

    /// **Boolean, Text and Variant come from the header's `+` menu.** On a lone
    /// main the card is collapsed while it has no property (`sync_paint_collapse`);
    /// *Boolean* makes an unbound boolean property named in full and opens the
    /// card; on a set there is no *Variant*. Flips, run: the *Variant* row's
    /// `lone_main` guard dropped fails *"no Variant on a set"*; the
    /// `sync_paint_collapse` call removed fails *"collapsed while empty"*; the
    /// add's `open_paint_panel` removed fails *"opened by the add"*.
    #[test]
    fn the_properties_card_adds_from_its_header_menu() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut n = nested_fixture(&ctx);
        let painted = open_props_menu(&mut n.app, &ctx, n.button);
        assert!(
            n.app.collapsed_panels.contains("Properties"),
            "collapsed while empty"
        );
        assert_eq!(menu_row_named(&painted, "Boolean").1, ["visibility"]);
        assert_eq!(menu_row_named(&painted, "Text").1, ["text content"]);
        assert_eq!(menu_row_named(&painted, "Variant").1, ["makes a set"]);
        click(
            &mut n.app,
            &ctx,
            menu_row_named(&painted, "Boolean").0.center(),
        );
        let props = n.app.session.doc.get(n.button).unwrap().props().to_vec();
        assert_eq!(props.len(), 1, "one property made");
        assert_eq!(props[0].kind, ondin_core::variant::PropKind::Boolean);
        assert_eq!(props[0].name, "Boolean");
        assert!(props[0].bound.is_empty(), "bound to nothing");
        settle(&mut n.app, &ctx);
        assert!(
            !n.app.collapsed_panels.contains("Properties"),
            "opened by the add"
        );
        // Combined into a set, the card is the set's, and a set is no lone main.
        n.app.session.selection.set_one(n.button);
        n.app.combine_as_variants();
        let set = ondin_core::variant::owner_of_main(&n.app.session.doc, n.button).unwrap();
        assert_ne!(set, n.button, "a set was made");
        let painted = open_props_menu(&mut n.app, &ctx, set);
        assert!(
            painted.iter().any(|(t, _)| t == "Boolean"),
            "the set's menu: {painted:?}"
        );
        assert!(
            !painted.iter().any(|(t, _)| t == "Variant"),
            "no Variant on a set"
        );
    }

    /// **The instance count is on the name's line and selects them**, on a main,
    /// a set (every variant's instances; its variant count is the title, *VARIANTS
    /// 2*) and a variant. Flips, run: `count_link` answering `false` fails *"the
    /// main's count selects its instance"*; the `Face::Variant` arm's
    /// `act = Some(Act::SelectAllInstances)` made a no-op fails *"the variant's
    /// count selects its instances"*.
    #[test]
    fn the_count_after_a_faces_name_selects_its_instances() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let on_one_line = |painted: &[(String, egui::Rect)], name: &str, eyebrow: &str| {
            let top = painted
                .iter()
                .find(|(t, _)| t == eyebrow)
                .unwrap()
                .1
                .bottom();
            let name = painted
                .iter()
                .filter(|(t, r)| t == name && r.top() > top)
                .min_by(|a, b| a.1.top().total_cmp(&b.1.top()))
                .unwrap()
                .1;
            let count = painted.iter().find(|(t, _)| t == "1 instance").unwrap().1;
            // Tops, not centres: the count's ink runs down to its underline.
            assert!(
                (count.top() - name.top()).abs() <= 1.0 && count.left() > name.right(),
                "the count after the name, on its line: {count:?} against {name:?}"
            );
            assert!(
                !painted.iter().any(|(t, _)| t == "Select all"),
                "no Select all"
            );
            count.center()
        };
        let mut f = fixture(&ctx);
        let painted = texts(&settled(&mut f.app, &ctx, f.m));
        let count = on_one_line(&painted, "Button", "COMPONENT");
        click(&mut f.app, &ctx, count);
        assert_eq!(
            f.app.session.selection.ids(),
            &[f.i],
            "the main's count selects its instance"
        );
        let mut v = variants_fixture(&ctx);
        let painted = texts(&settled(&mut v.app, &ctx, v.set));
        let count = on_one_line(&painted, "Button", "VARIANTS 2");
        click(&mut v.app, &ctx, count);
        assert_eq!(
            v.app.session.selection.ids(),
            &[v.i],
            "the set's count selects every variant's instances"
        );
        // The variant's face: its derived name, then the count.
        let painted = texts(&settled(&mut v.app, &ctx, v.small));
        let count = on_one_line(&painted, "Small", "COMPONENT");
        click(&mut v.app, &ctx, count);
        assert_eq!(
            v.app.session.selection.ids(),
            &[v.i],
            "the variant's count selects its instances"
        );
    }

    /// **The Variants card rules off one property from the next**: one `card_rule`
    /// hairline between *Size* and *Style*, none above *Size*. Flip, run: the rule
    /// drawn above every property (`pi > 0` dropped) fails *"none above the first"*.
    #[test]
    fn the_variants_card_rules_between_its_properties() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let tx = ondin_core::variant::add_property(&v.app.session.doc, v.set, "Style", "Primary")
            .expect("a second property");
        assert!(v.app.session.commit(tx));
        let out = settled(&mut v.app, &ctx, v.set);
        let painted = texts(&out);
        let at = |s: &str| {
            painted
                .iter()
                .find(|(t, _)| t == s)
                .unwrap_or_else(|| panic!("{s} painted: {painted:?}"))
                .1
        };
        let rule = crate::theme::color::text_a(26);
        let mut rules = Vec::new();
        fn walk(s: &egui::Shape, rule: egui::Color32, o: &mut Vec<f32>) {
            match s {
                egui::Shape::LineSegment { points, stroke } if stroke.color == rule => {
                    o.push(points[0].y)
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, rule, o)),
                _ => {}
            }
        }
        for s in &out.shapes {
            walk(&s.shape, rule, &mut rules);
        }
        let (size, style) = (at("Size"), at("Style"));
        // The name line's count, the last thing over the first property.
        let counts = at("1 instance");
        let between = |a: f32, b: f32| rules.iter().filter(|y| **y > a && **y < b).count();
        assert_eq!(
            between(size.bottom(), style.top()),
            1,
            "one rule between Size and Style: {rules:?}"
        );
        assert_eq!(
            between(counts.bottom(), size.top()),
            0,
            "none above the first: {rules:?}"
        );
    }

    /// **A label cut to its slot says so, and one that fits does not.** A tooltip
    /// never reaches `FullOutput::shapes`, so the answer it hangs from is what is
    /// asserted. Flip, run: `cut` back to `galley.text() != text` fails *"a long
    /// name is cut"*.
    #[test]
    fn a_label_cut_to_its_slot_names_itself_and_one_that_fits_does_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let _ = ctx.run_ui(Default::default(), |ui| {
            let w = 76.0 - MARK_ROOM;
            let long = "Initials kana for the avatar";
            let (galley, cut) = fitted_label(ui, long, 12.0, egui::Color32::WHITE, w);
            assert_eq!(galley.text(), long, "the job's text, whole");
            assert!(galley.size().x <= w + 0.5, "laid out inside the slot");
            assert!(cut, "a long name is cut");
            let (_, cut) = fitted_label(ui, "Size", 12.0, egui::Color32::WHITE, w);
            assert!(!cut, "a name that fits is not");
        });
    }

    /// **A property bound from a layer inside the main opens the card when the main
    /// is selected again.** The card is not drawn for the Badge, so its edge has to
    /// move on a selection it skips, or the Button reads as the same subject and
    /// the card stays shut. Flip, run: `subject_changed("Properties")` asked below
    /// the early returns fails *"open over the property bound from inside"*.
    #[test]
    fn a_property_bound_from_inside_the_main_opens_its_card() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut n = nested_fixture(&ctx);
        settled(&mut n.app, &ctx, n.button);
        assert!(
            n.app.collapsed_panels.contains("Properties"),
            "collapsed while empty"
        );
        settled(&mut n.app, &ctx, n.slot);
        let (tx, _) = ondin_core::variant::define(
            &n.app.session.doc,
            &mut n.app.session.ids,
            n.button,
            "Badge",
            ondin_core::variant::PropKind::Swap,
            vec![n.slot],
        )
        .expect("a swap property");
        assert!(n.app.session.commit(tx));
        settle(&mut n.app, &ctx);
        settled(&mut n.app, &ctx, n.button);
        assert!(
            !n.app.collapsed_panels.contains("Properties"),
            "open over the property bound from inside"
        );
    }

    /// **The `+` menu hangs from the `+` as the ⋯ menu hangs from the ⋯, and is
    /// wide enough for its rows**: its frame starts `MENU_GAP` under the box the
    /// `+` paints (not its padded hit target), ends at its right edge, and each
    /// row's label and what it binds keep `ADD_MENU_GAP`, within 2pt. Flips, run:
    /// `action_rect` back to the padded rect fails *"MENU_GAP under the + box"*; a
    /// fixed 200 width fails *"the label and what it binds apart"*.
    #[test]
    fn the_properties_menu_hangs_from_its_plus_and_fits_its_rows() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut n = nested_fixture(&ctx);
        // No nested instance to offer, so *Instance swap* reads *nested instance*.
        let (tx, _) = ondin_core::variant::define(
            &n.app.session.doc,
            &mut n.app.session.ids,
            n.button,
            "Badge",
            ondin_core::variant::PropKind::Swap,
            vec![n.slot],
        )
        .expect("a swap property");
        assert!(n.app.session.commit(tx));
        open_props_menu(&mut n.app, &ctx, n.button);
        let out = settle(&mut n.app, &ctx);
        let painted = texts(&out);
        let at = |s: &str| {
            painted
                .iter()
                .find(|(t, _)| t == s)
                .unwrap_or_else(|| panic!("{s} painted: {painted:?}"))
                .1
        };
        let head = at("PROPERTIES");
        let plus = painted
            .iter()
            .find(|(t, r)| {
                t == icon::PLUS
                    && (r.center().y - head.center().y).abs() < 4.0
                    && r.left() > head.right()
            })
            .expect("the header's +")
            .1
            .center();
        let rects: Vec<egui::Rect> = fills(&out).into_iter().map(|(r, _)| r).collect();
        // The box the `+` paints: its hover ground, the pointer still on it.
        let plus_box = *rects
            .iter()
            .find(|r| r.contains(plus) && r.size() == egui::Vec2::splat(ui::HEAD_ACTION_W))
            .expect("the + box's hover ground");
        // The smallest rect holding the first row and the last (the cards under
        // the menu hold them too); the frame, not its shadow, which is offset down.
        let (first, last) = (at("Boolean").center(), at("Variant").center());
        let frame = *rects
            .iter()
            .filter(|r| r.contains(first) && r.contains(last))
            .min_by(|a, b| {
                a.area()
                    .total_cmp(&b.area())
                    .then(a.top().total_cmp(&b.top()))
            })
            .expect("the menu's frame");
        assert_eq!(
            frame.top() - plus_box.bottom(),
            ui::MENU_GAP,
            "MENU_GAP under the + box: {frame:?} against {plus_box:?}"
        );
        assert!(
            (frame.right() - plus_box.right()).abs() <= 1.0,
            "its right edge at the box's: {frame:?} against {plus_box:?}"
        );
        let gaps: Vec<f32> = [
            ("Boolean", "visibility"),
            ("Text", "text content"),
            ("Instance swap", "nested instance"),
            ("Variant", "makes a set"),
        ]
        .into_iter()
        .map(|(label, what)| at(what).left() - at(label).right())
        .collect();
        assert!(
            gaps.iter()
                .all(|g| *g >= super::super::variants::ADD_MENU_GAP - 2.0),
            "the label and what it binds apart: {gaps:?}"
        );
    }

    /// **A nested instance no swap property drives is offered as one, from the
    /// header's `+` menu, and a swap property's filter is a row of its own**: the
    /// *Instance swap* row names the Badge and makes the property, after which the
    /// card draws its funnel row and the menu names the Badge no more; showing the
    /// Badge's properties brings the *Shown from nested* heading. Flip, run: the
    /// menu's dormant list keeping nothing fails *"the Badge is named on Instance
    /// swap"*.
    #[test]
    fn a_nested_instance_is_offered_as_a_swap_property_and_its_filter_is_a_row() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut n = nested_fixture(&ctx);
        let words =
            |out: &egui::FullOutput| texts(out).into_iter().map(|(t, _)| t).collect::<Vec<_>>();
        // The menu's *Instance swap* row, and what it names on its line.
        let open_menu = |n: &mut N| {
            let painted = open_props_menu(&mut n.app, &ctx, n.button);
            let (row, named) = menu_row_named(&painted, "Instance swap");
            (row.center(), named)
        };
        let out = settled(&mut n.app, &ctx, n.button);
        assert!(
            !words(&out).iter().any(|t| t == "SHOWN FROM NESTED"),
            "no heading while nothing is shown"
        );
        let (row, named) = open_menu(&mut n);
        assert_eq!(named, ["Badge"], "the Badge is named on Instance swap");
        click(&mut n.app, &ctx, row);
        let props = n.app.session.doc.get(n.button).unwrap().props().to_vec();
        assert_eq!(props.len(), 1, "one property made");
        assert_eq!(props[0].kind, ondin_core::variant::PropKind::Swap);
        assert_eq!(props[0].bound, vec![n.slot]);
        let out = settled(&mut n.app, &ctx, n.button);
        let w = words(&out);
        assert!(
            w.iter().any(|t| t == icon::FUNNEL),
            "the filter's funnel: {w:?}"
        );
        assert!(
            w.iter().any(|t| t == "Filter displayed list by name"),
            "{w:?}"
        );
        assert!(
            !w.iter().any(|t| t == "Instance swap"),
            "the menu closed on its row"
        );
        let (_, named) = open_menu(&mut n);
        assert_eq!(named, ["nested instance"], "offered no more");
        let tx = ondin_core::variant::set_shown(
            &n.app.session.doc,
            &mut n.app.session.ids,
            n.slot,
            true,
        )
        .expect("a showing");
        assert!(n.app.session.commit(tx));
        let out = settled(&mut n.app, &ctx, n.button);
        assert!(
            words(&out).iter().any(|t| t == "SHOWN FROM NESTED"),
            "the heading once something is shown"
        );
    }

    /// **The set's card, the clash and the templates**: the set's name on one line,
    /// at a main's height, no caption; *Variant clash* for a repeated combination;
    /// no Frame templates card on any component. Flip, run: the templates'
    /// component guard removed fails *"not on a set"*. The caption's flip was not
    /// run; *"no caption"* asserts its absence directly.
    #[test]
    fn the_set_card_is_one_line_the_clash_is_short_and_components_get_no_templates() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let under = |out: &egui::FullOutput, eyebrow: &str, name: &str| {
            let painted = texts(out);
            let top = painted
                .iter()
                .find(|(t, _)| t == eyebrow)
                .unwrap()
                .1
                .bottom();
            painted
                .iter()
                .filter(|(t, r)| t == name && r.top() > top)
                .map(|(_, r)| r.top() - top)
                .fold(f32::INFINITY, f32::min)
        };
        let out = settled(&mut v.app, &ctx, v.set);
        let words: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(
            !words.iter().any(|t| t == "Component set"),
            "no caption: {words:?}"
        );
        assert!(
            !words.iter().any(|t| t == "FRAME TEMPLATES"),
            "not on a set"
        );
        // The title carries the set's variant count.
        let set = under(&out, "VARIANTS 2", "Button");
        let mut f = fixture(&ctx);
        let main = under(&settled(&mut f.app, &ctx, f.m), "COMPONENT", "Button");
        assert!(
            (set - main).abs() <= 0.5,
            "the set's name {set}, a main's {main}"
        );

        assert!(
            v.app
                .session
                .commit(Transaction(vec![Operation::SetVariant {
                    id: v.large,
                    values: vec!["Small".into()],
                }]))
        );
        for id in [v.small, v.i] {
            let out = settled(&mut v.app, &ctx, id);
            let words: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
            assert!(
                !words.iter().any(|t| t == "FRAME TEMPLATES"),
                "not on a component: {words:?}"
            );
            if id == v.small {
                assert!(words.iter().any(|t| t == "Variant clash"), "{words:?}");
                assert!(!words.iter().any(|t| t.starts_with("Another variant")));
            }
        }
    }

    // ── The variants panel's writes ────────────────────────────────────────────

    /// A second instance of Small beside `v.i`, its label set to `text`.
    fn second_instance(v: &mut V, text: &str) -> NodeId {
        let doc = &v.app.session.doc;
        let (tx, made) = ondin_core::insert_subtrees(
            doc,
            &mut v.app.session.ids,
            &[Placement {
                nodes: doc.capture_subtree(v.small).unwrap(),
                parent: doc.first_page(),
                index: None,
            }],
            Default::default(),
        );
        assert!(v.app.session.commit(tx));
        let j = made[0];
        let p = ondin_core::variant::instance_properties(&v.app.session.doc, j)
            .into_iter()
            .find(|p| p.value.name == "Label text")
            .unwrap()
            .value;
        let tx = ondin_core::variant::set_property(
            &v.app.session.doc,
            &[j],
            &p,
            &ondin_core::variant::PropValue::Text(text.into()),
        );
        assert!(v.app.session.commit(Transaction(tx)));
        j
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// **Typing over a mixed Text property writes what was typed.** *Mixed* is the
    /// hint, and the field starts empty. Flip, run: seeding the buffer from a
    /// literal "Mixed" fails both instances' text, `MixedOK`.
    #[test]
    fn typing_over_a_mixed_text_property_writes_what_was_typed() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let j = second_instance(&mut v, "Stop");
        let jlabel = v.app.session.doc.get(j).unwrap().children()[0];
        v.app.session.selection.set(vec![v.i, j]);
        let out = settle(&mut v.app, &ctx);
        let at = texts(&out)
            .into_iter()
            .find(|(t, _)| t == ui::MIXED_WORD)
            .expect("the hint")
            .1
            .center();
        click(&mut v.app, &ctx, at);
        frame(&mut v.app, &ctx, vec![key(egui::Key::End)]);
        frame(&mut v.app, &ctx, vec![egui::Event::Text("OK".into())]);
        frame(&mut v.app, &ctx, vec![key(egui::Key::Enter)]);
        assert_eq!(content(&v.app, v.ilabel), "OK");
        assert_eq!(content(&v.app, jlabel), "OK");
    }

    /// **A Text property's content commits as typed**: trailing spaces kept, and an
    /// emptied field empties the label. Flip, run: content under name rules fails
    /// "spaces kept", `Go!`.
    #[test]
    fn a_text_propertys_content_keeps_its_spaces_and_may_be_emptied() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let field = |app: &mut OndinApp| {
            app.session.selection.set_one(v.i);
            let out = settle(app, &ctx);
            texts(&out)
                .into_iter()
                .find(|(t, _)| t.starts_with("Go"))
                .expect("the field")
                .1
                .center()
        };
        let at = field(&mut v.app);
        click(&mut v.app, &ctx, at);
        frame(&mut v.app, &ctx, vec![key(egui::Key::End)]);
        frame(&mut v.app, &ctx, vec![egui::Event::Text("!  ".into())]);
        frame(&mut v.app, &ctx, vec![key(egui::Key::Enter)]);
        assert_eq!(content(&v.app, v.ilabel), "Go!  ", "spaces kept");
        let at = field(&mut v.app);
        click(&mut v.app, &ctx, at);
        frame(&mut v.app, &ctx, vec![key(egui::Key::End)]);
        for _ in 0..5 {
            frame(&mut v.app, &ctx, vec![key(egui::Key::Backspace)]);
        }
        frame(&mut v.app, &ctx, vec![key(egui::Key::Enter)]);
        assert_eq!(content(&v.app, v.ilabel), "", "emptied");
    }

    /// **A *Mixed* boolean property turns on**: one click on the toggle over two
    /// instances that differ sets both on, rather than flipping each. Flip, run:
    /// the arm's `if mixed { true } else { !on }` made `!on` fails "both on".
    #[test]
    fn a_mixed_boolean_property_turns_on() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        with_a_toggle(&mut v);
        let j = second_instance(&mut v, "Go");
        let p = ondin_core::variant::instance_properties(&v.app.session.doc, j)
            .into_iter()
            .find(|p| p.value.name == "Interesante")
            .unwrap()
            .value;
        let off = ondin_core::variant::set_property(
            &v.app.session.doc,
            &[j],
            &p,
            &ondin_core::variant::PropValue::Boolean(false),
        );
        assert!(v.app.session.commit(Transaction(off)));
        v.app.session.selection.set(vec![v.i, j]);
        let out = settle(&mut v.app, &ctx);
        let label = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Interesante")
            .expect("the toggle's row")
            .1;
        // The switch sits right of its label, at the field column.
        let at = egui::pos2(label.right() + 40.0, label.center().y);
        click(&mut v.app, &ctx, at);
        let on = |id| {
            matches!(
                ondin_core::variant::property_state(&v.app.session.doc, id, &p),
                Some((ondin_core::variant::PropValue::Boolean(true), _))
            )
        };
        assert!(on(v.i) && on(j), "both on");
    }

    /// The set card's chip of `value`, after the card has settled.
    fn chip(app: &mut OndinApp, ctx: &egui::Context, set: NodeId, value: &str) -> egui::Pos2 {
        let out = settled(app, ctx, set);
        texts(&out)
            .into_iter()
            .filter(|(t, r)| t == value && r.top() > 100.0)
            .min_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .expect("the chip")
            .1
            .center()
    }

    fn values(app: &OndinApp, set: NodeId) -> Vec<String> {
        app.session.doc.get(set).unwrap().set().unwrap().props[0]
            .values
            .clone()
    }

    /// **A value chip's menu closes on a move** (it is keyed by place, so it would
    /// belong to the neighbour), and a rename to a taken value is refused, saying
    /// so. Flip, run: dropping the `ui.close()` beside *Move later* fails "the menu
    /// closed".
    #[test]
    fn a_value_chips_menu_closes_on_a_move_and_refuses_a_taken_name() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let at = chip(&mut v.app, &ctx, v.set, "Small");
        click(&mut v.app, &ctx, at);
        let out = frame(&mut v.app, &ctx, Vec::new());
        let row = texts(&out)
            .into_iter()
            .find(|(t, _)| t == "Move later")
            .expect("the menu's row")
            .1
            .center();
        click(&mut v.app, &ctx, row);
        assert_eq!(values(&v.app, v.set), ["Large", "Small"]);
        let out = settle(&mut v.app, &ctx);
        assert!(
            !texts(&out).iter().any(|(t, _)| t == "Move later"),
            "the menu closed"
        );
        // Rename Small to Large through its menu.
        let at = chip(&mut v.app, &ctx, v.set, "Small");
        click(&mut v.app, &ctx, at);
        let out = frame(&mut v.app, &ctx, Vec::new());
        let field = texts(&out)
            .into_iter()
            .filter(|(t, _)| t == "Small")
            .max_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .expect("the menu's name field")
            .1
            .center();
        click(&mut v.app, &ctx, field);
        frame(&mut v.app, &ctx, vec![key(egui::Key::End)]);
        for _ in 0..5 {
            frame(&mut v.app, &ctx, vec![key(egui::Key::Backspace)]);
        }
        frame(&mut v.app, &ctx, vec![egui::Event::Text("Large".into())]);
        frame(&mut v.app, &ctx, vec![key(egui::Key::Enter)]);
        assert_eq!(values(&v.app, v.set), ["Large", "Small"], "unchanged");
        assert_eq!(v.app.session.status().text, "That value is taken");
    }

    /// **A value renamed in its chip's menu and left by a canvas click commits**:
    /// the menu's field asks the inspector to hold, as in
    /// `a_typed_edit_commits_when_a_canvas_click_takes_the_selection`. Flip, run:
    /// no `hold_inspector` fails "renamed". ⚠️ **Dropping the closed menu's
    /// `pending` commit stays green**: held, the field commits by its blur;
    /// `pending` is the second defence, for a menu closed before its field is drawn.
    #[test]
    fn a_chip_rename_commits_when_a_canvas_click_closes_the_menu() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut v = variants_fixture(&ctx);
        let mut wf = eframe::Frame::_new_kittest();
        let mut pass = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut wf))
        };
        fn click(
            app: &mut OndinApp,
            at: egui::Pos2,
            pass: &mut dyn FnMut(&mut OndinApp, Vec<egui::Event>) -> egui::FullOutput,
        ) -> egui::FullOutput {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            pass(app, vec![egui::Event::PointerMoved(at)]);
            pass(app, vec![press(true)]);
            pass(app, vec![press(false)])
        }
        fn settle(
            app: &mut OndinApp,
            pass: &mut dyn FnMut(&mut OndinApp, Vec<egui::Event>) -> egui::FullOutput,
        ) -> egui::FullOutput {
            let mut out = pass(app, Vec::new());
            for _ in 0..3 {
                out = pass(app, Vec::new());
            }
            out
        }
        v.app.session.selection.set_one(v.set);
        let out = settle(&mut v.app, &mut pass);
        let at = texts(&out)
            .into_iter()
            .filter(|(t, r)| t == "Small" && r.left() > 900.0)
            .max_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .expect("the chip")
            .1
            .center();
        click(&mut v.app, at, &mut pass);
        let out = settle(&mut v.app, &mut pass);
        assert!(
            texts(&out).iter().any(|(t, _)| t == "Move later"),
            "the fixture: the menu is open"
        );
        let field = texts(&out)
            .into_iter()
            .filter(|(t, _)| t == "Small")
            .max_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .expect("the menu's name field")
            .1
            .center();
        click(&mut v.app, field, &mut pass);
        pass(&mut v.app, vec![key(egui::Key::End)]);
        pass(&mut v.app, vec![egui::Event::Text("XY".into())]);
        pass(&mut v.app, Vec::new());
        click(&mut v.app, egui::pos2(600.0, 700.0), &mut pass);
        settle(&mut v.app, &mut pass);
        assert_eq!(values(&v.app, v.set), ["SmallXY", "Large"], "renamed");
        assert!(
            v.app.session.selection.ids().is_empty(),
            "the click deselected"
        );
        assert!(v.app.inspector_hold.is_none(), "and the hold let go");
    }

    // --- the card's cost and the roots it reads ------------------------------

    /// This thread's `Reads`, zeroed.
    fn reads_taken() -> Reads {
        READS.with(|r| r.replace(Reads::default()))
    }

    /// **An idle frame computes no property reading, and a filling one computes
    /// one drift per root** (not per half of the card). Counted, never timed.
    /// Flips, run: `prop_drift_of` computing every time fails *"one per root"*
    /// with 4; the rows' states read uncached fails *"an idle frame reads
    /// nothing"* with `state: 4`.
    #[test]
    fn an_idle_frame_computes_no_property_reading() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        let j = second_instance(&mut v, "Sign up");
        v.app.session.selection.set(vec![v.i, j]);
        reads_taken();
        let out = frame(&mut v.app, &ctx, Vec::new());
        assert!(
            texts(&out).iter().any(|(t, _)| t == "Label text"),
            "the fixture: the shared property row is drawn"
        );
        let first = reads_taken();
        assert_eq!(first.drift, 2, "one per root: {first:?}");
        frame(&mut v.app, &ctx, Vec::new());
        frame(&mut v.app, &ctx, Vec::new());
        assert_eq!(
            reads_taken(),
            Reads::default(),
            "an idle frame reads nothing"
        );
    }

    /// **A property row builds its reset only when it is overridden**: an instance
    /// following its main draws the *Label text* row and builds no reset for it;
    /// overridden, it builds one, once. Flip, run: the reset built unconditionally
    /// and uncached fails *"nothing to reset"* with `reset: 4`.
    #[test]
    fn a_property_rows_reset_is_built_only_when_it_is_overridden() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        v.app.session.selection.set_one(v.i);
        reads_taken();
        settle(&mut v.app, &ctx);
        assert_eq!(reads_taken().reset, 0, "nothing to reset");
        let j = second_instance(&mut v, "Sign up");
        v.app.session.selection.set_one(j);
        reads_taken();
        settle(&mut v.app, &ctx);
        assert_eq!(
            reads_taken().reset,
            1,
            "built once, then read from the cache"
        );
    }

    /// **The property cache follows the document**, as the drift cache does: the
    /// instance's *Label text* set through its property moves the summary to
    /// *1 property* and the row's field to *Sign up* on the next frame. Flip,
    /// run: `PropCache::at` emptying only when it was never filled fails *"the
    /// count follows"* with the stale `props: 0`.
    #[test]
    fn the_property_cache_follows_an_edit() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        v.app.session.selection.set_one(v.i);
        settle(&mut v.app, &ctx);
        assert_eq!(v.app.prop_drift_of(v.i).props, 0);
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
        assert_eq!(v.app.prop_drift_of(v.i).props, 1, "the count follows");
        let out = settle(&mut v.app, &ctx);
        let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(painted.contains(&"1 property".to_string()), "{painted:?}");
        assert!(painted.contains(&"Sign up".to_string()), "{painted:?}");
    }

    /// **The one-instance card reads the root it was drawn for, whatever else the
    /// selection holds**, in either order of the instance and its label. Flip, run:
    /// the rows given `selection.ids()` fails *"label first"*.
    #[test]
    fn the_one_instance_card_reads_its_root_and_not_the_selection() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        for (order, ids) in [
            ("label first", vec![v.ilabel, v.i]),
            ("instance first", vec![v.i, v.ilabel]),
        ] {
            v.app.session.selection.set(ids);
            assert!(
                matches!(v.app.component_face(), Some(Face::Instance { root, .. }) if root == v.i),
                "the fixture: one instance's face"
            );
            let out = settle(&mut v.app, &ctx);
            let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
            assert!(
                painted.contains(&"Size".to_string())
                    && painted.contains(&"Label text".to_string()),
                "{order}: {painted:?}"
            );
            assert!(
                !painted.iter().any(|t| t.contains("Only an instance")),
                "{order}: not refused the switch"
            );
        }
    }

    /// **The card's *Reset fields* resets what its count counted, with a nested
    /// copy selected inside its instance** (the Button instance and the Badge copy,
    /// whose *Count* is a field of the Button). Flip, run: `bound` read from every
    /// selected layer's nearest instance fails *"the count's reset is made"*.
    #[test]
    fn reset_fields_resets_what_it_counted_with_a_nested_copy_selected() {
        let ctx = egui::Context::default();
        let mut n = nested_fixture(&ctx);
        let count = n.app.session.doc.get(n.r).unwrap().children()[0];
        assert!(n.app.session.commit(Transaction(vec![Operation::SetText {
            id: count,
            content: "3".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }])));
        n.app.session.selection.set(vec![n.b1, n.r]);
        assert!(
            matches!(n.app.component_face(), Some(Face::Instance { root, .. }) if root == n.b1),
            "the fixture: the Button's face"
        );
        n.app.component_act(Some(Act::Reset(Kind::Fields)));
        assert_eq!(content(&n.app, count), "1", "the count's reset is made");
    }

    /// **Several instances, reset as one.** Two instances of *Button*, each dimmed
    /// to 0.5: the Appearance header counts two and its reset puts both back; and
    /// the opacity field's own mark writes both. Flip, run:
    /// `gather_card_overrides` reading the first selected layer alone fails
    /// *"both counted"* with 1.
    #[test]
    fn a_reset_over_two_instances_resets_both() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let second = {
            let doc = &f.app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut f.app.session.ids,
                &[Placement {
                    nodes: doc.capture_subtree(f.m).unwrap(),
                    parent: doc.first_page(),
                    index: None,
                }],
                Default::default(),
            );
            assert!(f.app.session.commit(tx));
            made[0]
        };
        let dim = |id| Operation::SetOpacity { id, opacity: 0.5 };
        assert!(
            f.app
                .session
                .commit(Transaction(vec![dim(f.i), dim(second)]))
        );
        let opacity = |app: &OndinApp, id| app.session.doc.get(id).unwrap().opacity();
        f.app.session.selection.set(vec![f.i, second]);
        f.app.gather_card_overrides();
        let (n, ops) = f.app.card_override("Appearance").expect("a header count");
        assert_eq!(n, 2, "both counted");
        let header = Transaction(ops.to_vec());
        let field = f
            .app
            .sub_mark(
                &[f.i, second],
                |op| match op {
                    Operation::SetOpacity { opacity, .. } => Some(*opacity),
                    _ => None,
                },
                |n| Some(n.opacity()),
                |o| *o,
                |o, v| *o = v,
                |id, opacity| Operation::SetOpacity { id, opacity },
                |v| v.to_string(),
            )
            .expect("the opacity field is marked");
        assert_eq!(field.tx.0.len(), 2, "the field's reset writes both");
        assert!(f.app.session.commit(header));
        assert_eq!(
            (opacity(&f.app, f.i), opacity(&f.app, second)),
            (1.0, 1.0),
            "both back"
        );
    }

    /// **Several instances' faces**: variants of one set read as instances of the
    /// set, with its rows; unrelated mains as *Instances of 2 components*. Flip,
    /// run: `shared` never `Some` fails the face's own `shared: Some(_)` match, and
    /// `a_mixed_boolean_property_turns_on` and
    /// `typing_over_a_mixed_text_property_writes_what_was_typed` beside it.
    #[test]
    fn several_instances_are_named_for_their_set_or_counted() {
        let ctx = egui::Context::default();
        let mut v = variants_fixture(&ctx);
        let k = {
            let doc = &v.app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut v.app.session.ids,
                &[Placement {
                    nodes: doc.capture_subtree(v.large).unwrap(),
                    parent: doc.first_page(),
                    index: None,
                }],
                Default::default(),
            );
            assert!(v.app.session.commit(tx));
            made[0]
        };
        v.app.session.selection.set(vec![v.i, k]);
        assert!(matches!(
            v.app.component_face(),
            Some(Face::Instances { count: 2, main: Some((m, _)), shared: Some(_), .. }) if m == v.set
        ));
        let out = settle(&mut v.app, &ctx);
        let painted: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(
            painted.contains(&"2 instances of".to_string())
                && painted.contains(&"Button".to_string()),
            "named for the set: {painted:?}"
        );
        assert!(
            painted.contains(&"Label text".to_string()),
            "the set's rows: {painted:?}"
        );

        let mut f = fixture(&ctx);
        let card = f.app.session.ids.mint();
        let root = f.app.session.doc.first_page();
        assert!(f.app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: card,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(50.0, 50.0),
                },
                transform: None,
                name: Some("Card".into()),
            },
            Operation::SetComponent {
                id: card,
                component: true,
            },
        ])));
        let other = {
            let doc = &f.app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut f.app.session.ids,
                &[Placement {
                    nodes: doc.capture_subtree(card).unwrap(),
                    parent: doc.first_page(),
                    index: None,
                }],
                Default::default(),
            );
            assert!(f.app.session.commit(tx));
            made[0]
        };
        f.app.session.selection.set(vec![f.i, other]);
        let out = settle(&mut f.app, &ctx);
        assert!(
            texts(&out)
                .iter()
                .any(|(t, _)| t == "Instances of 2 components"),
            "no single main"
        );
    }
}
