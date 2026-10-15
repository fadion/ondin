//! Styles (§5.3f, §15 D1037, D1038, D1062).
//!
//! A **style** is one slot's whole set of values — an isolated CSS class, not the
//! cascade. Build step 3's slots are a layer's **fills**, its **strokes** and its
//! **effects**, each the whole list its inspector card edits; step 6 adds a text
//! layer's **type** (§15 D1067); a container's layout has no step. A node holds at
//! most one style per slot ([`StyleRefs`]), and styles do not stack.
//!
//! **A text style is the node's character defaults, not its spans** (§15 D1067,
//! D1037's open (e)). It holds a [`TextStyle`] less its colour and its language
//! ([`type_part`]): the colour is the Fill card's and its style's, and a language
//! belongs to the words. Spans stay the text's own marks over it — a bold word in
//! a *Body* paragraph stays bold when *Body* changes — so styling a selection
//! writes spans and never detaches, and only a write of the defaults does.
//!
//! ⚠️ **Linked copies, not a read-time resolve** (§15 D1062). A styled layer
//! keeps the style's values **in its own list**, written there by the commit
//! ([`settle`]) whenever the style or the binding changes — so every reader of
//! a layer's paint and effects (the scene walk, the bounds and their escape, the
//! SVG and PNG writers, the snapshot, the hit test) reads the list it always
//! read, and none of them learns that styles exist. That is §15 D978's argument
//! for instances, made again for the same reason: a resolve at read costs every
//! reader, and these lists have more readers than any bindable number. The
//! variables a style's values hold stay bound in every copy and resolve at read
//! as they always do, so a styled fill bound to *brand/500* still follows the
//! frame's mode.
//!
//! The commit's pass is the whole of the rule "a card is styled or not, as a
//! whole" (§5.3f): a layer whose slot no longer matches its style is either
//! **rewritten** — the style was edited, applied, or a component follow drifted
//! it — or **detached**, its values kept, when the edit's own operations wrote
//! that slot (D1038 (1): every door but the inspector detaches). So a canvas
//! gesture, *Paste properties*, MCP and every future door detach without knowing
//! this module exists.
//!
//! The table is the fourth piece of document state that is not a node, beside
//! the guides, the images and the variables, changed by operations that name a
//! style rather than a node; its rules are a post-condition of every
//! transaction ([`check`]).

use crate::document::Document;
use crate::effect::Effect;
use crate::id::NodeId;
use crate::item::{ItemId, Keyed, same_values};
use crate::node::{Fill, Stroke};
use crate::op::{Operation, Transaction};
use crate::typography::TextStyle;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// A style's identity: a [`NodeId`] under a newtype, minted from the session's
/// one [`crate::IdSource`] as a variable's is — so it never collides with a
/// node's, and `reserve_existing_ids` covers it without a second counter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct StyleId(pub NodeId);

impl std::fmt::Debug for StyleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "style {:?}", self.0)
    }
}

/// Written as the node id's wire form, `"<actor-hex>:<seq>"`.
impl Serialize for StyleId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_wire())
    }
}

impl<'de> Deserialize<'de> for StyleId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        NodeId::from_wire(&s)
            .map(StyleId)
            .ok_or_else(|| serde::de::Error::custom(format!("bad id {s:?}")))
    }
}

impl crate::IdSource {
    /// A fresh [`StyleId`].
    pub fn mint_style(&mut self) -> StyleId {
        StyleId(self.mint())
    }
}

/// Which of a layer's cards a style stands for. The order is the inspector's,
/// which is the browser's *Styles* tab's (D1038 G).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StyleSlot {
    Fills,
    Strokes,
    Effects,
    /// A text layer's type — its character defaults (§15 D1067). Last, as the
    /// Type card sits below the paint cards' slots in the browser's tab.
    Text,
}

impl StyleSlot {
    pub const ALL: [StyleSlot; 4] = [
        StyleSlot::Fills,
        StyleSlot::Strokes,
        StyleSlot::Effects,
        StyleSlot::Text,
    ];

    /// The card's title, as the inspector writes it.
    pub fn title(self) -> &'static str {
        match self {
            StyleSlot::Fills => "Fill",
            StyleSlot::Strokes => "Stroke",
            StyleSlot::Effects => "Effects",
            StyleSlot::Text => "Type",
        }
    }
}

/// `style` as a text style holds it (§15 D1067): its colour and its language
/// cleared. A run's colour is the Fill card's business and the fill style's, and
/// a language tag describes the words rather than the type, so neither travels
/// with *Body* onto a layer, and neither detaches it when changed.
pub fn type_part(style: &TextStyle) -> TextStyle {
    TextStyle {
        color: None,
        locale: None,
        ..style.clone()
    }
}

/// `typed`'s type over `own`'s colour and language — what a styled text layer's
/// character defaults become when the commit writes its style onto it.
fn typed_over(own: &TextStyle, typed: &TextStyle) -> TextStyle {
    TextStyle {
        color: own.color,
        locale: own.locale.clone(),
        ..typed.clone()
    }
}

/// A style's values: the whole list of one slot, its items keyed as a layer's
/// are (§15 D980). A copy written onto a layer carries these ids verbatim, which
/// is what lets an instance of a styled main follow it item by item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StyleValue {
    Fills(Vec<Keyed<Fill>>),
    Strokes(Vec<Keyed<Stroke>>),
    Effects(Vec<Keyed<Effect>>),
    /// A text style: character defaults, colour and language cleared
    /// ([`type_part`], §15 D1067). Boxed: a `TextStyle` is several times a list.
    Text(Box<TextStyle>),
}

impl StyleValue {
    pub fn slot(&self) -> StyleSlot {
        match self {
            StyleValue::Fills(_) => StyleSlot::Fills,
            StyleValue::Strokes(_) => StyleSlot::Strokes,
            StyleValue::Effects(_) => StyleSlot::Effects,
            StyleValue::Text(_) => StyleSlot::Text,
        }
    }

    /// `node`'s own list for `slot`, as a style's value. A layer that is not text
    /// answers the default type for [`StyleSlot::Text`] — [`check`] refuses
    /// binding one, so the answer is only ever compared, never written.
    pub fn of(node: &crate::Node, slot: StyleSlot) -> StyleValue {
        match slot {
            StyleSlot::Fills => StyleValue::Fills(node.paint.fills.clone()),
            StyleSlot::Strokes => StyleValue::Strokes(node.paint.strokes.clone()),
            StyleSlot::Effects => StyleValue::Effects(node.effects.clone()),
            StyleSlot::Text => StyleValue::Text(Box::new(match &node.kind {
                crate::NodeKind::Text { style, .. } => type_part(style),
                _ => type_part(&TextStyle::default()),
            })),
        }
    }

    /// Whether the two **look** the same — equal values in the same order, the
    /// items' ids not asked ([`same_values`]). A layer holding its style's look
    /// under other ids is still the style's. Two text styles are compared on
    /// their [`type_part`]s, so a colour a hand-written file left in one is not
    /// a difference.
    pub fn same_look(&self, other: &StyleValue) -> bool {
        match (self, other) {
            (StyleValue::Fills(a), StyleValue::Fills(b)) => same_values(a, b),
            (StyleValue::Strokes(a), StyleValue::Strokes(b)) => same_values(a, b),
            (StyleValue::Effects(a), StyleValue::Effects(b)) => same_values(a, b),
            (StyleValue::Text(a), StyleValue::Text(b)) => type_part(a) == type_part(b),
            _ => false,
        }
    }

    /// The operation writing this value onto `node`'s slot. A text style keeps
    /// the node's own colour and language ([`type_part`]) and re-states its spans
    /// against the new defaults (`spans: None`, `Operation::SetTextStyle`) — so a
    /// span the new defaults now equal is dropped as redundant, the price of the
    /// spans' one invariant (§15 D1067).
    pub fn write_op(&self, node: &crate::Node) -> Operation {
        let id = node.id;
        match self {
            StyleValue::Fills(fills) => Operation::SetFills {
                id,
                fills: fills.clone(),
            },
            StyleValue::Strokes(strokes) => Operation::SetStrokes {
                id,
                strokes: strokes.clone(),
            },
            StyleValue::Effects(effects) => Operation::SetEffects {
                id,
                effects: effects.clone(),
            },
            StyleValue::Text(typed) => {
                let own = match &node.kind {
                    crate::NodeKind::Text { style, .. } => style.as_ref().clone(),
                    _ => TextStyle::default(),
                };
                Operation::SetTextStyle {
                    id,
                    style: typed_over(&own, typed),
                    spans: None,
                }
            }
        }
    }

    /// How many items it holds — the summary's count; a text style is one.
    pub fn len(&self) -> usize {
        match self {
            StyleValue::Fills(v) => v.len(),
            StyleValue::Strokes(v) => v.len(),
            StyleValue::Effects(v) => v.len(),
            StyleValue::Text(_) => 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The variables its items are bound to — a paint's colour, a stroke's
    /// width (§15 D1073) — in order, repeats kept.
    pub fn vars(&self) -> Vec<crate::variable::VarId> {
        match self {
            StyleValue::Fills(v) => v.iter().filter_map(|f| f.brush.var()).collect(),
            StyleValue::Strokes(v) => v
                .iter()
                .flat_map(|s| s.brush.var().into_iter().chain(s.width_var))
                .collect(),
            StyleValue::Effects(_) | StyleValue::Text(_) => Vec::new(),
        }
    }

    fn first_duplicate(&self) -> Option<ItemId> {
        match self {
            StyleValue::Fills(v) => crate::item::first_duplicate(v),
            StyleValue::Strokes(v) => crate::item::first_duplicate(v),
            StyleValue::Effects(v) => crate::item::first_duplicate(v),
            StyleValue::Text(_) => None,
        }
    }
}

/// One style: a name, and one slot's values.
///
/// **The name is a path** — `Surface/Card` — the slashes the browser's groups,
/// as a variable's are (D1038 G). Names are unique **within a slot**: a fill
/// style and a stroke style may both be *Primary*, since the inspector and the
/// browser never show the two in one list, and a paste rebinds by name and slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub id: StyleId,
    pub name: String,
    pub value: StyleValue,
    /// Which kit it came from, for a style a vendored main is bound to (§15
    /// D1064) — `None` for the document's own. A kit's style is read-only here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<StyleOrigin>,
    /// The kit style this one of the document's own was forked from by *Make
    /// local* (§15 D1066), as `crate::variable::Variable::forked` marks a
    /// variable — `None` for every other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked: Option<StyleOrigin>,
}

impl Style {
    pub fn slot(&self) -> StyleSlot {
        self.value.slot()
    }
    /// The kit this style came from, `None` for the document's own.
    pub fn kit(&self) -> Option<&str> {
        self.origin.as_ref().map(|o| o.kit.as_str())
    }
}

/// Where a kit's style came from (§15 D1064) — `crate::variable::VarOrigin` for
/// a style: the kit's `meta.id`, the style in its published state, the version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleOrigin {
    pub kit: String,
    pub style: StyleId,
    pub version: u32,
}

/// The document's styles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Styles {
    pub(crate) styles: BTreeMap<StyleId, Style>,
}

impl Styles {
    pub fn get(&self, id: StyleId) -> Option<&Style> {
        self.styles.get(&id)
    }
    /// Every style, in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Style> {
        self.styles.values()
    }
    /// One slot's styles, in id order.
    pub fn of_slot(&self, slot: StyleSlot) -> impl Iterator<Item = &Style> {
        self.styles.values().filter(move |s| s.slot() == slot)
    }
    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }
    /// The document's own style of `slot` named `name` — a kit's are named in
    /// their kit, and a paste rebinds to one of those by its origin (§15 D1064).
    pub fn by_name(&self, slot: StyleSlot, name: &str) -> Option<&Style> {
        self.of_slot(slot)
            .find(|s| s.origin.is_none() && s.name == name)
    }
    /// The style whose origin is `key`, a kit's style held here.
    pub fn by_origin(&self, key: (&str, StyleId)) -> Option<&Style> {
        self.styles.values().find(|s| {
            s.origin
                .as_ref()
                .is_some_and(|o| (o.kit.as_str(), o.style) == key)
        })
    }
    /// Every id the table holds, for `reserve_existing_ids` — the styles' own and
    /// their items', which a style with no users holds alone.
    pub(crate) fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.styles.values().flat_map(|s| {
            let items: Vec<NodeId> = match &s.value {
                StyleValue::Fills(v) => v.iter().map(|k| k.id.0).collect(),
                StyleValue::Strokes(v) => v.iter().map(|k| k.id.0).collect(),
                StyleValue::Effects(v) => v.iter().map(|k| k.id.0).collect(),
                StyleValue::Text(_) => Vec::new(),
            };
            std::iter::once(s.id.0).chain(items)
        })
    }
    /// A table of these styles, for the loader.
    pub(crate) fn from_list(list: Vec<Style>) -> Self {
        Self {
            styles: list.into_iter().map(|s| (s.id, s)).collect(),
        }
    }
}

/// The styles a layer is bound to, one per slot (§5.3f) — `None` on every slot
/// of every layer until somebody applies one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleRefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fills: Option<StyleId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strokes: Option<StyleId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<StyleId>,
    /// A text layer's type (§15 D1067) — `None` on every other kind of layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<StyleId>,
}

impl StyleRefs {
    pub fn get(&self, slot: StyleSlot) -> Option<StyleId> {
        match slot {
            StyleSlot::Fills => self.fills,
            StyleSlot::Strokes => self.strokes,
            StyleSlot::Effects => self.effects,
            StyleSlot::Text => self.text,
        }
    }
    pub fn set(&mut self, slot: StyleSlot, style: Option<StyleId>) {
        match slot {
            StyleSlot::Fills => self.fills = style,
            StyleSlot::Strokes => self.strokes = style,
            StyleSlot::Effects => self.effects = style,
            StyleSlot::Text => self.text = style,
        }
    }
    pub fn is_empty(&self) -> bool {
        StyleSlot::ALL
            .into_iter()
            .all(|slot| self.get(slot).is_none())
    }
    /// Each bound slot with its style.
    pub fn iter(&self) -> impl Iterator<Item = (StyleSlot, StyleId)> + '_ {
        StyleSlot::ALL
            .into_iter()
            .filter_map(|slot| self.get(slot).map(|s| (slot, s)))
    }
}

/// The operation binding `id`'s `slot` to `style`, `None` to detach. Writes the
/// binding alone: the commit writes the style's values beside it ([`settle`]).
pub fn bind_op(id: NodeId, slot: StyleSlot, style: Option<StyleId>) -> Operation {
    match slot {
        StyleSlot::Fills => Operation::SetFillStyle { id, style },
        StyleSlot::Strokes => Operation::SetStrokeStyle { id, style },
        StyleSlot::Effects => Operation::SetEffectsStyle { id, style },
        StyleSlot::Text => Operation::SetTypeStyle { id, style },
    }
}

/// The slot an operation **writes the values of**, with its node — `SetFills`,
/// `SetStrokes`, `SetEffects`, and `SetTextStyle` for the type — or `None`.
///
/// ⚠️ **`SetTextStyle` writes more than the slot** — the defaults' colour and
/// language too, which a text style does not hold ([`type_part`]). That is safe
/// because nothing acts on a write alone: [`settle`] detaches only a layer whose
/// look has left its style's, so a colour-only write leaves the binding alone.
/// The spans' ops (`SetTextSpans`, `SetText`) are not the slot's at all: styling a
/// selection keeps the style (§15 D1067).
pub fn writes_slot(op: &Operation) -> Option<(NodeId, StyleSlot)> {
    match op {
        Operation::SetFills { id, .. } => Some((*id, StyleSlot::Fills)),
        Operation::SetStrokes { id, .. } => Some((*id, StyleSlot::Strokes)),
        Operation::SetEffects { id, .. } => Some((*id, StyleSlot::Effects)),
        Operation::SetTextStyle { id, .. } => Some((*id, StyleSlot::Text)),
        _ => None,
    }
}

/// The slot an operation **binds**, with its node and the style, or `None`.
pub fn binds_slot(op: &Operation) -> Option<(NodeId, StyleSlot, Option<StyleId>)> {
    match op {
        Operation::SetFillStyle { id, style } => Some((*id, StyleSlot::Fills, *style)),
        Operation::SetStrokeStyle { id, style } => Some((*id, StyleSlot::Strokes, *style)),
        Operation::SetEffectsStyle { id, style } => Some((*id, StyleSlot::Effects, *style)),
        Operation::SetTypeStyle { id, style } => Some((*id, StyleSlot::Text, *style)),
        _ => None,
    }
}

/// Which style rule a document broke. Carried by `OpError::Styles`.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum StyleRule {
    #[error("a style has an empty name")]
    EmptyName,
    #[error("the {0:?} style name {1:?} is used twice")]
    DuplicateName(StyleSlot, String),
    #[error("{0:?} repeats an item id")]
    DuplicateItem(StyleId),
    #[error("{0:?} is bound to a variable that is not there or not a colour")]
    BadVariable(StyleId),
    #[error(
        "node {0:?} is bound to a style that is not there or of another slot, or to a text style without being text"
    )]
    BadBinding(NodeId),
    #[error("node {0:?} in the vault is bound to a style of the document's own")]
    InVault(NodeId),
    #[error(
        "{0:?} is a kit's and reads a variable of the document's own, or names a malformed kit"
    )]
    KitStyle(StyleId),
}

/// The style rules, over the table and every node's bindings — a post-condition
/// of every transaction (`Document::apply`) and of the loader.
///
/// Over the whole document for `variable::check`'s reason: removing a style
/// breaks a binding on a node the removal never dirtied. **A layer's values are
/// not compared with its style's** — [`settle`] keeps them equal at the commit,
/// and a refusal here over a drift would turn a slip in a pass into an edit the
/// user cannot make.
pub(crate) fn check<'a>(
    styles: &Styles,
    vars: &crate::variable::Variables,
    nodes: impl IntoIterator<Item = &'a crate::Node>,
    in_vault: impl Fn(NodeId) -> bool,
) -> Result<(), StyleRule> {
    let mut names: FxHashSet<(Option<&str>, StyleSlot, &str)> = FxHashSet::default();
    for s in styles.styles.values() {
        if s.name.trim().is_empty() {
            return Err(StyleRule::EmptyName);
        }
        // Unique among the document's own, and among each kit's (§15 D1064).
        if !names.insert((s.kit(), s.slot(), s.name.as_str())) {
            return Err(StyleRule::DuplicateName(s.slot(), s.name.clone()));
        }
        // A kit's style reads only kits' variables, as a vault node does.
        if let Some(k) = s.kit()
            && (!crate::meta::DocumentMeta::is_wellformed_id(k)
                || s.value
                    .vars()
                    .iter()
                    .any(|v| vars.get(*v).is_some_and(|v| v.origin.is_none())))
        {
            return Err(StyleRule::KitStyle(s.id));
        }
        if s.value.first_duplicate().is_some() {
            return Err(StyleRule::DuplicateItem(s.id));
        }
        // A paint binds a colour, a stroke's width a number (§15 D1073).
        use crate::variable::VarKind;
        let bad = |v: Option<crate::variable::VarId>, want: VarKind| {
            v.is_some_and(|v| vars.kind_of(v) != Some(want))
        };
        let wrong = match &s.value {
            StyleValue::Fills(v) => v.iter().any(|f| bad(f.brush.var(), VarKind::Color)),
            StyleValue::Strokes(v) => v
                .iter()
                .any(|x| bad(x.brush.var(), VarKind::Color) || bad(x.width_var, VarKind::Number)),
            StyleValue::Effects(_) | StyleValue::Text(_) => false,
        };
        if wrong {
            return Err(StyleRule::BadVariable(s.id));
        }
    }
    for n in nodes {
        for (slot, id) in n.styles.iter() {
            let Some(style) = styles.get(id).filter(|s| s.slot() == slot) else {
                return Err(StyleRule::BadBinding(n.id));
            };
            // Only text has type to style (§15 D1067).
            if slot == StyleSlot::Text && !matches!(n.kind, crate::NodeKind::Text { .. }) {
                return Err(StyleRule::BadBinding(n.id));
            }
            // A vault node is bound to kits' styles only (§15 D1064), as it
            // reads kits' variables only (`variable::check`).
            if in_vault(n.id) && style.origin.is_none() {
                return Err(StyleRule::InVault(n.id));
            }
        }
    }
    Ok(())
}

/// Every layer bound to `style` — the browser's usage count and its *Select*
/// (D1038 G), in id order.
pub fn users(doc: &Document, style: StyleId) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| n.styles.iter().any(|(_, s)| s == style))
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// What a transaction owes the styles (§15 D1062), appended by
/// `propagate::owed`: every layer whose slot no longer holds its style's look in
/// the document `tx` leaves is either **rewritten** with the style's values or
/// **detached** from it, its values kept.
///
/// - **Rewritten** when the transaction edited the style (`SetStyleValue`,
///   `AddStyle`) or set the binding (an apply, a reset) — the style
///   wins — and when nothing the user asked for wrote the slot: a component
///   follow carrying a main's item onto a copy bound to a style of its own, or a
///   pasted layer rebound by name to this document's style of the same name.
/// - **Detached** when one of the first `user` operations — the user's edit and
///   the doors' rewrites, everything before `propagate::owed` began — wrote the
///   slot: a canvas gesture, *Paste properties*, MCP (D1038 (1)).
///
/// Run twice by `owed`: before the component passes, so a main's rewrite or
/// detach reaches its copies, and after them, so a follow that drifted a copy
/// bound to another style is put back. `user` is the same both times.
pub fn settle(doc: &Document, tx: &Transaction, user: usize) -> Vec<Operation> {
    // The document is cloned only for a transaction that can leave a styled
    // slot off its style — most commits touch none.
    let relevant = tx.0.iter().any(|op| {
        matches!(
            op,
            Operation::AddStyle { .. } | Operation::SetStyleValue { .. }
        ) || matches!(op, Operation::InsertSubtree { nodes, .. }
            if nodes.iter().any(|n| !n.styles.is_empty()))
            || binds_slot(op).is_some()
            || writes_slot(op)
                .is_some_and(|(id, slot)| doc.get(id).is_some_and(|n| n.styles.get(slot).is_some()))
    });
    if !relevant {
        return Vec::new();
    }
    let mut after = doc.clone();
    if after.apply_unchecked(tx).is_err() {
        return Vec::new(); // `apply` refuses it and says why
    }
    let mut restyled: FxHashSet<StyleId> = FxHashSet::default();
    let mut rebound: FxHashSet<(NodeId, StyleSlot)> = FxHashSet::default();
    for op in &tx.0 {
        match op {
            Operation::AddStyle { style } => {
                restyled.insert(style.id);
            }
            Operation::SetStyleValue { id, .. } => {
                restyled.insert(*id);
            }
            _ => {
                if let Some((n, slot, _)) = binds_slot(op) {
                    rebound.insert((n, slot));
                }
            }
        }
    }
    let written: FxHashSet<(NodeId, StyleSlot)> = tx.0[..user.min(tx.0.len())]
        .iter()
        .filter_map(writes_slot)
        .collect();

    let mut bound: Vec<&crate::Node> = after
        .node_map()
        .values()
        .filter(|n| !n.styles.is_empty())
        .collect();
    bound.sort_by_key(|n| n.id);
    let mut out = Vec::new();
    for n in bound {
        for (slot, s) in n.styles.iter() {
            let Some(style) = after.styles().get(s) else {
                continue; // `check` refuses it
            };
            if slot == StyleSlot::Text && !matches!(n.kind, crate::NodeKind::Text { .. }) {
                continue; // `check` refuses it, and says why where `SetTextStyle` would not
            }
            // 🚨 **A text style's family is plain, and a bound family under it
            // goes** (§15 D1069). `build::keep_text_bindings` detaches it where
            // the user applies the style, but a copy receives its main's
            // `SetTypeStyle` from propagation, after that pass — and kept bound,
            // it drew its variable over the family written here.
            if slot == StyleSlot::Text && n.text_vars[1].is_some() {
                out.push(Operation::SetTextVars {
                    id: n.id,
                    vars: [n.text_vars[0], None],
                });
            }
            let cur = StyleValue::of(n, slot);
            if cur.same_look(&style.value) {
                continue;
            }
            let wins = restyled.contains(&s) || rebound.contains(&(n.id, slot));
            if !wins && written.contains(&(n.id, slot)) {
                out.push(bind_op(n.id, slot, None));
            } else {
                out.push(style.value.write_op(n));
            }
        }
    }
    out
}

/// `tx` with every write of `style`'s slot on a layer bound to it **turned into
/// an edit of the style** (§15 D1063) — the door the browser's *Edit* opens: the
/// inspector's own cards edit the style's users, and this makes the first such
/// list the style's value and drops the per-layer writes, which [`settle`] then
/// writes back onto every user, the edited layers among them. A write on any
/// other layer or slot passes through.
///
/// A text style's write **also passes through** (§15 D1067): `SetTextStyle`
/// carries the layer's colour and language beside the type, which the style does
/// not hold, so dropping it would drop a colour picked in the same edit. Kept, it
/// lands the layer on the style's new look, and [`settle`] has nothing to do there.
pub fn edit_through(doc: &Document, tx: Transaction, style: StyleId) -> Transaction {
    let Some(slot) = doc.styles().get(style).map(Style::slot) else {
        return tx;
    };
    let bound = |id: NodeId| {
        doc.get(id)
            .is_some_and(|n| n.styles.get(slot) == Some(style))
    };
    // A text style's users whose defaults this edit writes. The Type card's write
    // of the defaults carries a `SetTextSpans` clearing the attribute's spans over
    // the whole layer (§15 D164) — right for a plain layer, wrong here: the style's
    // other users keep their spans when it changes, so the edited one must too,
    // or *Body*'s size edit flattens a 30px word on the layer it was typed on and
    // nowhere else (`arch-scribe`'s read of D1068's first cut).
    let restyled: FxHashSet<NodeId> = if slot == StyleSlot::Text {
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetTextStyle { id, .. } if bound(*id) => Some(*id),
                _ => None,
            })
            .collect()
    } else {
        FxHashSet::default()
    };
    let mut value: Option<StyleValue> = None;
    let mut out = Vec::with_capacity(tx.0.len());
    for op in tx.0 {
        if matches!(&op, Operation::SetTextSpans { id, .. } if restyled.contains(id)) {
            continue;
        }
        let hit = writes_slot(&op).is_some_and(|(id, s)| s == slot && bound(id));
        if !hit {
            out.push(op);
            continue;
        }
        if let Operation::SetTextStyle { style: s, .. } = &op {
            if value.is_none() {
                value = Some(StyleValue::Text(Box::new(type_part(s))));
            }
            out.push(op);
            continue;
        }
        if value.is_none() {
            value = Some(match op {
                Operation::SetFills { fills, .. } => StyleValue::Fills(fills),
                Operation::SetStrokes { strokes, .. } => StyleValue::Strokes(strokes),
                Operation::SetEffects { effects, .. } => StyleValue::Effects(effects),
                _ => unreachable!("`writes_slot` answered for it"),
            });
        }
    }
    if let Some(value) = value {
        out.push(Operation::SetStyleValue { id: style, value });
    }
    Transaction(out)
}

/// The operations that delete `styles` (D1038 (6)): **every layer bound to one
/// detaches, its values kept**, then the styles go — one transaction, one
/// `Ctrl+Z`. The commit refuses a bare `RemoveStyle` of a style in use
/// ([`check`]), so this is the only shape a delete of one can take.
pub fn remove_ops(doc: &Document, styles: &[StyleId]) -> Vec<Operation> {
    let mut ops = Vec::new();
    let mut nodes: Vec<&crate::Node> = doc
        .node_map()
        .values()
        .filter(|n| n.styles.iter().any(|(_, s)| styles.contains(&s)))
        .collect();
    nodes.sort_by_key(|n| n.id);
    for n in nodes {
        for (slot, s) in n.styles.iter() {
            if styles.contains(&s) {
                ops.push(bind_op(n.id, slot, None));
            }
        }
    }
    for s in styles {
        if doc.styles().get(*s).is_some() {
            ops.push(Operation::RemoveStyle { id: *s });
        }
    }
    ops
}

/// The operations that make a style named `name` from `from`'s `slot` and bind
/// every one of `nodes` to it (*Create style from this…*, D1038 (5)) — the new
/// style's values are `from`'s list, its item ids kept, so `from` already holds
/// its look and the others are rewritten by the commit.
pub fn create_ops(
    doc: &Document,
    id: StyleId,
    name: &str,
    from: NodeId,
    slot: StyleSlot,
    nodes: &[NodeId],
) -> Option<Vec<Operation>> {
    // A bound family is read as drawn (§15 D1069): a text style holds a plain
    // family, and `from`'s literal is empty. Binding `from` then detaches its
    // family (`build::keep_text_bindings`), the style's being the same string.
    let mut source = doc.get(from)?.clone();
    if slot == StyleSlot::Text && source.text_vars[1].is_some() {
        source.kind = doc.text_drawn_in(doc.variables(), from, &source.kind, source.text_vars);
    }
    let value = StyleValue::of(&source, slot);
    let mut ops = vec![Operation::AddStyle {
        style: Style {
            id,
            name: name.to_string(),
            value,
            origin: None,
            forked: None,
        },
    }];
    ops.extend(nodes.iter().map(|n| bind_op(*n, slot, Some(id))));
    Some(ops)
}

/// A name for a new style of `slot` that no style there holds: `base`, else
/// `base 2`, `base 3`… The browser's and *Create*'s default.
pub fn free_name(styles: &Styles, slot: StyleSlot, base: &str) -> String {
    if styles.by_name(slot, base).is_none() {
        return base.to_string();
    }
    (2..)
        .map(|k| format!("{base} {k}"))
        .find(|n| styles.by_name(slot, n).is_none())
        .expect("an unbounded range finds a free name")
}

/// `value` with every paint bound to one of `vars` **detached to the colour it
/// draws in `mode`** — what deleting a variable does to the styles that use it
/// (`variable::remove_ops`), in the document's default mode: a style is read by
/// layers in many modes and holds one value. `None` in a document with no modes,
/// where a kit's variable reads its kit's default (§15 D1064).
pub(crate) fn detach_vars(
    value: &StyleValue,
    t: &crate::variable::Variables,
    mode: Option<crate::variable::ModeId>,
    vars: &[crate::variable::VarId],
) -> StyleValue {
    use crate::variable::{BrushSource, Value};
    let drawn = |b: &BrushSource| -> BrushSource {
        match b {
            BrushSource::Bound { var, alpha } if vars.contains(var) => {
                let c = match t.resolve_opt(*var, mode) {
                    Some(Value::Color(c)) => c.multiply_alpha(*alpha),
                    _ => peniko::Color::TRANSPARENT,
                };
                crate::Brush::Solid(c).into()
            }
            other => other.clone(),
        }
    };
    match value {
        StyleValue::Fills(v) => StyleValue::Fills(
            v.iter()
                .map(|f| {
                    f.map(|f| Fill {
                        brush: drawn(&f.brush),
                        ..f.clone()
                    })
                })
                .collect(),
        ),
        StyleValue::Strokes(v) => StyleValue::Strokes(
            v.iter()
                .map(|s| {
                    s.map(|s| {
                        // And a width bound to one of them, its number in `mode`
                        // (§15 D1073).
                        let (width, width_var) = match s.width_var {
                            Some(v) if vars.contains(&v) => (
                                match t.resolve_opt(v, mode) {
                                    Some(Value::Number(x)) => x.max(0.0),
                                    _ => 0.0,
                                },
                                None,
                            ),
                            other => (s.width, other),
                        };
                        Stroke {
                            brush: drawn(&s.brush),
                            width,
                            width_var,
                            ..s.clone()
                        }
                    })
                })
                .collect(),
        ),
        StyleValue::Effects(_) | StyleValue::Text(_) => value.clone(),
    }
}

/// One style a copy's layers are bound to, as the copy knows it — enough for a
/// paste to rebind by name and slot (§15 D1062).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarriedStyle {
    pub id: StyleId,
    pub name: String,
    pub slot: StyleSlot,
    /// A kit's style's origin (§15 D1064), which a paste rebinds by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<StyleOrigin>,
}

/// The variables' paste rule made for a style (§15 D1062, D1038's rule for a
/// variable): `s` itself where `styles` has it at the slot, else `styles`' style
/// of the **same name and slot**, else `None` — the layer keeps the values it
/// was copied with, detached. A kit's style rebinds by its origin instead, as a
/// kit's variable does (§15 D1064).
pub fn rebind(
    styles: &Styles,
    carried: &[CarriedStyle],
    slot: StyleSlot,
    s: StyleId,
) -> Option<StyleId> {
    if styles.get(s).is_some_and(|st| st.slot() == slot) {
        return Some(s);
    }
    let c = carried.iter().find(|c| c.id == s && c.slot == slot)?;
    match &c.origin {
        Some(o) => styles
            .by_origin((&o.kit, o.style))
            .filter(|st| st.slot() == slot)
            .map(|st| st.id),
        None => styles.by_name(slot, &c.name).map(|st| st.id),
    }
}
