//! The **Component** card — what the selection is to the components machinery
//! (`architecture.md` §5.3d), drawn from §15 D981's accepted chrome
//! (`design/Components.dc.html`, sections 3A–3H): a main's instance count and its
//! two verbs, an instance's link to its main, its drift and the reset family, and
//! the summaries several selected instances get. A layer *inside* an instance gets
//! one line instead of a card — D981's *"one line, not a card"* — drawn as a row of
//! the identity card since §15 D995 (`ComponentPart::Identity`), as are a mixed
//! selection's line and the binding line.
//!
//! **Directly under the identity card** (3H), because it says what the layer *is*.
//!
//! **Counts tell you what each reset will do; a reset with nothing to do is
//! disabled, not hidden** (D981). The counts are `reset::Drift`, cached per
//! session revision (`OndinApp::drift_cache`), since the card is read every frame
//! and a drift is a walk of the instance — and the property readings beside them
//! on the same key ([`PropCache`], `OndinApp::prop_cache`, `[X8.1-L4-02]`).
//!
//! ⚠️ **The drift summary counts every difference a *Reset all* would undo** —
//! fields, removed children and order — where the mockup's *3 overrides* sat over a
//! *Reset fields 3* and a *Restore removed children 1*. Read literally that hides a
//! removed child from the summary, and an instance whose only drift is a deleted
//! layer then reads as untouched; the session's choice, accepted in the blanket
//! ruling of 2026-10-06 (§15's header).
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
    /// One instance root — `root`, which the face was decided on: the card's
    /// rows read it rather than the raw selection, which may hold a layer inside
    /// it too, in either order (`[X8.1-L1-03]`).
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
    #[cfg(test)]
    count(|r| r.drift += 1);
    let defined = !variant::instance_properties(doc, root).is_empty();
    // The instance's own properties, then the shown rows of the nested instances
    // its card shows (§15 D988, 4N: *"it counts as a property"*).
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
                    PropKind::Boolean => matches!(o.reset, Operation::SetVisible { .. }),
                    PropKind::Text => matches!(o.reset, Operation::SetText { .. }),
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
/// counted so a test can say an idle frame computes none of them
/// (`[X8.1-L4-02]`, `[X10-L4-01]`). A count and not a clock, the shape
/// `svg::def_count_tests` took when a timing assertion would not hold.
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

/// The Component card's **property readings**, as of the session revision they
/// were read at — the twin of `OndinApp::drift_cache`, and for the same reason:
/// the card is drawn every frame it is up and each reading is a walk of an
/// instance (`[X8.1-L4-02]`, `[X10-L4-01]`).
///
/// Uncached, a selection of many instances paid for every one of them every
/// frame, idle or not — `prop_drift` twice, since the card draws in two halves
/// (§15 D995) and each starts from `OndinApp::component_face`, and every
/// property's `variant::property_state` and `variant::reset_property` per root
/// in `OndinApp::own_rows`: measured at 200 instances, a 33–49 ms inspector
/// frame. Keyed on the revision, so an edit, undo or redo empties it — a preview
/// never writes the document, so nothing else can make a reading stale.
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

/// Which half of the components inspector a call draws (§15 D995): the rows that
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
    /// is unchanged (`[X8.1-L4-02]`).
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
        // The append is `placement_stated`'s, which the context menu's and the
        // Component card's resets take too (`[X4-L1-01]`).
        if let Some((_, _, ops)) = cards.iter_mut().find(|(t, ..)| *t == "Transform") {
            *ops = placement_stated(doc, Transaction(std::mem::take(ops))).0;
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

    /// `tx`, a reset, with every placed copy's insets stated beside it — the
    /// module's `placement_stated`, which says why. The context menu's and the
    /// Component card's resets take it (`OndinApp::reset_tx`, `[X4-L1-01]`), as
    /// the Transform header's does.
    pub(crate) fn placement_stated(&self, tx: Transaction) -> Transaction {
        placement_stated(&self.session.doc, tx)
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

    /// The Component card (§15 D981) — see the module doc.
    pub(super) fn inspector_component(&mut self, ui: &mut egui::Ui, part: ComponentPart) {
        let Some(face) = self.component_face() else {
            return;
        };
        let mut act = None;
        if part == ComponentPart::Identity {
            // **Inside the identity card, under its second row** (§15 D995): the
            // one-line faces and the binding line were cards of their own with no
            // title — the maintainer's look found them orphaned, and each is a
            // fact about what the layer *is*, which is that card's subject.
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
            // property is made (§15 D983 (iii)) and which that face had never
            // drawn (§15 D989).
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
                        root,
                        main,
                        main_name,
                        drift,
                        props,
                    } => {
                        let summary = drift_summary(*drift, *props);
                        let link = Some((*main, main_name.as_str()));
                        // One line, the main's name alone: the mockup's *Instance
                        // of* caption over it is gone, the outline hexagon saying
                        // it (§15 D993, as the main's face).
                        heading(ui, false, "", link, None, &summary, &mut act);
                        // The root the face was decided on, not the selection: a
                        // layer of the instance selected first drew no rows, and
                        // selected after it was asked to switch (`[X8.1-L1-03]`).
                        app.chosen_by_note(ui, *root);
                        app.instance_rows(ui, &[*root]);
                        app.show_switch(ui, *root);
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
        self.component_act(act);
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
                // Shown rows included (§15 D988, 4N).
                let ops: Vec<Operation> = roots
                    .iter()
                    .flat_map(|r| ondin_core::variant::reset_all_properties(doc, *r))
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
                    // The roots `reset_tx` scoped — the outermost selected, as the
                    // face and its count were read — not every selected layer's
                    // nearest instance: a nested copy selected inside its outer
                    // instance put its own properties' fields here, filtered out
                    // the very reset the row counted, and the click did nothing
                    // (`[X8.1-L1-03]`).
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
                        // leaves alone: said, as `reset_tx` says its own nothing,
                        // rather than a click that silently does nothing.
                        self.session.info("Only component properties differ here");
                    } else {
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
/// drawn dashed and dim with a restore button (§15 D981, 4D) — the list half of
/// the reset family. `label` is the item's own words (a hex, *Linear*, *Drop
/// shadow*). Answers whether the restore button was clicked.
///
/// **Laid out as the live row is** (§15 D993, the maintainer's look): the dashed
/// box is the field's box, its corners rounded 3px — the maintainer's number,
/// where the live field's own are 5 (`ui::field_frame`) — and its chip or
/// glyph and label stand where the live row's do; *restore* is an icon-only
/// button in the column the live row's eye takes, rather than a worded button
/// inside the field. A row with no eye column ([`GhostLead::Plain`]) still gets
/// one, so every list's restore is in the same place.
///
/// ⚠️ **Lists only.** A deleted child *layer* gets no ghost row in the layers
/// panel — D981's asymmetry, ruled: only the card's *Restore removed children*.
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
/// copy — a `SetTransform`, or a `SetGeometry` that resizes — and has no
/// `SetInsets` of its own in `tx`: the insets of its **slot source**
/// (`reset::slot_source_of`), or its own where its placement is its own
/// (`reset::placement_is_own`, an instance root).
///
/// **Both of the commit door's tool-intent rewrites read a bare placement as the
/// hand's**, and a reset is not the hand. `keep_insets` reads a transform on a
/// pinned layer as *"draw it here"* and re-pins it where it lands — in an
/// instance wider than its main, the main's stored point is another inset
/// (`c292b50`'s defect, measured as `right 172` against `12`); and
/// `kept_flow_translations` keeps an in-flow layout item's stored translation and
/// drops the write as changing nothing, so an item's *Reset all* committed
/// nothing at all. A `SetInsets` for the node stands both down. Where `tx` writes
/// no insets the node's equal its source's — `reset::overrides` would have reset
/// them otherwise — so this states the value the node already has, except on the
/// Transform header's reset, which takes the card's ops alone and so resets the
/// insets with it (counted under Position).
///
/// ⚠️ **The slot, not `source_of`, for a swapped copy** (§15 D983 (3)): its
/// placement is its link's, and the swap target's insets unpinned the slot
/// (`[X9.1-L1-01]`). ⚠️ **One helper for every reset door** (`[X4-L1-01]`):
/// `c292b50` wrote this append in `OndinApp::gather_card_overrides` alone, and
/// the context menu's and the Component card's resets (`OndinApp::reset_tx`) went
/// on sending the bare transform. A field's own reset (`OndinApp::transform_marks`)
/// states its insets per axis, and is left to them.
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
/// (§15 D996): centred, a short label sat far from its field and a long one beside
/// it, so *Size* read as further left than *Body text* in the column below it.
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
    let galley = match size.filter(|_| right) {
        Some(slot) => {
            let mut job = egui::text::LayoutJob::single_section(
                text.to_owned(),
                egui::TextFormat::simple(egui::FontId::proportional(pt), color),
            );
            job.wrap = egui::text::TextWrapping {
                max_width: (slot.x - MARK_ROOM).max(0.0),
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            ui.painter().layout_job(job)
        }
        None => ui
            .painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(pt), color),
    };
    let cut = galley.text() != text;
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
    // A cut label names itself in the tooltip too, over the mark's reset — or a
    // long overridden name could not be read whole anywhere (`arch-scribe`'s
    // find, §15 D996).
    match cut {
        true => resp.on_hover_text(format!("{text}\n{}", m.tip)),
        false => resp.on_hover_text(&m.tip),
    }
    .clicked()
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

/// The face of an instance card — or a variant's, with `main` — the hexagon
/// (outline for an instance, filled for a `main`, `paint_hexagon`), an optional
/// small caption over the main's name as a link (or over a plain `title`), and
/// the grey drift summary on the right. One line when `caption` is empty — a
/// single instance's face since §15 D993; several instances keep their count as
/// the caption, and a variant its set.
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
        // **The name is cut to the room the row leaves it** (`[X8.1-L1-01]`): a
        // plain label in a horizontal row never wraps, and a main's name past
        // ~50 characters ran the row — and egui grows the parent to its widest
        // row, so every card below — off the screen, *Detach* half drawn and the
        // summary not at all. A variant's derived name (`values.join(", ")`) gets
        // long with three properties. Cut with `…`, the whole name in the
        // tooltip, as the card's own label column already was (§15 D996).
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
                // The name, then the ↗ — one target, so a click on either goes.
                // **As tall as the name, not as a control** (§15 D996): a
                // horizontal row opens at `interact_size.y`, 24, and centred the
                // name in it — so an instance's name and hexagon sat 4pt below a
                // main's, whose name is a plain label in its block. The
                // maintainer's look. Nothing in this block is a control.
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
                .gap(ui::MENU_GAP)
                .show(|ui| {
                    ui::menu_rows(ui);
                    // **As wide as its longest row, not as the screen** (§15 D993):
                    // `menu_row` takes the available width, which in a popup's
                    // `Area` is however far the screen goes, so the menu ran most
                    // of the way across the canvas. The rows have no glyph column
                    // (`menu_row`'s bare form) and a count of at most a few digits.
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
                    let widest = std::iter::once(PROPS)
                        .chain(rows.iter().map(|(_, l, _)| *l))
                        .map(label_w)
                        .fold(0.0, f32::max);
                    ui.set_width((8.0 + widest + 24.0 + 16.0 + 8.0).ceil());
                    // A count only where there is something to reset: the `—` a
                    // zero row carried read as a stray mark, and the dimmed row
                    // already says there is nothing (§15 D993).
                    let count_of = |n: usize| (n > 0).then(|| n.to_string());
                    // 4C: properties and fields count, and reset, separately.
                    // Present wherever the component has properties, and
                    // disabled at zero like its neighbours (D981) — `arch-scribe`
                    // read the first cut hiding it.
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
                });
        }
    });
}

/// 3A: the filled hexagon and the name on **one line**, the instance count with
/// *Select all*, and *Duplicate*. The mockup's *Main component* caption over the
/// name is gone (§15 D993): the filled hexagon already says it.
fn main_body(ui: &mut egui::Ui, name: &str, instances: usize, act: &mut Option<Act>) {
    name_line(ui, None, name);
    // The count, *Select all* and *Duplicate* 4pt closer under the one-line face,
    // the gap an instance's face leaves above its *Reset all* (§15 D995, the
    // maintainer's look). Here and not in `main_tail`, which a variant's face
    // shares under its property rows.
    ui.add_space(-4.0);
    main_tail(ui, instances, act);
}

/// A card's one-line face: the glyph in its slot and the name beside it — a
/// main's, `None` for the filled hexagon, and a set's on the Variants card (§15
/// D1000), one layout so the two names sit at one height.
pub(super) fn name_line(ui: &mut egui::Ui, glyph: Option<&str>, name: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        let slot = glyph_slot(ui);
        let block = ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(name)
                    .size(13.0)
                    .color(theme::text::STRONG),
            );
        });
        match glyph {
            None => paint_hexagon(ui, true, slot, block.response.rect),
            Some(g) => paint_glyph(ui, g, slot, block.response.rect),
        }
    });
}

/// A main's count with *Select all*, and *Duplicate* — 3A's lower half, which a
/// variant's face shares (3D). The mockup's *Duplicate as component*, shortened on
/// the card that already says what it is (§15 D993); the context menu keeps the
/// long name, where nothing around it does.
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

/// 3D: *In Button instance · Go to main*, or *Local to this instance · no
/// counterpart* — a row of the identity card since §15 D995.
///
/// A local layer leads with the **broken link**, where the mockup had `+`: a `+`
/// at the head of a row reads as a button that adds something, and this one is a
/// statement that the layer has no counterpart (§15 D995, the maintainer's look).
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
        // **Cut to what *Go to main* leaves** (`[X8.1-L1-01]`): measured, a
        // 16-character main name already ran under the button and a 53-character
        // one pushed it — and every card below, egui growing the parent to the
        // row — off the screen. The whole line in the tooltip when cut (the
        // label's own, `show_tooltip_when_elided`).
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
    // The hexagon a point above the row's centre, where it sat low against the
    // words beside it (§15 D995, the maintainer's look).
    let lift = if linked { 1.0 } else { 0.0 };
    paint_glyph(
        ui,
        glyph,
        slot,
        row.response.rect.translate(egui::vec2(0.0, -lift)),
    );
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
            // **Off its main** (`[X8.2-L6-01]`): placed over the main, world and
            // local readings agree for every node here, and a Transform mark
            // compared on the card's world numbers — the one implementation
            // `OndinApp::transform_marks`' doc forbids — passed every test.
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

    /// The restore button of the ghost row labelled `label` — the ↺ on the
    /// label's line, to its right (§15 D993: an icon in the eye's column).
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
    /// main's name on one line — no *Instance of* caption since §15 D993 — and
    /// *1 override*; a press and release on *Reset all* puts the name back and the
    /// summary goes. Driven through `RawInput`, so the button's `Response` is the
    /// one the card reads.
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
        // The hexagon is centred on the name's line (`glyph_slot`, `paint_glyph`),
        // the block being the name alone now.
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
        // hangs 3.5pt below the baseline and drags the box's centre down with it
        // (measured 2.25 off where the cap band is 0.3). Inter's cap height is
        // 0.727em, the name 13pt.
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
    ///
    /// ⚠️ **The world-reading flip bites only because `fixture` places the
    /// instance off its main** (`[X8.2-L6-01]`). Over its main, world and local
    /// agreed for every node, and X/Y compared as `world[axis]` against the main's
    /// `world_transform` left the whole suite green. Flip, run, with the instance
    /// at (200, 40): that comparison fails *"Y follows"* here and *"an unmarked
    /// label still scrubs"* in `a_marked_fields_reset_shows_an_arrow_and_a_tooltip`.
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

    /// **A marked field's ↺ reads as a button, not as a scrub handle** (§15 D995):
    /// with the pointer resting on X's label slot the cursor is the plain arrow
    /// (§9.2's rule, §15 D371), where an unmarked label shows the scrub's
    /// horizontal arrows, and a tooltip
    /// opens naming the reset. Flips, both run: the strip's cursor left at the
    /// arrows fails *"the marked label's cursor"*; dropping the strip's
    /// `on_hover_text` fails *"a tooltip is open"* — which says the tooltip was
    /// there before this entry (*Reset to main ·* the main's value, after egui's
    /// 0.5s delay), and only the cursor was wrong.
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

    /// **The Appearance card's marks** (§15 D981): the instance's rect has its
    /// opacity and one corner's radius overridden. Opacity's drop glyph is bright
    /// and the radius field marked — the whole radius differs — while each corner
    /// is marked alone: the top left and no other, its reset writing that corner
    /// and leaving the other three as the copy holds them. A click on opacity's
    /// glyph puts the main's 100% back.
    ///
    /// ⚠️ **The corner is held by two guards in `sub_mark`, and either alone keeps
    /// it unmarked**: the equality skip (`get(&cur) == want`), and *"a reset that
    /// would write nothing marks nothing"* (`cur == before`), since for an
    /// untouched corner `put` writes the value it already holds. Flips, run
    /// (`[X8.2-L6-05]`, re-run 2026-10-07): the equality skip dropped leaves this
    /// test green, and is caught where `put` is lossy — by
    /// `layout::tests::an_instance_marks_its_own_layout_fields_and_no_others`
    /// (*"the mode follows"*), `typography::instance_mark_tests::the_sizing_mark_compares_the_mode_and_not_the_width`
    /// (*"the width is W's"*) and, since its copy carries a vertical pin of its
    /// own, `a_centre_button_marks_a_centring_the_main_does_not_share` (*"one
    /// dot"*, two). The write-nothing guard dropped alone fails only the layout
    /// test (*"no layout, no gap to reset"*). Both dropped fail here at *"the top
    /// right follows"*.
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

    /// The centre of every override dot the frame painted — the tests' one
    /// detector, `ui::painted_override_dots` (`[X8.2-L3-01]`).
    fn dots(out: &egui::FullOutput) -> Vec<egui::Pos2> {
        ui::painted_override_dots(out)
    }

    /// The inspector drawn, then three more frames for it to settle — a widget's
    /// state is last frame's (CLAUDE.md, the egui measuring traps). The module's
    /// one copy (`[X8.2-L3-01]`: it was written out twenty-five times).
    fn settle(app: &mut OndinApp, ctx: &egui::Context) -> egui::FullOutput {
        let mut out = frame(app, ctx, Vec::new());
        for _ in 0..3 {
            out = frame(app, ctx, Vec::new());
        }
        out
    }

    /// Press and release the primary button at `at`, the pointer moved there
    /// first — the precondition a click here has (§15 D986), in one place
    /// (`[X8.2-L3-01]`: it was written out nine times, twice as named helpers).
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

    /// **The Position card's centre buttons carry a mark** (§15 D981 (e), closed
    /// 2026-10-06). The main's rect is centred horizontally, between 30 and 60;
    /// the instance's copy is pinned left at 5 alone. The horizontal button draws
    /// the dot at its own top right, between the two glyphs, and the vertical
    /// button none, both centred nowhere. A click on the marked button is its
    /// reset: the main's whole horizontal axis comes back — both insets and the
    /// auto margins — and the vertical axis is not written. Flip: `centred`
    /// reading the margins of the wrong axis (top and bottom) leaves the
    /// horizontal button unmarked and fails *"one dot"* with none. Flip, run:
    /// `centre_mark`'s `put` copying the main's whole insets (`*i = from`) fails
    /// *"the vertical axis untouched"* — the copy's own `top 20` gone — which it
    /// passed while the copy's vertical axis was the main's (`[X8.2-L6-06]`).
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
        // And pinned top 20 of its own, against the main's unset top: the
        // vertical axis has to *differ* for "untouched" to say anything — with
        // both `None`, a reset writing the main's whole insets left the same pair
        // (`[X8.2-L6-06]`).
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
        // The last of each: the align row above the Position card draws the same
        // two glyphs.
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

    /// **The shared radius over several layers carries a mark** (§15 D981 (e),
    /// closed 2026-10-06). The instance and a plain rect are selected; the copy's
    /// radius is 6 where its main's is 0, the plain rect's 6 with no main. The
    /// field's glyph is bright — any rect in the selection's subtree that overrides
    /// it marks it — and a click on the glyph writes the main's radius into the
    /// copy alone, the plain rect keeping its 6. Flip: subjects taken as the
    /// selection rather than its subtree reach only the instance's frame, which
    /// has no radius, and fail *"the radius is overridden"* with the glyph dim.
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

    /// **A list reads item by item against the main's** (§15 D981, 4D). The main's
    /// rect has two fills; the instance recolours the first, removes the second and
    /// adds one of its own. The removed one draws as a ghost row; the own one shows
    /// `+`; the recoloured one's slot, under the pointer, is ↺, whose click takes
    /// that item's colour back and nothing else; and the ghost row's restore puts
    /// the whole list back as the main has it, the own fill gone (§15 D994).
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
        let out = settle(&mut f.app, &ctx);
        let painted = texts(&out);
        let gone_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 2, 30));
        assert!(painted.iter().any(|(t, _)| *t == gone_hex), "a ghost row");
        let own_hex = crate::ui::hex_of(ondin_core::peniko::Color::from_rgb8(10, 3, 30));
        let rect_of = |s: &str| painted.iter().find(|(t, _)| t == s).map(|(_, r)| *r);
        // **The own fill's `+` is on its own row** (`[X8.2-L6-08]`): this counted
        // `+`s at two or more, which the Fill and Stroke headers' own `+`s already
        // satisfy with no mark on the row at all. Flip, run: `item_slot`'s `Local`
        // arm painting `""` for `icon::PLUS` fails *"a + on the own fill's row"*,
        // and left the count green.
        let own_row = rect_of(&own_hex).expect("the own fill's row");
        assert!(
            painted.iter().any(|(t, r)| {
                t == icon::PLUS
                    && (r.center().y - own_row.center().y).abs() < 4.0
                    && r.left() > own_row.right()
            }),
            "a + on the own fill's row: {painted:?}"
        );
        // *Restore* on the ghost row.
        let click = |app: &mut OndinApp, at: egui::Pos2| click(app, &ctx, at);
        // **The ghost row is laid out as the live one** (§15 D993): its hex
        // starts where a live row's hex does, and its restore stands in the eye's
        // column. Flips, both run: the label at the old lead (a 12pt chip at the
        // field's inset) fails *"the hex's x"*; the restore back inside the field
        // fails *"the eye's column"*.
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
        // ↺ on the recoloured row first: that item alone, the ghost and the own
        // fill left as they are.
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
        // own fill gone with the rest (§15 D994). Flip, run: restoring the one
        // item (`reset_item`, the rule before) fails *"the main's list"*.
        let mut out = frame(&mut f.app, &ctx, Vec::new());
        for _ in 0..2 {
            out = frame(&mut f.app, &ctx, Vec::new());
        }
        let restore =
            ghost_restore(&texts(&out), &gone_hex).expect("the ghost row's restore, still there");
        click(&mut f.app, restore);
        assert_eq!(fills_of(&f.app, f.ir), main, "the main's list");
    }

    /// The Effects card reads its stack the same way (§15 D981, 4D–4E's *Inner
    /// shadow ↺*): the instance hides the main's first effect and removes the
    /// second; the first's ↺ under the pointer takes the main's back alone, and the
    /// second is a ghost row whose restore brings the whole stack back (§15 D994).
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
        // item would read as the whole stack and the test could not tell the two
        // rules apart (`arch-scribe`'s find).
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
        // The hidden effect's ↺ first: that item alone, the ghost row still there.
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
        // Then the ghost row's restore: the whole stack as the main has it (§15
        // D994).
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

    /// **The Stroke card reads its list as the Fill card does** (`[X8.2-L6-02]`;
    /// §15 D981, 4D; D994): the instance recolours the main's first stroke, removes
    /// the second and adds one of its own. The recoloured row's ↺ takes that item
    /// back alone; the ghost row's *Restore* puts the whole list back as the main
    /// has it, the own stroke gone with it.
    ///
    /// Flips, run: the restore arm handed `reset_item(src, strokes, gone)` (the
    /// rule before D994) fails *"the main's list"*; the ↺ arm writing nothing
    /// fails *"that item alone"*. Both were green before this test.
    #[test]
    fn a_stroke_list_restores_and_resets_item_by_item() {
        use ondin_core::{Keyed, Stroke, keyed_by_position};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = f.app.session.doc.get(f.m).unwrap().children()[0];
        let rgb = |g: u8| ondin_core::peniko::Color::from_rgb8(10, g, 30);
        let stroke = |g: u8| Stroke {
            brush: ondin_core::Brush::Solid(rgb(g)),
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

    /// **The Layout grid card, on an instance's root frame** (`[X8.2-L6-02]`; §15
    /// D981, 4D; D994): the instance changes the main's first grid's count, removes
    /// the second and adds one of its own. The changed grid's ↺ takes it back
    /// alone; the ghost row's *Restore* puts the main's whole list back.
    ///
    /// Flips, run: the card's `list` match answering `None` for a restore fails
    /// *"the main's list"*, and for a ↺ fails *"that item alone"*; the restore
    /// handed `reset_item` (the rule before D994) fails *"the main's list"*. All
    /// three were green before this test.
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

    /// **The Export card, on two items** (`[X8.2-L6-02]`, `[X9.2-L6-06]`; §15 D981,
    /// D994): the instance turns the main's 2× PNG into 3× SVG, removes its 1× SVG
    /// and adds one of its own. The changed row's ↺ takes the main's export back by
    /// its id and value; the ghost row's *Restore* is the main's whole list, the own
    /// export gone. `an_emptied_list_still_draws_its_ghost_rows` drives this
    /// restore on a list of one, where the whole list and the one item are the same
    /// write.
    ///
    /// Flips, run: the ↺ arm answering `None` fails *"that item alone"*; the
    /// restore handed `reset_item(src, &specs, …)` (the rule before D994) fails
    /// *"the main's list"*. Both were green before this test.
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

    /// **X's reset writes the horizontal insets and keeps the copy's own
    /// vertical ones** (`[X8.2-L6-06]`). `pinned_and_moved`'s copy keeps the
    /// main's `top 8`, so a reset copying the main's whole insets passed
    /// `an_x_reset_on_a_pinned_layer_leaves_no_drift` too; here the copy is pinned
    /// `top 20` of its own first, and X's ↺ must leave it — a vertical pin is an
    /// override of its own, not X's. Flip, run: `transform_marks`' X and Y resets
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
    /// gives the main's insets for the same reason (through `placement_stated`,
    /// since `[X4-L1-01]` the reset doors' one helper). Flip: dropping that append
    /// fails *"the insets came back"* with `right 172` — the main's point, pinned
    /// against the wider instance.
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
        // **And through W's own ↺** (`[X8.2-L6-07]`): `6210953` fixed the root's
        // insets at the header and in `transform_marks` both, and only the header
        // was driven. Widened again, as above, and W's bright label clicked. Flip,
        // run: `transform_marks`' `main_in` read from the source for a root too
        // fails *"W keeps the instance's own pin"* — `right: None`, the instance
        // unpinned, the shipped defect.
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

    /// **The menu's and the card's *Reset all* state the placement too**
    /// (`[X4-L1-01]`). The main's rect pinned `right 12` alone, stored at (78, 8)
    /// as the app's pin writes it; the instance 260 wide, so the copy draws at
    /// x 238; the copy moved straight down 20 from where it is drawn — its insets
    /// still the main's, its stored transform (238, 28), one override: the
    /// transform. `reset_tx` sent that `SetTransform` bare, and `keep_insets` read
    /// it as *"draw it here"*: measured, the copy came back `right 172` — 160 px
    /// left of where it was, with an insets override it never had. Through
    /// `reset_selection` (*Reset ‹Label›*, the menu's door) and then through the
    /// Component card's *Reset all* on the instance (`Act::Reset(Kind::All)`), the
    /// two doors `reset_tx` serves. Flip, run: `reset_tx` without
    /// `placement_stated` fails *"the menu's reset keeps the pin"* with
    /// `right: Some(Px(172.0))`.
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
        // The instance's own width is reset with it — the root's size is no
        // placement — so the copy, still `right 12`, draws where the main's does.
        assert_eq!(drawn_x(&f), 78.0, "pinned in the main's width");
        assert_eq!(ondin_core::reset::overrides(doc, f.ir), Vec::new());
    }

    /// **An in-flow flex item's *Reset all* lands** (`[X4-L1-01]`'s second
    /// case). The main laid out as a flex row; the copy pinned (`left 40, top
    /// 30`, out of the flow), placed there, then unpinned — back in the flow with
    /// the main's insets and a stored translation of (40, 30) where the main's
    /// rect stores (0, 0). One override, and *Reset all* offered for it; but
    /// `kept_flow_translations` keeps an in-flow item's stored translation and
    /// dropped the write as changing nothing, so the commit was empty — measured:
    /// nothing committed, the override and the enabled reset there for good.
    /// With the insets stated beside the transform the door leaves the write to
    /// the transaction. Flip, run: `reset_tx` without `placement_stated` fails
    /// *"the override is gone"*.
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
        let root = n.app.session.doc.root();
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

    /// **A swapped slot's Transform marks compare with the slot, not the swap**
    /// (`[X9.1-L1-01]`, §15 D983 (3)). The slot keeps its placement and size, so
    /// nothing in Transform differs and X, Y, W and H are drawn dim. Read from
    /// `reset::source_of` — the Tag — all four were marked, their tips naming the
    /// Tag's page position and size, and X's ↺ moved the slot out of its button.
    /// Then the copy is resized to 40 × 40, pin kept: *Reset transform* puts the
    /// slot's 30 × 30 back and keeps its `right 8` — with the Tag's insets stated
    /// beside it, the reset unpinned the slot, measured. Flips, run:
    /// `transform_marks` reading its source from `source_of` again fails *"X
    /// follows the slot"*; `placement_stated` reading `source_of` fails *"the
    /// slot's pin stays"* with `right: None`.
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

    /// **A text's box width is marked on W** (`[X9.1-L1-02]`, `[X9.2-L1-02]`).
    /// The main's text wraps at 100 and the copy's at 180, both auto-height: the
    /// Type header counted one override and no field showed it — the Sizing mark
    /// compares the mode, which agrees, and `transform_marks` had no text arm. W is
    /// bright now, H dim, and W's ↺ writes `AutoHeight(100)`, the copy's mode kept.
    /// Flip, run: the text arm's `AutoHeight` case removed fails *"W is
    /// overridden"*.
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

    /// **A line's length is marked on `L`, and its direction on R**
    /// (`[X9.1-L1-02]`). The main's line ends at (50, 0). The copy's at (30, 0):
    /// `L` is bright and its ↺ writes (50, 0). Then the copy's at (0, 50), the
    /// same length turned: `L` dim, R bright, and R's ↺ writes (50, 0). Before,
    /// the Transform header counted the `LineEnd` override and neither field was
    /// marked. Flips, run: `L` drawn through the unmarked `value_field` again
    /// fails *"L is overridden"*; the direction's mark removed fails *"R is
    /// overridden"*.
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
        // The rotation field's glyph — the first one in X's column; the quarter
        // turn buttons below draw arrows too.
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

    /// **Rotation's ↺ turns back about the pivot** (§15 D1003 (8),
    /// `[X9.1-L1-03]`). The copy's 10 × 10 rect turned 90° about its centre, as
    /// the field and the handle turn it — stored translation (10, 0), its box
    /// where it was. R's ↺ writes the main's basis with the centre held, so the
    /// rect is back exactly as the main has it, X and Y following. Keeping the
    /// copy's translation instead left it at (10, 0) — 10 px off its own place and
    /// the main's. Flip, run: R's reset built with `cur[4], cur[5]` fails *"back
    /// in place"*.
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
    /// it back** (4C: *"properties and fields count, and reset, separately"*;
    /// `[X8.2-L6-03]`). The label's text is set through *Label text* and its
    /// opacity overridden. The card's *Reset fields* puts the opacity back and
    /// keeps "Sign up"; *Reset properties* puts "Go" back. The filter is the app's
    /// alone — no core test reaches it. Flips, run: the filter made `true || …`
    /// (nothing kept out) fails *"the property's value stays"* with "Go";
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

    /// ***Reset fields* with only a property differing says so** rather than
    /// doing nothing silently (`1e77a22`): the filtered transaction is empty, so
    /// nothing commits and the status line reads *"Only component properties
    /// differ here"*. Flip, run: the `kept.is_empty()` arm removed (the empty
    /// transaction committed through `commit_reset`) fails *"says why"* with an
    /// empty status — *"nothing committed"* holding under it too, an empty
    /// commit changing nothing, so the words are the whole of what it adds.
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

    /// **A long main name is cut, and the card keeps its width** (`[X8.1-L1-01]`).
    /// Measured in the inspector's layout: with a child selected, *In … instance*
    /// ran under *Go to main* from ~16 characters, and at 53 pushed the button —
    /// and every card below, egui growing the parent to its widest row — off the
    /// screen; with the instance selected, the heading's name cut *Detach* in half
    /// and hid the summary. Here the main is renamed to 53 characters and each
    /// control is where it is with the short name: *Go to main*, the Transform
    /// header's title, *Detach* and the drift summary. Flips, run: the child
    /// line's label without `.truncate()` fails *"Go to main stays put"*; the
    /// heading's name without it fails *"Detach stays put"*.
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
        // Each mark's shapes: a dot is the override dot (the tests' one detector,
        // `ui::painted_override_dots` — this copy counted any 2pt circle,
        // `[X8.2-L3-01]`), a `+` two 1.2pt strokes.
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
        // The *Component set* caption went with §15 D1000; its absence is
        // `the_set_card_is_one_line_the_clash_is_short_and_components_get_no_templates`'.
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

    /// A **Badge** main with a text *Count* bound to a Text property, nested as
    /// `slot` inside a **Button** main, and `b1` an instance of Button whose copy
    /// of the slot is `r` (§15 D988's fixture).
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

    /// **The opt-in, from the nested instance's own card, and the main's list**
    /// (§15 D988, 4K, 4L). Selected inside the Button main, the Badge's card
    /// draws the binding line — which an instance root inside a main never drew
    /// before (§15 D989) — and *Show properties on instances*, captioned with
    /// what it would show; a click on it shows the slot. The Button main's
    /// Properties card then lists it under *Shown from nested*, with its *Count*.
    /// Flips, both run: the switch's call taken out of the instance face fails
    /// *"the switch is drawn"*; the binding line's condition made never true
    /// fails *"the binding line"*.
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

    /// **A shown nested instance's rows on the outer instance's card** (§15
    /// D988, 4M–4N): `b1`'s card draws the Badge's group — its name, the main it
    /// shows, and its *Count* row — and the copy's count set to 3 reads as one
    /// **property**, not an override. Flips, both run: `instance_rows` not
    /// drawing the groups fails *"the group's row"*; `prop_drift` without the
    /// shown rows fails *"1 property"* with **no summary at all** — not the
    /// *1 override* first predicted, since `property_fields` still subtracts the
    /// count's field from the overrides.
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
    /// small variant's label — the maintainer's own name for the row in the look
    /// that asked for this (§15 D996).
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

    /// The frame after a few settling passes with `id` selected.
    fn settled(app: &mut OndinApp, ctx: &egui::Context, id: NodeId) -> egui::FullOutput {
        app.session.selection.set_one(id);
        settle(app, ctx)
    }

    /// **The Component card's rows are the other cards' rows** (§15 D996, the
    /// maintainer's look: *"make things consistent in the inspector cards"*). On
    /// an instance with a variant dropdown, a text property and a boolean one:
    ///
    /// - every field and dropdown in the label column is `CONTROL_H` tall and ends
    ///   at the card's content edge — where the ⋯ button under them ends, as every
    ///   other card's last control does;
    /// - the labels *Size*, *Label text* and *Interesante* end at one x, right
    ///   against their controls;
    /// - the boolean's switch starts where the fields do;
    /// - and the instance's name sits at the height a main's does in its card.
    ///
    /// Measured before the change: dropdowns 24 tall ending 8 short of the edge,
    /// the labels centred (ending at 65, 81 and — the toggle's, at the content
    /// edge in 11.5pt — 76), the switch against the right margin, and the
    /// instance's name 4pt below the main's.
    ///
    /// **Flips run**, each alone: `control_height` not called fails *"28 tall"*
    /// at 24; the `- 8.0` back on the variant dropdown fails *"at the content
    /// edge"* at 257; `row_label` back on `label_mark` (centred) fails *"one right
    /// edge"*; and the heading's `interact_size.y = 0.0` removed fails the name's
    /// height at 4 off.
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
        // The ⋯ under the rows: the last control of the card, at its content edge.
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

    /// **Deleting a value variants use asks first, in a modal** (§15 D996, the
    /// maintainer's look — it was a sentence inside the chip's menu over a row
    /// that did it). With *Small* used by one variant that has one instance: the
    /// chip's *Delete value* row deletes nothing and raises the modal, which
    /// counts both; `Escape` cancels and keeps the value; a second ask answered
    /// *Delete* deletes the value and its variant.
    ///
    /// **Flip run**: the row deleting straight away again (the `using.is_empty()`
    /// arm taken for every value) fails *"nothing deleted yet"*, the predicted
    /// site.
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
    /// away** (§15 D999, the maintainer's report: *"Here I have to press enter to
    /// change the value. If I just blur it doesn't persist."*). The whole app,
    /// through `eframe::App::ui`: an instance's text property typed into, then a
    /// click on empty canvas, which deselects — the text is the property's; and
    /// the same for the layer's name in the identity card, which had the same
    /// fault. The selection the click made is the one left afterwards, so the
    /// hold lets go.
    ///
    /// Before: `Go` stayed `Go` and `Small` stayed `Small`. A click inside the
    /// inspector always committed — measured — because the field was still drawn;
    /// the canvas runs before the inspector, and the click that deselected had the
    /// inspector draw no field to report its blur.
    ///
    /// **Flip run**: `inspector_panel`'s hold never applied (`held` always
    /// `None`) fails *"the property's text"* with `Go`, the predicted site.
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

    /// **A nested instance no swap property drives is offered as one, and a swap
    /// property's filter is a row of its own** (§15 D1000, the maintainer's look:
    /// *"If I remove the icon property … I have no idea how to bring it back"*).
    /// A Button main holding a Badge instance and no properties: its Properties
    /// card offers the Badge with a `+`, and no *Shown from nested* heading while
    /// nothing is shown; the `+` makes a swap property bound to the Badge, after
    /// which the card draws its funnel row and the line under it, and offers the
    /// Badge no more; showing the Badge's properties brings the heading.
    ///
    /// **Flip run**: the offered rows' filter keeping nothing fails *"the Badge is
    /// offered"*, the predicted site.
    #[test]
    fn a_nested_instance_is_offered_as_a_swap_property_and_its_filter_is_a_row() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut n = nested_fixture(&ctx);
        let words =
            |out: &egui::FullOutput| texts(out).into_iter().map(|(t, _)| t).collect::<Vec<_>>();
        let out = settled(&mut n.app, &ctx, n.button);
        let painted = texts(&out);
        let row = painted
            .iter()
            .find(|(t, _)| t == "· nested instance")
            .unwrap_or_else(|| panic!("the Badge is offered: {:?}", words(&out)))
            .1;
        assert!(
            !words(&out).iter().any(|t| t == "SHOWN FROM NESTED"),
            "no heading while nothing is shown"
        );
        let plus = painted
            .iter()
            .find(|(t, r)| {
                t == icon::PLUS
                    && (r.center().y - row.center().y).abs() < 4.0
                    && r.left() > row.right()
            })
            .expect("the row's +")
            .1
            .center();
        click(&mut n.app, &ctx, plus);
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
            !w.iter().any(|t| t == "· nested instance"),
            "offered no more"
        );
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

    /// **The set's card, the clash and the templates** (§15 D1000, the
    /// maintainer's look). The Variants card's face is the set's name on one line
    /// — no *Component set* caption — at the height a main's name sits under its
    /// card's eyebrow; a variant whose combination another has says *Variant
    /// clash* and not the sentence that ran under *Select it*; and no component —
    /// a set, a variant, an instance — gets the Frame templates card.
    ///
    /// **Flip run**: the templates' component guard removed fails *"not on a
    /// set"* — at the set, the first selection asked, where the prediction had
    /// been the variant. The caption's flip was not run; *"no caption"* asserts
    /// its absence directly.
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
        let set = under(&out, "VARIANTS", "Button");
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

    // ── The variants panel's writes (`v0.4.1..7d0c666` release review) ─────────

    /// A second instance of Small beside `v.i`, its label set to `text`.
    fn second_instance(v: &mut V, text: &str) -> NodeId {
        let doc = &v.app.session.doc;
        let (tx, made) = ondin_core::insert_subtrees(
            doc,
            &mut v.app.session.ids,
            &[Placement {
                nodes: doc.capture_subtree(v.small).unwrap(),
                parent: doc.root(),
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

    /// **Typing over a mixed Text property writes what was typed** (`[X10-L1-01]`):
    /// the field's buffer was seeded with the word *Mixed*, so typing *OK* wrote
    /// "MixOKed" into every selected instance. *Mixed* is the hint now, and the
    /// field starts empty.
    ///
    /// Flip, run: seeding the buffer from a literal "Mixed" again fails both
    /// instances' text, `MixedOK`.
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

    /// **A Text property's content commits as typed** (§15 D1003 (10),
    /// `[X10-L1-04]`): trailing spaces kept, and an emptied field empties the
    /// label. Flip, run: content under name rules again fails "spaces kept", `Go!`.
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

    /// **A *Mixed* boolean property turns on** (`[X10-L6-01]`): one click on the
    /// toggle over two instances that differ sets both on, rather than flipping
    /// each. Flip, run: the arm's `if mixed { true } else { !on }` made `!on`
    /// fails "both on".
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

    /// **A value chip's menu closes on a move** (`[X10-L1-02]`): it is keyed by
    /// place, so left open after *Move later* it belonged to the neighbour — a
    /// second *Move later* moved that one back, and *Delete* deleted it. And a
    /// rename to a value the property has is refused, saying so (`[X10-L6-01]`).
    ///
    /// Flip, run: dropping the `ui.close()` beside *Move later* fails "the menu
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

    /// **A value renamed in its chip's menu and left by a canvas click commits**
    /// (§15 D1003 (11), `[X10-L1-03]`) — D999's promise for a popover field that
    /// commits. The click deselected the set, so the card — and the menu with it —
    /// was not drawn again and its field never saw its focus go; the menu's field
    /// now asks the inspector to hold. Through `eframe::App::ui`, as D999's own
    /// test.
    ///
    /// Flips, run: no `hold_inspector` fails "renamed". ⚠️ **Dropping the closed
    /// menu's `pending` commit stays green**: held, the field is drawn on the
    /// click's own frame and commits by its blur. `pending` is the second defence,
    /// for a menu that closes before its field is drawn.
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

    // --- the card's cost and the roots it reads (K17) ------------------------

    /// This thread's `Reads`, zeroed.
    fn reads_taken() -> Reads {
        READS.with(|r| r.replace(Reads::default()))
    }

    /// **An idle frame computes no property reading, and a filling one computes
    /// one drift per root** (`[X8.1-L4-02]`, `[X10-L4-01]`). Two instances of
    /// *Small*, one with its *Label text* overridden, both selected: the first
    /// frame reads `prop_drift` once per root — not once per root per half of the
    /// card (§15 D995) — and the next two frames, the document unchanged, read
    /// nothing at all: no drift, no `property_state`, no `reset_property`.
    /// Counted, never timed (the `svg::def_count_tests` shape). Flips, run:
    /// `prop_drift_of` computing every time fails *"one per root"* with 4; the
    /// rows' states read uncached fails *"an idle frame reads nothing"* with
    /// `state: 4`.
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

    /// **A property row builds its reset only when it is overridden**
    /// (`[X10-L4-01]`): an instance following its main draws the *Label text*
    /// row and builds no reset for it; overridden, it builds one, once. Flip,
    /// run: the reset built unconditionally and uncached, as it was, fails
    /// *"nothing to reset"* with `reset: 4` — one a frame over `settle`'s four.
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

    /// **The one-instance card reads the root it was drawn for, whatever else
    /// the selection holds** (`[X8.1-L1-03]`). The instance selected with its own
    /// label, the label first — Ctrl-clicking in the layers panel gives that
    /// order — draws the variant dropdown and the property row; the other order
    /// draws them too, and is not refused the switch. Flip, run: the rows given
    /// `selection.ids()` again fails *"label first"*.
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
    /// copy selected inside its instance** (`[X8.1-L1-03]`). The Badge copy's
    /// *Count* text is set to 3 — a field of the Button instance, since the
    /// Button does not show the Badge's properties. With the Button instance
    /// *and* the copy selected, the face is the Button's and *Reset fields*
    /// puts the 1 back. Flip, run: `bound` read from every selected layer's
    /// nearest instance again fails *"the count's reset is made"*, the click
    /// committing nothing.
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

    /// **Several instances, reset as one** (`[X8.2-L6-04]`, D981's *"a reset
    /// resets every one of them"*). Two instances of *Button*, each dimmed to 0.5:
    /// the Appearance header counts two and its reset puts both back; and the
    /// opacity field's own mark writes both. Flip, run:
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
                    parent: doc.root(),
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

    /// **Several instances' faces** (`[X8.2-L6-04]`): instances of two variants
    /// of one set read as instances of the set — named for it, with its rows —
    /// and instances of two unrelated mains read as *Instances of 2
    /// components*. Flip, run: `shared` never `Some` fails the face's own
    /// `shared: Some(_)` match, before *"the set's rows"* is reached — and fails
    /// `a_mixed_boolean_property_turns_on` and
    /// `typing_over_a_mixed_text_property_writes_what_was_typed` beside it, whose
    /// rows are drawn through `shared` too.
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
                    parent: doc.root(),
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
        let root = f.app.session.doc.root();
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
                    parent: doc.root(),
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
