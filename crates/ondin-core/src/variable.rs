//! Variables and modes (§5.3f, §15 D1037, D1038, D1039).
//!
//! A **variable** is a named value of one type with **one value per mode** of
//! its [`Collection`]. Each collection has its own mode list, its default the
//! first. A frame may set one mode per collection for itself and everything
//! under it ([`crate::Node::modes`]); each collection inherits on its own, and
//! anything below no frame that sets one of a collection's modes reads that
//! collection's default.
//!
//! A cell is a value or an **alias** — another variable of the same type, named
//! per mode, in any collection — never a fifth type. The target reads its own
//! collection's mode in force. A variable's **scope** ([`Scope`]) is the fields
//! it may bind. A field bound to a variable holds
//! [`BrushSource::Bound`]; the value it draws is found **at read**, against the mode
//! in force at the node, and is in no node (invariant 4, §12 property 4).
//!
//! The table is the third piece of document state that is not a node, after the
//! guides and the images, and it is changed the way they are: by operations that
//! name a variable or a mode rather than a node ([`crate::Operation`]'s
//! `*Variable*` and `*Mode*` arms). Its rules are a post-condition of every
//! transaction ([`check`]), as the component rules are, because a binding breaks
//! when its *target* goes.

use crate::id::NodeId;
use peniko::Color;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

macro_rules! wire_id {
    ($name:ident, $what:literal) => {
        /// A [`NodeId`] under a newtype, minted from the session's one
        /// [`crate::IdSource`] as a guide's is — so it can never collide with a
        /// node's, and `reserve_existing_ids` covers it without a second counter.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
        pub struct $name(pub NodeId);

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!($what, " {:?}"), self.0)
            }
        }

        /// Written as the node id's wire form, `"<actor-hex>:<seq>"`.
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0.to_wire())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                NodeId::from_wire(&s)
                    .map($name)
                    .ok_or_else(|| serde::de::Error::custom(format!("bad id {s:?}")))
            }
        }
    };
}

wire_id!(VarId, "variable");
wire_id!(ModeId, "mode");
wire_id!(CollectionId, "collection");

impl CollectionId {
    /// The id of the collection a document's variables open in when its file
    /// names none — every document saved before collections. Never minted: actor
    /// 0 is no session's.
    pub const FIRST: CollectionId = CollectionId(NodeId { actor: 0, seq: 1 });

    pub fn is_first(&self) -> bool {
        *self == Self::FIRST
    }
}

impl crate::IdSource {
    /// A fresh [`VarId`].
    pub fn mint_var(&mut self) -> VarId {
        VarId(self.mint())
    }
    /// A fresh [`ModeId`].
    pub fn mint_mode(&mut self) -> ModeId {
        ModeId(self.mint())
    }
    /// A fresh [`CollectionId`].
    pub fn mint_collection(&mut self) -> CollectionId {
        CollectionId(self.mint())
    }
}

/// One of the document's modes — a column of its collection's table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mode {
    pub id: ModeId,
    pub name: String,
}

/// The name the first collection takes when its file names none.
pub const FIRST_COLLECTION_NAME: &str = "Variables";

/// A named set of variables with modes of its own, the first its default.
///
/// **A variable's collection is the one owning its cells' modes**
/// ([`Variables::collection_of`]), so a variable records none and moving one is
/// rewriting its cells. A frame sets at most one mode per collection, and each
/// collection's mode is inherited on its own ([`crate::Document::modes_at`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub id: CollectionId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<Mode>,
    /// Each mode's minimum width in px when the collection is **picked by
    /// width**, `None` when it is not ([`Breakpoints`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<Breakpoints>,
}

/// A by-width collection's minimum width per mode, keyed by mode id so adding,
/// removing or moving a mode leaves the other widths where they are. The first
/// mode's is 0 and the widths ascend in mode order ([`check`]), so the default is
/// always the narrowest.
pub type Breakpoints = BTreeMap<ModeId, f64>;

impl Collection {
    /// The collection every document starts with: [`CollectionId::FIRST`],
    /// named [`FIRST_COLLECTION_NAME`], no modes.
    pub fn first() -> Collection {
        Collection {
            id: CollectionId::FIRST,
            name: FIRST_COLLECTION_NAME.into(),
            modes: Vec::new(),
            breakpoints: None,
        }
    }

    pub fn default_mode(&self) -> Option<ModeId> {
        self.modes.first().map(|m| m.id)
    }

    pub fn has(&self, mode: ModeId) -> bool {
        self.modes.iter().any(|m| m.id == mode)
    }

    /// Whether a top-level frame picks this collection's mode from its width.
    pub fn by_width(&self) -> bool {
        self.breakpoints.is_some()
    }

    /// `mode`'s minimum width, in a collection picked by width.
    pub fn min_width(&self, mode: ModeId) -> Option<f64> {
        self.breakpoints.as_ref()?.get(&mode).copied()
    }

    /// The mode a frame `width` wide picks: the one with the largest minimum it
    /// reaches, else the first. `None` when the collection is not picked by
    /// width or has no modes.
    pub fn mode_for_width(&self, width: f64) -> Option<ModeId> {
        let b = self.breakpoints.as_ref()?;
        self.modes
            .iter()
            .rev()
            .find(|m| b.get(&m.id).is_some_and(|min| width >= *min))
            .or(self.modes.first())
            .map(|m| m.id)
    }
}

/// A family of fields a variable may bind — its **scope** is a set of these.
///
/// Grouped as the inspector's cards group them, each binding one kind
/// ([`Field::kind`]). Visibility is the boolean's one field, so a boolean has
/// no scope to narrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    /// A width or a height.
    Size,
    /// A min or max width or height.
    Limits,
    /// A gap or a padding.
    Spacing,
    Corner,
    StrokeWidth,
    Opacity,
    /// An effect's X, Y, blur or spread.
    Effect,
    /// A text's size, line height, letter or paragraph spacing.
    Type,
    Weight,
    Fill,
    Stroke,
    Content,
    Family,
    /// A flex container's direction: a string naming one, *Row* or *Column*.
    FlexDirection,
    /// A flex container's wrap: *Wrap*, *No wrap*.
    FlexWrap,
    Visible,
}

impl Field {
    pub const ALL: [Field; 16] = [
        Field::Size,
        Field::Limits,
        Field::Spacing,
        Field::Corner,
        Field::StrokeWidth,
        Field::Opacity,
        Field::Effect,
        Field::Type,
        Field::Weight,
        Field::Fill,
        Field::Stroke,
        Field::Content,
        Field::Family,
        Field::FlexDirection,
        Field::FlexWrap,
        Field::Visible,
    ];

    /// The kind of variable this field binds.
    pub fn kind(self) -> VarKind {
        match self {
            Field::Fill | Field::Stroke => VarKind::Color,
            Field::Content | Field::Family | Field::FlexDirection | Field::FlexWrap => {
                VarKind::String
            }
            Field::Visible => VarKind::Boolean,
            _ => VarKind::Number,
        }
    }

    /// Every field binding `kind`, in [`Field::ALL`]'s order.
    pub fn of_kind(kind: VarKind) -> impl Iterator<Item = Field> {
        Field::ALL.into_iter().filter(move |f| f.kind() == kind)
    }

    /// Whether the field holds a length in px — what makes a number scoped to
    /// these alone a DTCG `dimension`. A text's type numbers are px or `%`;
    /// counted as lengths, as the field shows px by default.
    pub fn is_length(self) -> bool {
        matches!(
            self,
            Field::Size
                | Field::Limits
                | Field::Spacing
                | Field::Corner
                | Field::StrokeWidth
                | Field::Effect
                | Field::Type
        )
    }

    /// The name a scope is saved and exported under.
    pub fn wire(self) -> &'static str {
        match self {
            Field::Size => "size",
            Field::Limits => "limits",
            Field::Spacing => "spacing",
            Field::Corner => "corner",
            Field::StrokeWidth => "strokeWidth",
            Field::Opacity => "opacity",
            Field::Effect => "effect",
            Field::Type => "type",
            Field::Weight => "weight",
            Field::Fill => "fill",
            Field::Stroke => "stroke",
            Field::Content => "content",
            Field::Family => "family",
            Field::FlexDirection => "flexDirection",
            Field::FlexWrap => "flexWrap",
            Field::Visible => "visible",
        }
    }

    /// Whether only a variable scoped to it binds it — not one with no scope,
    /// which binds every other field of its kind. A flex direction or wrap
    /// takes only its own words, so an unscoped string (a label's *Continue*)
    /// never lands on one.
    pub fn needs_scope(self) -> bool {
        matches!(self, Field::FlexDirection | Field::FlexWrap)
    }

    /// The words a string bound to it may hold, when it takes only its own (a
    /// direction's, a wrap's); empty for any other field.
    pub fn words(self) -> Vec<&'static str> {
        use crate::container::{FlexDirection, FlexWrap};
        match self {
            Field::FlexDirection => FlexDirection::ALL.map(FlexDirection::word).to_vec(),
            Field::FlexWrap => FlexWrap::ALL.map(FlexWrap::word).to_vec(),
            _ => Vec::new(),
        }
    }

    /// Whether `s` is one of [`Self::words`] (or its CSS keyword), for a field
    /// that takes only its own.
    pub fn takes(self, s: &str) -> bool {
        use crate::container::{FlexDirection, FlexWrap};
        match self {
            Field::FlexDirection => FlexDirection::from_word(s).is_some(),
            Field::FlexWrap => FlexWrap::from_word(s).is_some(),
            _ => true,
        }
    }

    pub fn from_wire(s: &str) -> Option<Field> {
        Field::ALL.into_iter().find(|f| f.wire() == s)
    }

    fn bit(self) -> u16 {
        1 << Field::ALL.iter().position(|f| *f == self).expect("listed")
    }
}

/// The fields a variable may bind, when not every field of its kind
/// ([`Variable::scope`]). Saved as a list of [`Field::wire`] names in
/// [`Field::ALL`]'s order; an unknown name is dropped on load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Scope(u16);

impl Scope {
    pub fn of(fields: impl IntoIterator<Item = Field>) -> Scope {
        Scope(fields.into_iter().fold(0, |b, f| b | f.bit()))
    }
    pub fn contains(self, f: Field) -> bool {
        self.0 & f.bit() != 0
    }
    pub fn with(self, f: Field, on: bool) -> Scope {
        Scope(if on {
            self.0 | f.bit()
        } else {
            self.0 & !f.bit()
        })
    }
    pub fn fields(self) -> impl Iterator<Item = Field> {
        Field::ALL.into_iter().filter(move |f| self.contains(*f))
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl Serialize for Scope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.fields().map(Field::wire))
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(d)?;
        Ok(Scope::of(names.iter().filter_map(|n| Field::from_wire(n))))
    }
}

/// What a variable holds. Build step 1's two, step 2's boolean, bound to a
/// layer's visibility (D1038 (d), §15 D1059), and step 6's string, bound to a
/// text's content and its family (§15 D1069).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VarKind {
    Color,
    Number,
    Boolean,
    String,
}

/// One variable's value in one mode.
///
/// Not `Copy` since the string kind (§15 D1069): a cell is cloned where it was
/// copied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cell {
    Color(Color),
    Number(f64),
    Boolean(bool),
    String(String),
    /// Another variable of the same kind, read in the same mode (§5.3f).
    Alias(VarId),
}

impl Cell {
    /// The kind of value this cell holds, `None` for an alias — whose kind is its
    /// target's.
    pub fn kind(&self) -> Option<VarKind> {
        match self {
            Cell::Color(_) => Some(VarKind::Color),
            Cell::Number(_) => Some(VarKind::Number),
            Cell::Boolean(_) => Some(VarKind::Boolean),
            Cell::String(_) => Some(VarKind::String),
            Cell::Alias(_) => None,
        }
    }

    fn is_finite(&self) -> bool {
        match self {
            Cell::Color(c) => c.components.iter().all(|x| x.is_finite()),
            Cell::Number(n) => n.is_finite(),
            Cell::Boolean(_) | Cell::String(_) | Cell::Alias(_) => true,
        }
    }
}

/// A resolved value: what a cell comes to once its aliases are followed.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Color(Color),
    Number(f64),
    Boolean(bool),
    String(String),
}

impl Value {
    /// The string, `None` for any other kind.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

impl From<Value> for Cell {
    /// The cell holding `v` itself — what an alias becomes when its target goes.
    fn from(v: Value) -> Cell {
        match v {
            Value::Color(c) => Cell::Color(c),
            Value::Number(n) => Cell::Number(n),
            Value::Boolean(b) => Cell::Boolean(b),
            Value::String(s) => Cell::String(s),
        }
    }
}

/// A variable: a name, a kind and one cell per mode.
///
/// **The name is a path** — `brand/500` — and the slashes are the browser's
/// groups (D1038 G); there is no group object. Names are unique within each of
/// the document's collections, and within each kit ([`check`]), which is what
/// lets a paste from another document rebind by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub id: VarId,
    pub name: String,
    pub kind: VarKind,
    /// One cell per mode, keyed by mode — **every** mode of one collection of the
    /// document for one of its own, which makes it that collection's, every mode
    /// of its kit for a kit's ([`KitModes`]); [`check`] holds this, so a read
    /// never meets a missing column. Ordered by id for byte-stable saves
    /// (invariant 9), not by the modes' order.
    pub cells: BTreeMap<ModeId, Cell>,
    /// The fields it may bind, `None` for every field of its kind — including
    /// any that becomes bindable later. A boolean has none ([`check`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// Which kit it came from, for a variable a vendored main reads (§15 D1064)
    /// — `None` for the document's own. A kit's variable is **read-only** here, as
    /// a vendored main is: written by an update and by nothing the user does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<VarOrigin>,
    /// The kit variable this one of the document's own was **forked from** by
    /// *Make local* (§15 D1066) — `None` for every other. A mark, never a link:
    /// nothing updates a fork; a second *Make local* of the same kit binds to it
    /// rather than forking the kit's variable twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked: Option<VarOrigin>,
    /// What the variable is for, in the user's words; empty is none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl Variable {
    /// The kit this variable came from, `None` for the document's own.
    pub fn kit(&self) -> Option<&str> {
        self.origin.as_ref().map(|o| o.kit.as_str())
    }

    /// Whether it may bind `field`: of the field's kind, and in its scope.
    pub fn binds(&self, field: Field) -> bool {
        self.kind == field.kind()
            && match self.scope {
                None => !field.needs_scope(),
                Some(s) => s.contains(field),
            }
    }
}

/// Where a kit's variable came from (§15 D1064): the kit's `meta.id`, the
/// variable in that kit's published state, and the version it was published at
/// — `crate::kit::Origin` for a variable. Its own id is minted here, as a vendored
/// node's is (§15 D1010), and [`Self::key`] is what an update and a second
/// insert match on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarOrigin {
    pub kit: String,
    pub var: VarId,
    pub version: u32,
}

impl VarOrigin {
    /// Which kit, which variable there — the version left out, as
    /// `crate::kit::Origin::key` leaves it.
    pub fn key(&self) -> (&str, VarId) {
        (&self.kit, self.var)
    }
}

/// One kit's modes, as a document holding its variables keeps them (§15 D1064,
/// D1038 (b)): the kit's own lists, one per collection of the kit, and **the
/// per-kit mapping** from the document's modes onto them.
///
/// A kit's modes are not modes of the document, no frame of the document's own
/// sets one, and they appear nowhere but as the columns its variables are read
/// in. **Each kit collection maps from one document collection** — the one of
/// its name, else the first ([`Variables::kit_onto`]). Which column a layer
/// reads is the mapping's ([`Variables::kit_mode_at`]): the entry set by hand,
/// else the kit mode **of the same name** as that collection's mode in force,
/// else the kit collection's **first** mode.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KitModes {
    /// The kit's name when its variables arrived — the browser's group header,
    /// as `crate::kit::Origin::name` names a lost kit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The kit's modes, collection by collection, each in its order — a
    /// collection's first its fallback.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<KitMode>,
    /// The mapping's hand-set entries, a mode of the document to a mode of the
    /// kit. A document mode with no entry maps by name, then to the first.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub map: BTreeMap<ModeId, ModeId>,
}

impl KitModes {
    pub fn mode(&self, id: ModeId) -> Option<&KitMode> {
        self.modes.iter().find(|m| m.id == id)
    }
    /// The modes of kit collection `collection`, in order.
    pub fn modes_in<'a>(&'a self, collection: &'a str) -> impl Iterator<Item = &'a KitMode> + 'a {
        self.modes
            .iter()
            .filter(move |m| m.collection == collection)
    }
    /// The kit's collections by name, in order — `""` for a kit's only one.
    pub fn collections(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for m in &self.modes {
            if !out.contains(&m.collection.as_str()) {
                out.push(&m.collection);
            }
        }
        out
    }
}

/// One of a kit's modes, held here under an id of this document's (`id`) and
/// remembering the kit's own (`from`), which an update matches on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KitMode {
    pub id: ModeId,
    pub name: String,
    pub from: ModeId,
    /// The name of the kit's collection it is a mode of — empty for a kit with
    /// one collection, so a kit published before collections reads as it did.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub collection: String,
}

/// The document's variables and its collections.
#[derive(Clone, Debug, PartialEq)]
pub struct Variables {
    /// Never empty ([`check`]): a document starts with [`Collection::first`].
    pub(crate) collections: Vec<Collection>,
    pub(crate) vars: BTreeMap<VarId, Variable>,
    /// The modes of each kit whose variables this document holds, by kit id
    /// (§15 D1064).
    pub(crate) kits: BTreeMap<String, KitModes>,
    /// A frame read as if it were this wide, for the mode it picks by width —
    /// only in a preview's table ([`Self::with_width`]), never saved.
    pub(crate) previewed_width: Option<(NodeId, f64)>,
}

impl Default for Variables {
    fn default() -> Self {
        Variables {
            collections: vec![Collection::first()],
            vars: BTreeMap::new(),
            kits: BTreeMap::new(),
            previewed_width: None,
        }
    }
}

impl Variables {
    /// The document's collections, in order.
    pub fn collections(&self) -> &[Collection] {
        &self.collections
    }
    pub fn collection(&self, id: CollectionId) -> Option<&Collection> {
        self.collections.iter().find(|c| c.id == id)
    }
    pub fn collection_index(&self, id: CollectionId) -> Option<usize> {
        self.collections.iter().position(|c| c.id == id)
    }
    /// The first collection — where a document with one keeps everything.
    pub fn first_collection(&self) -> &Collection {
        &self.collections[0]
    }
    /// Whether the document holds more than one collection — when the browser,
    /// the frame's mode rows and the tooltips name them.
    pub fn has_collections(&self) -> bool {
        self.collections.len() > 1
    }
    /// The collection picked by width, if one is — at most one is ([`check`]).
    pub fn by_width(&self) -> Option<&Collection> {
        self.collections.iter().find(|c| c.by_width())
    }
    /// The document's collection holding `mode` — `None` for a kit's mode.
    pub fn collection_of_mode(&self, mode: ModeId) -> Option<&Collection> {
        self.collections.iter().find(|c| c.has(mode))
    }
    /// The document's collection a variable of its own is in: the one owning
    /// its cells' modes. `None` for a kit's variable, or none at all.
    pub fn collection_of(&self, var: VarId) -> Option<&Collection> {
        let m = self.vars.get(&var)?.cells.keys().next()?;
        self.collection_of_mode(*m)
    }
    /// The document's own variables in collection `c`, in id order.
    pub fn of_collection(&self, c: CollectionId) -> impl Iterator<Item = &Variable> {
        let modes: Vec<ModeId> = self
            .collection(c)
            .map(|c| c.modes.iter().map(|m| m.id).collect())
            .unwrap_or_default();
        self.local()
            .filter(move |v| v.cells.keys().next().is_some_and(|m| modes.contains(m)))
    }
    /// Every one of the document's modes, collection by collection — never a
    /// kit's.
    pub fn modes(&self) -> impl Iterator<Item = &Mode> {
        self.collections.iter().flat_map(|c| c.modes.iter())
    }
    /// The first collection's default mode — what a document with one
    /// collection reads everywhere no frame sets one — or `None` when it has no
    /// modes.
    pub fn default_mode(&self) -> Option<ModeId> {
        self.collections.first().and_then(Collection::default_mode)
    }
    /// One of the **document's** modes — never a kit's.
    pub fn mode(&self, id: ModeId) -> Option<&Mode> {
        self.modes().find(|m| m.id == id)
    }
    /// Where `id` sits in its collection's list — index 0 is its default.
    pub fn mode_index(&self, id: ModeId) -> Option<usize> {
        self.collection_of_mode(id)?
            .modes
            .iter()
            .position(|m| m.id == id)
    }
    /// The default of the collection `mode` is a mode of, or `None` for a
    /// kit's mode.
    pub fn default_of(&self, mode: ModeId) -> Option<ModeId> {
        self.collection_of_mode(mode)?.default_mode()
    }
    /// The kits whose variables this document holds, and their modes.
    pub fn kits(&self) -> &BTreeMap<String, KitModes> {
        &self.kits
    }
    /// The kit `mode` is a mode of, with the mode — `None` for one of the
    /// document's own, or none at all.
    pub fn kit_mode_of(&self, mode: ModeId) -> Option<(&str, &KitMode)> {
        self.kits
            .iter()
            .find_map(|(k, km)| km.mode(mode).map(|m| (k.as_str(), m)))
    }
    /// Whether `mode` is a mode of the document or of a kit it holds.
    pub fn has_mode(&self, mode: ModeId) -> bool {
        self.mode(mode).is_some() || self.kit_mode_of(mode).is_some()
    }
    pub fn get(&self, id: VarId) -> Option<&Variable> {
        self.vars.get(&id)
    }
    /// Every variable, the kits' among them, in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Variable> {
        self.vars.values()
    }
    /// The document's own variables, in id order — no kit's.
    pub fn local(&self) -> impl Iterator<Item = &Variable> {
        self.vars.values().filter(|v| v.origin.is_none())
    }
    /// The variables of `kit`, in id order.
    pub fn of_kit<'a>(&'a self, kit: &'a str) -> impl Iterator<Item = &'a Variable> + 'a {
        self.vars.values().filter(move |v| v.kit() == Some(kit))
    }
    /// The variable whose origin is `key`, a kit's variable held here.
    pub fn by_origin(&self, key: (&str, VarId)) -> Option<&Variable> {
        self.vars
            .values()
            .find(|v| v.origin.as_ref().is_some_and(|o| o.key() == key))
    }
    /// Whether the table holds nothing a file need write: no variable, no kit,
    /// and the one collection every document starts with, without modes.
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.kits.is_empty() && self.collections == [Collection::first()]
    }
    /// The document's own variable named `name`, the first collection's first
    /// — a kit's are named in their kit, and a paste rebinds to one of those by
    /// its origin instead.
    pub fn by_name(&self, name: &str) -> Option<&Variable> {
        self.collections
            .iter()
            .find_map(|c| self.by_name_in(c.id, name))
    }
    /// The document's own variable named `name` in collection `c`.
    pub fn by_name_in(&self, c: CollectionId, name: &str) -> Option<&Variable> {
        self.of_collection(c).find(|v| v.name == name)
    }

    /// The modes a variable's cells are keyed by: its collection's for one of
    /// the document's own, its kit's for a kit's.
    pub fn columns(&self, v: &Variable) -> Vec<ModeId> {
        let first = v.cells.keys().next().copied();
        match v.kit() {
            None => first
                .and_then(|m| self.collection_of_mode(m))
                .map(|c| c.modes.iter().map(|m| m.id).collect()),
            // A first cell that is none of its kit's names the kit's first
            // collection, so the check finds the stray and the missing cells.
            Some(kit) => self.kits.get(kit).map(|k| {
                let c = first
                    .and_then(|m| k.mode(m))
                    .or(k.modes.first())
                    .map_or("", |m| m.collection.as_str());
                k.modes_in(c).map(|m| m.id).collect()
            }),
        }
        .unwrap_or_else(|| v.cells.keys().copied().collect())
    }

    /// The kit collection a kit's variable is in, by name — `""` for a kit's
    /// only one. `None` for one of the document's own.
    pub fn kit_collection_of<'a>(&'a self, v: &Variable) -> Option<&'a str> {
        let k = self.kits.get(v.kit()?)?;
        Some(k.mode(*v.cells.keys().next()?)?.collection.as_str())
    }

    /// The document collection kit collection `collection` maps from: the one
    /// of its name, else the first.
    pub fn kit_onto(&self, collection: &str) -> Option<&Collection> {
        self.collections
            .iter()
            .find(|c| !collection.is_empty() && c.name == collection)
            .or(self.collections.first())
    }

    /// The column of `kit`'s first collection read when `mode` is in force
    /// (§15 D1064, D1038 (b)): [`Self::kit_mode_at`] with that one mode set,
    /// `None` every collection's default. `None` only for a kit this table does
    /// not hold.
    pub fn kit_mode(&self, kit: &str, mode: Option<ModeId>) -> Option<ModeId> {
        let first = self.kits.get(kit)?.modes.first()?.collection.clone();
        self.kit_mode_at(kit, &first, mode.as_slice())
    }

    /// The column of `kit`'s collection `collection` read under `set`, the
    /// modes in force nearest first ([`crate::Document::modes_at`]). The first
    /// of them that speaks for it wins: one of its own (a kit main's frame set
    /// to it), or a mode of the document collection it maps from — through the
    /// mapping's hand-set entry, else its mode of that name, else its first — or
    /// another kit's mode it has one of the same name as (that kit's variable
    /// having aliased this kit's). With none, that collection's default maps
    /// the same way.
    pub fn kit_mode_at(&self, kit: &str, collection: &str, set: &[ModeId]) -> Option<ModeId> {
        let k = self.kits.get(kit)?;
        let own = || k.modes_in(collection);
        let onto = self.kit_onto(collection);
        let mapped = |m: ModeId| {
            if let Some(to) = k.map.get(&m)
                && own().any(|x| x.id == *to)
            {
                return Some(*to);
            }
            let name = self.mode(m).map(|dm| dm.name.as_str())?;
            Some(own().find(|x| x.name == name).or_else(|| own().next())?.id)
        };
        for &m in set {
            if own().any(|x| x.id == m) {
                return Some(m);
            }
            if onto.is_some_and(|c| c.has(m)) {
                return mapped(m);
            }
            if let Some((_, other)) = self.kit_mode_of(m)
                && let Some(named) = own().find(|x| x.name == other.name)
            {
                return Some(named.id);
            }
        }
        match onto.and_then(Collection::default_mode) {
            Some(d) => mapped(d),
            None => own().next().map(|x| x.id),
        }
    }

    /// The column `v` reads under `set` — the mode of its collection in force,
    /// else that collection's default, for one of the document's own; the mapped
    /// column for a kit's.
    fn column_at(&self, v: &Variable, set: &[ModeId]) -> Option<ModeId> {
        match v.kit() {
            None => {
                let c = self.collection_of_mode(*v.cells.keys().next()?)?;
                set.iter()
                    .copied()
                    .find(|m| c.has(*m))
                    .or_else(|| c.default_mode())
            }
            Some(kit) => self.kit_mode_at(kit, self.kit_collection_of(v)?, set),
        }
    }

    /// `var`'s value in `mode`, its aliases followed: [`Self::resolve_at`] with
    /// that one mode in force, every other collection reading its default.
    ///
    /// `None` only for a table [`check`] refuses — a variable or a mode that is
    /// not there, a missing cell, or an alias loop — so every caller holding a
    /// committed document can treat it as unreachable.
    pub fn resolve(&self, var: VarId, mode: ModeId) -> Option<Value> {
        self.resolve_at(var, &[mode])
    }

    /// [`Self::resolve`] with `None` for every collection's default — what a
    /// kit's variable reads in a document with no modes of its own.
    pub fn resolve_opt(&self, var: VarId, mode: Option<ModeId>) -> Option<Value> {
        self.resolve_at(var, mode.as_slice())
    }

    /// `var`'s value under `set`, the modes in force nearest first, its aliases
    /// followed — **each step reading its own collection's column**: an alias to
    /// another collection's variable reads the mode in force for that one. The
    /// walk is bounded by the variable count: a loop is refused at the commit,
    /// and a read that met one anyway must end.
    ///
    /// **A kit's variable reads the mode its walk was started in**: a walk from
    /// one of the document's own reads kits through the document's modes alone;
    /// a walk from a kit's reads every kit step in the one mode that chose its
    /// first column — that kit's own, or the document's it maps from — so a kit
    /// mode set on a frame reaches another kit only along an alias, by name.
    pub fn resolve_at(&self, var: VarId, set: &[ModeId]) -> Option<Value> {
        let from_local = self.vars.get(&var)?.kit().is_none();
        let mut kit_set: Option<Vec<ModeId>> = None;
        let mut at = var;
        for _ in 0..=self.vars.len() {
            let v = self.vars.get(&at)?;
            let col = match v.kit() {
                None => self.column_at(v, set)?,
                Some(kit) => {
                    let kc = self.kit_collection_of(v)?;
                    let ks = kit_set.get_or_insert_with(|| match from_local {
                        true => set
                            .iter()
                            .copied()
                            .filter(|m| self.mode(*m).is_some())
                            .collect(),
                        false => {
                            let own = |m: ModeId| {
                                self.kits
                                    .get(kit)
                                    .is_some_and(|k| k.modes_in(kc).any(|x| x.id == m))
                            };
                            let onto = self.kit_onto(kc);
                            set.iter()
                                .copied()
                                .find(|m| own(*m) || onto.is_some_and(|c| c.has(*m)))
                                .into_iter()
                                .collect()
                        }
                    });
                    self.kit_mode_at(kit, kc, ks)?
                }
            };
            match v.cells.get(&col)? {
                Cell::Color(c) => return Some(Value::Color(*c)),
                Cell::Number(n) => return Some(Value::Number(*n)),
                Cell::Boolean(b) => return Some(Value::Boolean(*b)),
                Cell::String(s) => return Some(Value::String(s.clone())),
                Cell::Alias(next) => at = *next,
            }
        }
        None
    }

    /// Every value `var` can take — each of its cells' values, and through each
    /// alias every value its target can take — each once. What a bound family
    /// asks the font service to load (§15 D1069): a frame set to another mode
    /// draws another family, and it must be there before it is switched to.
    pub fn every_value(&self, var: VarId) -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();
        let mut seen = vec![var];
        let mut stack = vec![var];
        while let Some(at) = stack.pop() {
            let Some(v) = self.vars.get(&at) else {
                continue;
            };
            for cell in v.cells.values() {
                let value = match cell {
                    Cell::Color(c) => Value::Color(*c),
                    Cell::Number(n) => Value::Number(*n),
                    Cell::Boolean(b) => Value::Boolean(*b),
                    Cell::String(s) => Value::String(s.clone()),
                    Cell::Alias(next) => {
                        if !seen.contains(next) {
                            seen.push(*next);
                            stack.push(*next);
                        }
                        continue;
                    }
                };
                if !out.contains(&value) {
                    out.push(value);
                }
            }
        }
        out
    }

    /// This table with `var`'s cell in `mode` set to `cell` — what a previewed
    /// `SetVariableCell` resolves against (§15 D1056), the document's own table
    /// left as committed. `None` when the variable or the mode is not here. A
    /// loop the cell would close resolves to nothing, as [`Self::resolve`]'s walk
    /// is bounded; the commit refuses it.
    pub fn with_cell(&self, var: VarId, mode: ModeId, cell: Cell) -> Option<Variables> {
        if !self.has_mode(mode) {
            return None;
        }
        let mut t = self.clone();
        t.vars.get_mut(&var)?.cells.insert(mode, cell);
        Some(t)
    }

    /// This table with `frame` read as `width` wide wherever it picks a mode by
    /// width — what a resize preview resolves its contents against, so the
    /// mode changes under the drag as the edge crosses a breakpoint.
    pub fn with_width(&self, frame: NodeId, width: f64) -> Variables {
        Variables {
            previewed_width: Some((frame, width)),
            ..self.clone()
        }
    }

    /// The kind `var` resolves to.
    pub fn kind_of(&self, var: VarId) -> Option<VarKind> {
        self.vars.get(&var).map(|v| v.kind)
    }

    /// Whether `var`'s cell in `mode` aliasing `target` would make a loop — the
    /// browser's dimmed rows (D1038 G). True when `target` is `var` or reaches it
    /// under some choice of modes with `mode` among them.
    ///
    /// **Exact across collections**: a step into a collection no step has read
    /// yet tries each of its modes, and a later step into it reads the one
    /// chosen, since a layer reads one mode per collection. Reading every column
    /// of every other collection instead would refuse chains no layer can walk.
    pub fn would_loop(&self, var: VarId, mode: ModeId, target: VarId) -> bool {
        let mut set = vec![mode];
        self.reaches(target, var, &mut set, 0)
    }

    fn reaches(&self, at: VarId, goal: VarId, set: &mut Vec<ModeId>, depth: usize) -> bool {
        if at == goal || depth > self.vars.len() {
            return true;
        }
        let Some(v) = self.vars.get(&at) else {
            return false;
        };
        let columns = self.columns(v);
        let next = |m: &ModeId| match v.cells.get(m) {
            Some(Cell::Alias(next)) => Some(*next),
            _ => None,
        };
        // A kit's variable is a collection of its own here: whichever document
        // mode is in force maps it onto one of its columns.
        if let Some(m) = columns.iter().find(|m| set.contains(m)) {
            return next(m).is_some_and(|n| self.reaches(n, goal, set, depth + 1));
        }
        columns.iter().any(|m| {
            let Some(n) = next(m) else { return false };
            set.push(*m);
            let hit = self.reaches(n, goal, set, depth + 1);
            set.pop();
            hit
        })
    }

    /// Every id in the table, for `reserve_existing_ids`.
    pub(crate) fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.collections
            .iter()
            .filter(|c| !c.id.is_first())
            .map(|c| c.id.0)
            .chain(self.modes().map(|m| m.id.0))
            .chain(self.vars.keys().map(|v| v.0))
            .chain(
                self.kits
                    .values()
                    .flat_map(|k| k.modes.iter().map(|m| m.id.0)),
            )
    }
}

/// What owns a mode: one of the document's collections, or a kit's collection
/// by kit id and collection name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeOwner<'a> {
    Collection(CollectionId),
    Kit(&'a str, &'a str),
}

impl Variables {
    /// What owns `mode`, `None` for a mode the table does not hold.
    pub fn owner_of(&self, mode: ModeId) -> Option<ModeOwner<'_>> {
        if let Some(c) = self.collection_of_mode(mode) {
            return Some(ModeOwner::Collection(c.id));
        }
        self.kit_mode_of(mode)
            .map(|(k, m)| ModeOwner::Kit(k, m.collection.as_str()))
    }
}

/// Whether modes `a` and `b` are of one collection — of the document's or a
/// kit's — so a frame can set only one of them.
pub fn same_collection(t: &Variables, a: ModeId, b: ModeId) -> bool {
    a == b || t.owner_of(a).is_some_and(|o| t.owner_of(b) == Some(o))
}

/// `set` — a frame's modes — with its mode of `owner` replaced by `mode`, or
/// taken out for `None`; in id order, as [`crate::Node::modes`] keeps them.
/// What one pick in a frame's mode row writes, with every other collection's
/// mode kept.
pub fn with_mode(
    t: &Variables,
    set: &[ModeId],
    owner: ModeOwner<'_>,
    mode: Option<ModeId>,
) -> Vec<ModeId> {
    let mut out: Vec<ModeId> = set
        .iter()
        .copied()
        .filter(|m| t.owner_of(*m) != Some(owner))
        .chain(mode)
        .collect();
    out.sort();
    out
}

/// What a fill or a stroke paints with: a brush of its own, or a **colour
/// variable** times the fill's `%` (§5.3f, §15 D1039) — D1037's
/// `Literal(T) | Bound(VarId)` for the one bindable field build step 1 has whose
/// binding carries something beside the variable.
///
/// **The `%` rides in the binding.** A colour variable carries its own alpha and
/// the fill's `%` *"stays a separate field, not bindable, and what draws is the
/// variable's colour times that `%`"* (D1038). A literal solid's `%` is its
/// colour's alpha — there is no separate field to stay — so a bound one keeps its
/// `%` here, written from the literal's alpha when it binds (`paint::alpha_of`'s
/// door) and back into the colour when it detaches.
///
/// **No `Deref`, on purpose**: a reader has to say which it wants. Drawing asks
/// [`crate::Document::resolve_brush`], against the mode in force at the node; an
/// editor asks [`BrushSource::literal`] or [`BrushSource::var`]. A reader that
/// took the literal and drew it would draw a bound fill as nothing, in silence —
/// the failure D1037 names, and the type is what makes the compiler list them.
///
/// Saved as the brush itself when literal, so a file with no bindings is the
/// bytes it was, and as `{"var": "<id>", "alpha": …}` when bound, `alpha` left out
/// at 1.
///
/// ⚠️ **Unboxed, and `large_enum_variant` allowed for it**: the literal is the
/// arm nearly every fill takes, and a box would put a heap allocation on each of
/// them to shrink the rare bound one — the opposite of the trade the lint is
/// written for.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum BrushSource {
    Literal(crate::Brush),
    Bound { var: VarId, alpha: f32 },
}

impl BrushSource {
    /// The brush of its own, `None` when bound.
    pub fn literal(&self) -> Option<&crate::Brush> {
        match self {
            BrushSource::Literal(b) => Some(b),
            BrushSource::Bound { .. } => None,
        }
    }
    /// [`Self::literal`], to edit in place — the crop mode's picture, a relink.
    pub fn literal_mut(&mut self) -> Option<&mut crate::Brush> {
        match self {
            BrushSource::Literal(b) => Some(b),
            BrushSource::Bound { .. } => None,
        }
    }
    /// The variable this paint is bound to.
    pub fn var(&self) -> Option<VarId> {
        match self {
            BrushSource::Literal(_) => None,
            BrushSource::Bound { var, .. } => Some(*var),
        }
    }
    pub fn is_bound(&self) -> bool {
        matches!(self, BrushSource::Bound { .. })
    }
    /// The brush **of a paint already resolved** — one
    /// [`crate::Document::resolved_paint`] returned, whose brushes are all
    /// literal. A bound one answers transparent, which is what a reader that
    /// skipped the resolve draws: in silence, D1037's failure, so call this only
    /// below a resolve.
    pub fn drawn(&self) -> &crate::Brush {
        static NONE: crate::Brush = crate::Brush::Solid(Color::TRANSPARENT);
        self.literal().unwrap_or(&NONE)
    }
    /// Whether every number in it is finite (invariant 8, §15 D451).
    pub fn is_finite(&self) -> bool {
        match self {
            BrushSource::Literal(b) => crate::build::brush_is_finite(b),
            BrushSource::Bound { alpha, .. } => alpha.is_finite(),
        }
    }
}

impl From<crate::Brush> for BrushSource {
    fn from(b: crate::Brush) -> Self {
        BrushSource::Literal(b)
    }
}

impl From<Color> for BrushSource {
    fn from(c: Color) -> Self {
        BrushSource::Literal(crate::Brush::Solid(c))
    }
}

fn alpha_is_one(a: &f32) -> bool {
    *a == 1.0
}

fn one() -> f32 {
    1.0
}

/// `BrushSource::Bound`'s saved form.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundRepr {
    var: VarId,
    #[serde(default = "one", skip_serializing_if = "alpha_is_one")]
    alpha: f32,
}

impl Serialize for BrushSource {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            BrushSource::Literal(b) => b.serialize(s),
            BrushSource::Bound { var, alpha } => BoundRepr {
                var: *var,
                alpha: *alpha,
            }
            .serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for BrushSource {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // `Bound` first: `{"var": …}` is no brush, and trying it first leaves a
        // malformed brush's error to the brush arm.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Bound(BoundRepr),
            Literal(crate::Brush),
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Bound(b) => BrushSource::Bound {
                var: b.var,
                alpha: b.alpha,
            },
            Repr::Literal(b) => BrushSource::Literal(b),
        })
    }
}

/// A rectangle's four corner bindings, in [`kurbo::RoundedRectRadii`]'s order —
/// top-left, top-right, bottom-right, bottom-left (§15 D1039).
///
/// **On the node rather than in `NodeKind::Rect`**, beside the literal radii it
/// stands in for. A bound corner's literal is written 0 by the verb that binds
/// one (`crate::build::bind_corners_ops`) and is never drawn: `Resolved`'s used kind
/// carries the resolved radius, and every geometry reader already reads that.
pub type CornerVars = [Option<VarId>; 4];

/// A layout's gap and padding bindings (§15 D1058), in
/// [`crate::container::Display::spacing`]'s order — column gap, row gap, then
/// padding top, right, bottom, left.
///
/// **On the node beside `display`, as [`CornerVars`] sits beside the radii**:
/// a bound slot's literal is written 0 by the verb that binds it
/// (`crate::build::bind_spacing_ops`) and never laid out — every layout view
/// reads the display through [`crate::Document::resolved_display`]. Inert on a
/// node with no layout; taking the layout away clears it
/// (`crate::build::keep_spacing_bindings`).
pub type SpacingVars = [Option<VarId>; 6];

/// A layer's width and height bindings, in that order (§15 D1061).
///
/// **On the node beside the kind's `size`, as [`CornerVars`] sits beside the
/// radii**: a bound axis's literal is written 0 by the verb that binds it
/// (`crate::build::bind_size_ops`), so a main and a copy bound alike compare
/// equal, and is never drawn — every layout view and `Resolved`'s used kind read
/// the kind through [`crate::Document::resolved_kind`]. Only the five kinds with a
/// stored `size` bind, and only on an axis whose size the user sets
/// (`crate::build::size_axes_authored`); an axis something else comes to size
/// is detached (`crate::build::keep_size_bindings`).
pub type SizeVars = [Option<VarId>; 2];

/// A layout item's size-limit bindings — min width, max width, min height, max
/// height — held in the item itself (`crate::container::LayoutItem::limit_vars`).
///
/// A bound limit is px: its variable's number, never below 0, and its literal
/// `Px(0)` (`crate::build::bind_limit_ops`). An item write keeps a binding that
/// writes the literal or the number drawn and detaches any other value
/// (`crate::build::keep_limit_bindings`).
pub type LimitVars = [Option<VarId>; 4];

/// A text layer's bindings, slot by slot ([`TEXT_CONTENT`] … [`TEXT_PARAGRAPH_SPACING`]):
/// the content and family to string variables, the size, weight,
/// line height, letter spacing and paragraph spacing to number variables.
///
/// **On the node beside the kind, as [`SizeVars`] sits beside the size**: a
/// bound field's literal is written empty or 0 by the verb that binds it
/// (`crate::build::bind_text_ops`) — the content with **no spans and no
/// paragraph spans**, since a byte range cannot index a string whose length
/// changes with the mode, and a default with no runs of its own over it — so a
/// main and a copy bound alike compare equal, and is never drawn: every reader
/// that shapes, draws or exports the text reads the kind through
/// [`crate::Document::resolved_kind`]. Only a `Text` binds.
///
/// **The content binding is the text, whole**: an edit of the text — on the
/// canvas, through a component's text property, or a range styled — detaches it
/// to what was written (`crate::build::keep_text_bindings`). **Every other
/// binding is the node's default**, as a text style's values are: a
/// run or paragraph given a value of its own keeps it, and applying a text
/// style, which holds plain values, detaches the slots it holds
/// ([`TEXT_STYLED`]). A number binding reads its variable as the number the
/// field shows, in the literal's unit: a line height, letter spacing or
/// paragraph spacing in `%` is that many hundredths of an em.
pub type TextVars = [Option<VarId>; 7];

/// [`TextVars`]' slots.
pub const TEXT_CONTENT: usize = 0;
pub const TEXT_FAMILY: usize = 1;
pub const TEXT_SIZE: usize = 2;
pub const TEXT_WEIGHT: usize = 3;
pub const TEXT_LINE_HEIGHT: usize = 4;
pub const TEXT_LETTER_SPACING: usize = 5;
pub const TEXT_PARAGRAPH_SPACING: usize = 6;

/// The [`TextVars`] slots a text style holds — the character defaults — which
/// applying one detaches.
pub const TEXT_STYLED: std::ops::RangeInclusive<usize> = TEXT_FAMILY..=TEXT_LETTER_SPACING;

/// [`TextVars`] binding slot `k` to `var` and nothing else — what
/// `crate::build::bind_text_ops` binds one field with.
pub fn text_vars_at(k: usize, var: VarId) -> TextVars {
    let mut vars = TextVars::default();
    vars[k] = Some(var);
    vars
}

/// [`TextVars`]' slot `k` alone, as `crate::build::detach_text_ops` names slots.
pub fn text_slot_mask(k: usize) -> [bool; 7] {
    std::array::from_fn(|i| i == k)
}

/// Whether `vars` binds any slot a text style holds ([`TEXT_STYLED`]).
pub fn binds_styled(vars: &TextVars) -> bool {
    vars[TEXT_STYLED].iter().any(Option::is_some)
}

/// The kind of variable [`TextVars`] slot `k` binds to.
pub fn text_slot_kind(k: usize) -> VarKind {
    text_slot_field(k).kind()
}

/// The field family [`TextVars`] slot `k` is of.
pub fn text_slot_field(k: usize) -> Field {
    match k {
        TEXT_CONTENT => Field::Content,
        TEXT_FAMILY => Field::Family,
        TEXT_WEIGHT => Field::Weight,
        _ => Field::Type,
    }
}

/// The character attribute a number slot of [`TextVars`] is the default of;
/// `None` for the content, the family and the paragraph spacing.
pub fn text_slot_attr(k: usize) -> Option<crate::typography::CharAttrKind> {
    use crate::typography::CharAttrKind as K;
    match k {
        TEXT_SIZE => Some(K::Size),
        TEXT_WEIGHT => Some(K::Weight),
        TEXT_LINE_HEIGHT => Some(K::LineHeight),
        TEXT_LETTER_SPACING => Some(K::LetterSpacing),
        _ => None,
    }
}

/// Number slot `k` of a text's style and paragraph as its field shows it — a
/// size, a weight, px, or an em length ×100. `None` for a string slot, and for
/// an Auto line height.
pub fn text_slot_number(
    k: usize,
    style: &crate::typography::TextStyle,
    paragraph: &crate::typography::ParagraphStyle,
) -> Option<f64> {
    let shown = |l: crate::typography::Length| match l {
        crate::typography::Length::Px(v) => v,
        crate::typography::Length::Em(m) => m * 100.0,
    };
    match k {
        TEXT_SIZE => Some(style.font_size),
        TEXT_WEIGHT => Some(f64::from(style.weight)),
        TEXT_LINE_HEIGHT => style.line_height.map(shown),
        TEXT_LETTER_SPACING => Some(shown(style.letter_spacing)),
        TEXT_PARAGRAPH_SPACING => Some(shown(paragraph.spacing)),
        _ => None,
    }
}

/// Write `x` — a number as [`text_slot_number`] reads it — into number slot `k`
/// of a text's style or paragraph, in the unit the slot holds (px for an Auto
/// line height). A size is floored at `MIN_FONT_SIZE` and a weight kept to
/// 1–1000, the ranges the fields allow.
pub fn set_text_slot_number(
    k: usize,
    style: &mut crate::typography::TextStyle,
    paragraph: &mut crate::typography::ParagraphStyle,
    x: f64,
) {
    use crate::typography::Length;
    let like = |l: Option<Length>, x: f64| match l {
        Some(Length::Em(_)) => Length::Em(x / 100.0).canonical(),
        _ => Length::Px(x).canonical(),
    };
    match k {
        TEXT_SIZE => style.font_size = x.max(crate::typography::MIN_FONT_SIZE),
        TEXT_WEIGHT => style.weight = x.round().clamp(1.0, 1000.0) as u16,
        // A line is never less than no height.
        TEXT_LINE_HEIGHT => style.line_height = Some(like(style.line_height, x.max(0.0))),
        TEXT_LETTER_SPACING => style.letter_spacing = like(Some(style.letter_spacing), x),
        TEXT_PARAGRAPH_SPACING => paragraph.spacing = like(Some(paragraph.spacing), x),
        _ => {}
    }
}

/// Number slot `k`'s literal while bound: 0 in the slot's unit (a weight 0), so
/// a main and a copy bound alike compare equal.
pub fn clear_text_slot_number(
    k: usize,
    style: &mut crate::typography::TextStyle,
    paragraph: &mut crate::typography::ParagraphStyle,
) {
    use crate::typography::Length;
    let zero = |l: Option<Length>| match l {
        Some(Length::Em(_)) => Length::Em(0.0),
        _ => Length::Px(0.0),
    };
    match k {
        TEXT_SIZE => style.font_size = 0.0,
        TEXT_WEIGHT => style.weight = 0,
        TEXT_LINE_HEIGHT => style.line_height = Some(zero(style.line_height)),
        TEXT_LETTER_SPACING => style.letter_spacing = zero(Some(style.letter_spacing)),
        TEXT_PARAGRAPH_SPACING => paragraph.spacing = zero(Some(paragraph.spacing)),
        _ => {}
    }
}

/// `style` and `paragraph` with each slot `vars` binds at its literal: the
/// defaults a text's span lists are kept against while it is edited. A run equal
/// to a bound default's drawn value is a value of its own, since the variable
/// draws another in another mode, so it must not be normalized away against
/// what is drawn.
pub fn span_defaults(
    vars: TextVars,
    style: &crate::typography::TextStyle,
    paragraph: &crate::typography::ParagraphStyle,
) -> (
    crate::typography::TextStyle,
    crate::typography::ParagraphStyle,
) {
    let (mut style, mut paragraph) = (style.clone(), paragraph.clone());
    if vars[TEXT_FAMILY].is_some() {
        style.font_family.clear();
    }
    for (k, var) in vars.iter().enumerate().skip(TEXT_SIZE) {
        if var.is_some() {
            clear_text_slot_number(k, &mut style, &mut paragraph);
        }
    }
    (style, paragraph)
}

/// Whether `a` and `b` (each a text's style and paragraph) hold the same value
/// in slot `k` — a number with its unit, so a line height moved from px to % is
/// another value. False for the content, which neither holds.
pub fn text_slot_eq(
    k: usize,
    a: (
        &crate::typography::TextStyle,
        &crate::typography::ParagraphStyle,
    ),
    b: (
        &crate::typography::TextStyle,
        &crate::typography::ParagraphStyle,
    ),
) -> bool {
    match k {
        TEXT_FAMILY => a.0.font_family == b.0.font_family,
        TEXT_SIZE => a.0.font_size == b.0.font_size,
        TEXT_WEIGHT => a.0.weight == b.0.weight,
        TEXT_LINE_HEIGHT => a.0.line_height == b.0.line_height,
        TEXT_LETTER_SPACING => a.0.letter_spacing == b.0.letter_spacing,
        TEXT_PARAGRAPH_SPACING => a.1.spacing == b.1.spacing,
        _ => false,
    }
}

/// Copy slot `k`'s literal — the content, the family or a number — from text
/// kind `from` to text kind `to`; nothing when either is not a text.
pub fn copy_text_slot(k: usize, from: &crate::NodeKind, to: &mut crate::NodeKind) {
    let (
        crate::NodeKind::Text {
            content: fc,
            style: fs,
            paragraph: fp,
            ..
        },
        crate::NodeKind::Text {
            content,
            style,
            paragraph,
            ..
        },
    ) = (from, to)
    else {
        return;
    };
    match k {
        TEXT_CONTENT => *content = fc.clone(),
        TEXT_FAMILY => style.font_family = fs.font_family.clone(),
        TEXT_SIZE => style.font_size = fs.font_size,
        TEXT_WEIGHT => style.weight = fs.weight,
        TEXT_LINE_HEIGHT => style.line_height = fs.line_height,
        TEXT_LETTER_SPACING => style.letter_spacing = fs.letter_spacing,
        TEXT_PARAGRAPH_SPACING => paragraph.spacing = fp.spacing,
        _ => {}
    }
}

/// [`TextVars`] as the file holds it: the slots up to the last bound one, at
/// least the two the format first had, so a file written before the number
/// slots loads and one binding only strings is written as it was.
pub(crate) mod text_vars_serde {
    use super::{TextVars, VarId};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &TextVars, s: S) -> Result<S::Ok, S::Error> {
        let len = v
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |i| i + 1)
            .max(2);
        v[..len].serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<TextVars, D::Error> {
        let got = Vec::<Option<VarId>>::deserialize(d)?;
        let mut out = TextVars::default();
        if got.len() > out.len() {
            return Err(serde::de::Error::invalid_length(
                got.len(),
                &"at most 7 slots",
            ));
        }
        out[..got.len()].copy_from_slice(&got);
        Ok(out)
    }
}

/// `node` — a node of `doc`, or a copy of one still carrying its id — with every
/// binding **detached to the value it draws** in `doc` (D1038's detach): a bound
/// paint becomes a solid of its variable's colour times its `%`, a bound corner,
/// gap or padding its number (§15 D1058), a bound visibility what it shows (§15
/// D1059), a bound content or family its string (§15 D1069), and a frame's mode
/// is dropped.
///
/// For a node leaving the document whose table it reads, into one that has none
/// of it: a paste into another document with no variable of the name. (A kit's
/// frozen copy was the other until step 4: it carries its table now, §15 D1064.)
pub fn detached(doc: &crate::Document, mut node: crate::Node) -> crate::Node {
    let id = node.id;
    for f in node.paint.fills.iter_mut() {
        if f.brush.is_bound() {
            f.brush = doc.resolve_brush(id, &f.brush).into_owned().into();
        }
    }
    for s in node.paint.strokes.iter_mut() {
        if s.brush.is_bound() {
            s.brush = doc.resolve_brush(id, &s.brush).into_owned().into();
        }
        // A bound width its number (§15 D1073).
        if let Some(v) = s.width_var.take() {
            s.width = doc.stroke_width_drawn_in(doc.variables(), id, v);
        }
    }
    // And an effect's bound numbers theirs, and a size limit its px.
    node.effects = doc.resolved_effects(id, &node.effects).into_owned();
    node.item = doc.limits_drawn_in(doc.variables(), id, node.item);
    node.item.limit_vars = Default::default();
    if let Some(corner_radii) = node.kind.corner_radii()
        && let Some(drawn) = doc.resolve_corners(id, corner_radii)
    {
        node.kind.set_corner_radii(drawn);
    }
    node.corner_vars = Default::default();
    if let Some(d) = &node.display {
        let mut drawn = doc.spacing_drawn_in(doc.variables(), id, d, node.spacing_vars);
        if let crate::container::Display::Flex(f) = &mut drawn {
            (f.direction_var, f.wrap_var) = (None, None);
        }
        node.display = Some(drawn);
    }
    node.spacing_vars = Default::default();
    node.kind = doc.size_drawn_in(doc.variables(), id, &node.kind, node.size_vars);
    node.size_vars = Default::default();
    node.kind = doc.text_drawn_in(doc.variables(), id, &node.kind, node.text_vars);
    node.text_vars = Default::default();
    if node.visible_var.is_some() {
        node.visible = doc.shown(id);
        node.visible_var = None;
    }
    // A bound opacity its percentage (§15 D1072).
    if node.opacity_var.is_some() {
        node.opacity = doc.drawn_opacity(id);
        node.opacity_var = None;
    }
    node.modes = Vec::new();
    // And its styles, its lists already holding their values (§15 D1062).
    node.styles = Default::default();
    node
}

/// One variable a copy's layers read, as the copy knows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarriedVar {
    pub id: VarId,
    pub name: String,
    pub kind: VarKind,
    /// A kit's variable's origin (§15 D1064), which a paste rebinds by: the
    /// target's variable of the same origin, never one of the same name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<VarOrigin>,
    /// The name of the document collection it was in — a paste prefers the
    /// namesake in the collection of the same name. `None` for a kit's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
}

/// One mode a copied frame sets, with the name of its collection to map by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarriedMode {
    pub id: ModeId,
    pub name: String,
    #[serde(default)]
    pub collection: String,
}

/// What a copy carries about the variables its layers read (§15 D1039) — enough
/// for a paste to follow D1038's rule: *"rebinds to a variable of the same name
/// and type there, else detaches to the resolved value; within one document a
/// paste keeps its binding"*.
///
/// The names and kinds, to rebind by; the modes the copied frames set, to map by
/// name; and a **detached twin** of every copied layer that reads a variable,
/// taken at the copy against the source document — the value to fall back to,
/// which only the source can resolve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Carried {
    pub vars: Vec<CarriedVar>,
    pub modes: Vec<CarriedMode>,
    pub detached: Vec<crate::Node>,
    /// The styles the copied layers are bound to, by name and slot, for the
    /// same rule (§15 D1062, `crate::style::rebind`). A layer's own lists
    /// already hold its style's values, so no twin is needed to detach one.
    pub styles: Vec<crate::style::CarriedStyle>,
    /// The kits' variables and styles the copy reads, whole — its layers' and
    /// the vault mains' it carries (§15 D1064, `crate::tokens::carried`) — which
    /// a paste into a document holding none of them vendors with the mains.
    pub tokens: crate::tokens::Tokens,
}

impl Carried {
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
            && self.modes.is_empty()
            && self.detached.is_empty()
            && self.styles.is_empty()
            && self.tokens.is_empty()
    }
}

/// [`Carried`] for a copy of `subtrees`, taken from `doc` while the originals are
/// in it.
pub fn carried(doc: &crate::Document, subtrees: &[Vec<crate::Node>]) -> Carried {
    let mut out = Carried::default();
    for n in subtrees.iter().flatten() {
        let vars = n
            .paint
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
            .chain(n.flow_vars().into_iter().flatten())
            .chain(n.size_vars.iter().flatten().copied())
            .chain(n.text_vars.iter().flatten().copied())
            .chain(n.item.limit_vars.iter().flatten().copied())
            .chain(n.visible_var)
            .chain(n.opacity_var);
        for v in vars {
            if out.vars.iter().any(|c| c.id == v) {
                continue;
            }
            if let Some(var) = doc.variables().get(v) {
                out.vars.push(CarriedVar {
                    id: v,
                    name: var.name.clone(),
                    kind: var.kind,
                    origin: var.origin.clone(),
                    collection: doc.variables().collection_of(v).map(|c| c.name.clone()),
                });
            }
        }
        for &m in &n.modes {
            if !out.modes.iter().any(|c| c.id == m)
                && let Some(c) = doc.variables().collection_of_mode(m)
                && let Some(mode) = c.modes.iter().find(|x| x.id == m)
            {
                out.modes.push(CarriedMode {
                    id: m,
                    name: mode.name.clone(),
                    collection: c.name.clone(),
                });
            }
        }
        if n.reads_variables() {
            out.detached.push(detached(doc, n.clone()));
        }
        for (slot, s) in n.styles.iter() {
            if out.styles.iter().any(|c| c.id == s) {
                continue;
            }
            if let Some(style) = doc.styles().get(s) {
                out.styles.push(crate::style::CarriedStyle {
                    id: s,
                    name: style.name.clone(),
                    slot,
                    origin: style.origin.clone(),
                });
            }
        }
    }
    let kits = crate::kit::carried(doc, subtrees);
    out.tokens = crate::tokens::carried(doc, subtrees.iter().chain(&kits).flatten());
    out
}

/// D1038's paste rule for one binding of `field`, landing in table `t`: `v`
/// itself where `t` has it at the kind the copy knew, else `t`'s variable of the
/// same name and kind — in the collection of the same name first — else `None`,
/// the caller detaching. Either only where its scope takes `field`. Shared by
/// [`land`] and a property paste (`build::paste_properties`, §15 D1047), so the
/// two cannot drift.
///
/// **A kit's variable rebinds by its origin, not its name** (§15 D1064): to the
/// target's copy of the same kit's same variable, else nothing — a kit's
/// *brand/500* is not the document's *brand/500*, the namespaces being two.
pub fn rebind(t: &Variables, carried: &[CarriedVar], v: VarId, field: Field) -> Option<VarId> {
    let known = carried.iter().find(|c| c.id == v);
    if let Some(var) = t.get(v)
        && known.is_none_or(|c| c.kind == var.kind)
    {
        return var.binds(field).then_some(v);
    }
    let c = known?;
    let found = match &c.origin {
        Some(o) => t.by_origin(o.key()),
        None => c
            .collection
            .as_deref()
            .and_then(|name| t.collections().iter().find(|x| x.name == name))
            .and_then(|x| t.by_name_in(x.id, &c.name))
            .or_else(|| t.by_name(&c.name)),
    };
    found
        .filter(|var| var.kind == c.kind && var.binds(field))
        .map(|var| var.id)
}

/// `nodes` — copied layers carrying `carried` — rebound for a paste into `doc`
/// (D1038): each binding kept where `doc` has the variable itself (a paste into
/// the document it came from), else rebound to `doc`'s variable of the **same
/// name and kind**, else **detached** to the value its detached twin drew at the
/// copy. A frame's mode the same way, by name, else *Inherit*.
///
/// Per field rather than per layer: a fill whose variable has a namesake here
/// keeps a binding beside a corner whose variable has none.
pub fn land(doc: &crate::Document, carried: &Carried, nodes: Vec<crate::Node>) -> Vec<crate::Node> {
    let t = doc.variables();
    // A kit's variable the target lacks and the copy carries is **kept**, to be
    // vendored with the mains and pointed at its copy (`crate::kit::insert_carrying`,
    // §15 D1064) — an instance's own fields read its main's, and detaching them
    // would make every one an override.
    //
    // 🚨 **Into the kit itself, the kit's own variable or nothing** (D1023's
    // own-kit arm, `crate::tokens::plan`'s): a carried variable of `doc`'s own
    // kit lands on the original, and one the kit has since deleted detaches
    // here, by D1038's rule — kept for the plan, it would have nothing to be
    // pointed at and the commit would refuse the paste.
    let own = doc.meta().id.as_deref();
    let own_var = |v: VarId| {
        let o = carried.vars.iter().find(|c| c.id == v)?.origin.as_ref()?;
        (Some(o.kit.as_str()) == own).then_some(o.var)
    };
    let rebind = |v: VarId, field: Field| match own_var(v) {
        Some(orig) => t
            .get(orig)
            .filter(|x| x.origin.is_none() && x.binds(field))
            .map(|x| x.id),
        None => {
            rebind(t, &carried.vars, v, field).or_else(|| carried.tokens.holds_var(v).then_some(v))
        }
    };
    let own_style = |s: crate::style::StyleId| {
        let o = carried.styles.iter().find(|c| c.id == s)?.origin.as_ref()?;
        (Some(o.kit.as_str()) == own).then_some(o.style)
    };
    nodes
        .into_iter()
        .map(|mut n| {
            // A card's style by the same rule, by name and slot, else detached —
            // the layer's list already holds the values it was copied with
            // (§15 D1062). Rebound to another document's namesake, the commit
            // writes that style's values onto it (`style::settle`). A kit's, by
            // its origin, or kept to be vendored, as a kit's variable is.
            let refs = n.styles;
            for (slot, s) in refs.iter() {
                let to = match own_style(s) {
                    Some(orig) => doc
                        .styles()
                        .get(orig)
                        .filter(|x| x.origin.is_none() && x.slot() == slot)
                        .map(|x| x.id),
                    None => crate::style::rebind(doc.styles(), &carried.styles, slot, s)
                        .or_else(|| carried.tokens.holds_style(s).then_some(s)),
                };
                n.styles.set(slot, to);
            }
            if !n.reads_variables() && n.modes.is_empty() {
                return n;
            }
            let twin = carried.detached.iter().find(|d| d.id == n.id);
            let twin_brush = |fills: bool, i: usize| -> crate::Brush {
                let list = |d: &crate::Node| {
                    if fills {
                        d.paint.fills.get(i).map(|f| f.brush.clone())
                    } else {
                        d.paint.strokes.get(i).map(|s| s.brush.clone())
                    }
                };
                twin.and_then(list)
                    .and_then(|b| b.literal().cloned())
                    .unwrap_or(crate::Brush::Solid(Color::TRANSPARENT))
            };
            for (i, f) in n.paint.fills.iter_mut().enumerate() {
                if let BrushSource::Bound { var, alpha } = f.brush {
                    f.brush = match rebind(var, Field::Fill) {
                        Some(var) => BrushSource::Bound { var, alpha },
                        None => twin_brush(true, i).into(),
                    };
                }
            }
            for (i, s) in n.paint.strokes.iter_mut().enumerate() {
                if let BrushSource::Bound { var, alpha } = s.brush {
                    s.brush = match rebind(var, Field::Stroke) {
                        Some(var) => BrushSource::Bound { var, alpha },
                        None => twin_brush(false, i).into(),
                    };
                }
                // A bound width the same way, its fallback the twin's (§15 D1073).
                if let Some(v) = s.width_var {
                    s.width_var = rebind(v, Field::StrokeWidth);
                    if s.width_var.is_none() {
                        s.width = twin
                            .and_then(|d| d.paint.strokes.get(i))
                            .map_or(s.width, |t| t.width);
                    }
                }
            }
            // An effect's bound numbers the same way, each fallback the twin's
            // number in that slot (§15 D1074).
            for (i, e) in n.effects.iter_mut().enumerate() {
                for slot in 0..4 {
                    if let Some(v) = e.vars[slot] {
                        e.vars[slot] = rebind(v, Field::Effect);
                        if e.vars[slot].is_none()
                            && let Some(x) = twin
                                .and_then(|d| d.effects.get(i))
                                .and_then(|t| t.numbers()[slot])
                        {
                            e.value.set_number(slot, x);
                        }
                    }
                }
            }
            // A size limit's binding the same way, its fallback the twin's px.
            for k in 0..n.item.limit_vars.len() {
                if let Some(v) = n.item.limit_vars[k] {
                    n.item.limit_vars[k] = rebind(v, Field::Limits);
                    if n.item.limit_vars[k].is_none()
                        && let Some(t) = twin
                    {
                        *n.item.limit_mut(k) = t.item.limit(k);
                    }
                }
            }
            if n.corner_vars.iter().any(Option::is_some) {
                let drawn = twin.and_then(|d| d.kind.corner_radii());
                let mut vars = n.corner_vars;
                if let Some(mut corner_radii) = n.kind.corner_radii() {
                    let slots: [&mut f64; 4] = [
                        &mut corner_radii.top_left,
                        &mut corner_radii.top_right,
                        &mut corner_radii.bottom_right,
                        &mut corner_radii.bottom_left,
                    ];
                    let fallback =
                        drawn.map(|r| [r.top_left, r.top_right, r.bottom_right, r.bottom_left]);
                    for (k, slot) in slots.into_iter().enumerate() {
                        if let Some(v) = vars[k] {
                            vars[k] = rebind(v, Field::Corner);
                            if vars[k].is_none() {
                                *slot = fallback.map_or(*slot, |f| f[k]);
                            }
                        }
                    }
                    n.kind.set_corner_radii(corner_radii);
                }
                n.corner_vars = vars;
            }
            // A gap's or a padding's binding the same way, slot by slot, its
            // fallback the twin's laid-out number (§15 D1058).
            if n.spacing_vars.iter().any(Option::is_some)
                && let Some(d) = n.display.as_mut()
            {
                let fallback = twin.and_then(|t| t.display.as_ref()).map(|t| t.spacing());
                let mut s = d.spacing();
                for (k, slot) in n.spacing_vars.iter_mut().enumerate() {
                    if let Some(v) = *slot {
                        *slot = rebind(v, Field::Spacing);
                        if slot.is_none() {
                            s[k] = fallback.map_or(s[k], |f| f[k]);
                        }
                    }
                }
                d.set_spacing(s);
            } else if n.display.is_none() {
                // Inert with no layout: a rebind has nothing to fall back to.
                for slot in &mut n.spacing_vars {
                    if let Some(v) = *slot {
                        *slot = rebind(v, Field::Spacing);
                    }
                }
            }
            // A flex direction's or wrap's binding the same way, its fallback
            // the twin's drawn one.
            if let Some(crate::container::Display::Flex(f)) = n.display.as_mut() {
                let drawn = match twin.and_then(|t| t.display.as_ref()) {
                    Some(crate::container::Display::Flex(t)) => Some(*t),
                    _ => None,
                };
                if let Some(v) = f.direction_var {
                    f.direction_var = rebind(v, Field::FlexDirection);
                    if f.direction_var.is_none() {
                        f.direction = drawn.map_or(f.direction, |t| t.direction);
                    }
                }
                if let Some(v) = f.wrap_var {
                    f.wrap_var = rebind(v, Field::FlexWrap);
                    if f.wrap_var.is_none() {
                        f.wrap = drawn.map_or(f.wrap, |t| t.wrap);
                    }
                }
            }
            // A width's or a height's binding the same way, axis by axis, its
            // fallback the twin's drawn size (§15 D1061).
            if n.size_vars.iter().any(Option::is_some) {
                let fallback = twin.and_then(|t| t.kind.box_size());
                let mut vars = n.size_vars;
                if let Some(size) = n.kind.box_size_mut() {
                    let slots: [&mut f64; 2] = [&mut size.width, &mut size.height];
                    for (k, slot) in slots.into_iter().enumerate() {
                        if let Some(v) = vars[k] {
                            vars[k] = rebind(v, Field::Size);
                            if vars[k].is_none() {
                                *slot = fallback.map_or(*slot, |f| [f.width, f.height][k]);
                            }
                        }
                    }
                }
                n.size_vars = vars;
            }
            // A text's bindings the same way, slot by slot, its fallback the
            // twin's drawn string or number.
            if n.text_vars.iter().any(Option::is_some) {
                let mut vars = n.text_vars;
                for (k, var) in vars.iter_mut().enumerate() {
                    if let Some(v) = *var {
                        *var = rebind(v, text_slot_field(k));
                        if var.is_none()
                            && let Some(t) = twin
                        {
                            copy_text_slot(k, &t.kind, &mut n.kind);
                        }
                    }
                }
                n.text_vars = vars;
            }
            // A visibility's binding the same way, its fallback whether the twin
            // was shown (§15 D1059).
            if let Some(v) = n.visible_var {
                n.visible_var = rebind(v, Field::Visible);
                if n.visible_var.is_none() {
                    n.visible = twin.map_or(n.visible, |t| t.visible);
                }
            }
            // And an opacity's, its fallback the twin's percentage (§15 D1072).
            if let Some(v) = n.opacity_var {
                n.opacity_var = rebind(v, Field::Opacity);
                if n.opacity_var.is_none() {
                    n.opacity = twin.map_or(n.opacity, |t| t.opacity);
                }
            }
            // A kit's mode the copy carries is kept, as its variables are — but
            // one of `doc`'s own kit is that mode itself, or *Inherit*. A mode
            // `doc` lacks maps to its namesake in the collection of the same
            // name, else in the first, else *Inherit*.
            let mut modes: Vec<ModeId> = Vec::new();
            for &m in &n.modes {
                let carried_kit = carried
                    .tokens
                    .kits
                    .iter()
                    .find_map(|(k, km)| km.mode(m).map(|x| (k.as_str(), x.from)));
                let to = match carried_kit {
                    Some((k, from)) if Some(k) == own => t.mode(from).map(|x| x.id),
                    Some(_) => Some(m),
                    None if t.has_mode(m) => Some(m),
                    None => carried.modes.iter().find(|c| c.id == m).and_then(|c| {
                        t.collections()
                            .iter()
                            .find(|x| x.name == c.collection)
                            .unwrap_or(t.first_collection())
                            .modes
                            .iter()
                            .find(|here| here.name == c.name)
                            .map(|here| here.id)
                    }),
                };
                if let Some(to) = to
                    && !modes.iter().any(|x| same_collection(t, *x, to))
                {
                    modes.push(to);
                }
            }
            modes.sort();
            n.modes = modes;
            n
        })
        .collect()
}

/// Every layer whose own fields read `var` — the browser's usage count and its
/// *Select n layers* (D1038 G), in id order. Not through an alias: a layer bound
/// to *button/bg* uses *button/bg*, whatever that names.
pub fn users(doc: &crate::Document, var: VarId) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| reads(n, &[var]))
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// Whether any field of `n` is bound to one of `vars`.
fn reads(n: &crate::Node, vars: &[VarId]) -> bool {
    let hit = |v: Option<VarId>| v.is_some_and(|v| vars.contains(&v));
    n.paint.fills.iter().any(|f| hit(f.brush.var()))
        || n.paint
            .strokes
            .iter()
            .any(|s| hit(s.brush.var()) || hit(s.width_var))
        || n.effects.iter().any(|e| e.vars.iter().any(|v| hit(*v)))
        || n.item.limit_vars.iter().any(|v| hit(*v))
        || n.corner_vars.iter().any(|v| hit(*v))
        || n.spacing_vars.iter().any(|v| hit(*v))
        || n.flow_vars().iter().any(|v| hit(*v))
        || n.size_vars.iter().any(|v| hit(*v))
        || n.text_vars.iter().any(|v| hit(*v))
        || hit(n.visible_var)
        || hit(n.opacity_var)
}

/// The operations that delete `vars` (D1038): **every field bound to one detaches
/// to the value it drew**, every other variable's alias to one becomes the value
/// it resolved to in that mode, and then the variables go — one transaction, so
/// one `Ctrl+Z` brings back the variables and every binding. The commit refuses
/// a bare `RemoveVariable` of a variable in use (`check`), so this is the only
/// shape a delete of one can take.
pub fn remove_ops(doc: &crate::Document, vars: &[VarId]) -> Vec<crate::Operation> {
    remove_and_scope_ops(doc, vars, &[])
}

/// Whether a binding of family `f` detaches when its variable takes `scope`:
/// one the scope leaves out, and for `None` (every field) one that needs a
/// scope of its own.
pub fn scope_drops(scope: Option<Scope>, f: Field) -> bool {
    scope.map_or(f.needs_scope(), |s| !s.contains(f))
}

/// [`remove_ops`] for `removed`, and every `scoped` variable's fields of a
/// family its new scope drops detached, **in one detach pass** — two passes
/// would each write a node's whole list against the committed document, and
/// the later would bring back what the earlier took off. The scopes
/// themselves are the caller's to write (a `SetVariable`), after these.
pub fn remove_and_scope_ops(
    doc: &crate::Document,
    removed: &[VarId],
    scoped: &[(VarId, Option<Scope>)],
) -> Vec<crate::Operation> {
    use crate::Operation as O;
    let t = doc.variables();
    let gone = |v: VarId, f: Field| {
        removed.contains(&v)
            || scoped
                .iter()
                .any(|(s, scope)| *s == v && scope_drops(*scope, f))
    };
    let mut ops = detach_ops(doc, &gone);
    let vars = removed;
    for v in t.iter().filter(|v| !vars.contains(&v.id)) {
        for (mode, cell) in &v.cells {
            if let Cell::Alias(target) = cell
                && vars.contains(target)
            {
                let Some(cell) = t.resolve(v.id, *mode).map(Cell::from) else {
                    continue;
                };
                ops.push(O::SetVariableCell {
                    id: v.id,
                    mode: *mode,
                    cell,
                });
            }
        }
    }
    ops.extend(vars.iter().map(|id| O::RemoveVariable { id: *id }));
    ops
}

/// The operations narrowing `var`'s scope to `scope` (`None` every field):
/// every field of a family it leaves that is bound to `var` detaches to the
/// value it drew, then the variable takes the scope — one transaction, so a
/// bound field never sits outside its variable's scope. A string scoped to flex
/// directions or wraps alone has each cell that names none set to the first
/// family's first word ([`holds_its_words`]), and another variable aliasing it
/// in that mode takes the value it read, so its own users keep their text.
/// Empty when `var` is not the document's own.
pub fn scope_ops(doc: &crate::Document, var: VarId, scope: Option<Scope>) -> Vec<crate::Operation> {
    let t = doc.variables();
    let Some(v) = t.get(var).filter(|v| v.origin.is_none()) else {
        return Vec::new();
    };
    let mut ops = remove_and_scope_ops(doc, &[], &[(var, scope)]);
    let mut next = Variable { scope, ..v.clone() };
    if !holds_its_words(&next)
        && let Some(word) = scope
            .and_then(|s| s.fields().next())
            .and_then(|f| f.words().first().copied())
    {
        for (mode, cell) in next.cells.iter_mut() {
            if let Cell::String(s) = cell
                && !scope.is_some_and(|sc| sc.fields().any(|f| f.takes(s)))
            {
                *cell = Cell::String(word.into());
                for u in t.iter().filter(|u| u.id != var) {
                    if u.cells.get(mode) == Some(&Cell::Alias(var))
                        && let Some(kept) = t.resolve(u.id, *mode).map(Cell::from)
                    {
                        ops.push(crate::Operation::SetVariableCell {
                            id: u.id,
                            mode: *mode,
                            cell: kept,
                        });
                    }
                }
            }
        }
    }
    ops.push(crate::Operation::SetVariable { var: next });
    ops
}

/// The layers bound to `var` in each family of its kind, in [`Field::ALL`]'s
/// order — the counts beside a scope's boxes. A style's binding counts as its
/// users'.
pub fn users_by_field(doc: &crate::Document, var: VarId) -> Vec<(Field, Vec<NodeId>)> {
    let Some(kind) = doc.variables().kind_of(var) else {
        return Vec::new();
    };
    Field::of_kind(kind)
        .map(|f| {
            let mut ids: Vec<NodeId> = doc
                .node_map()
                .values()
                .filter(|n| binds_in(n, &[var], f))
                .map(|n| n.id)
                .collect();
            ids.sort();
            (f, ids)
        })
        .collect()
}

/// Whether `n` binds one of `vars` in a field of family `f`.
fn binds_in(n: &crate::Node, vars: &[VarId], f: Field) -> bool {
    bound_in(n, f).any(|v| vars.contains(&v))
}

/// The fields of a style's value a variable in it binds.
fn style_fields(value: &crate::style::StyleValue) -> &'static [Field] {
    use crate::style::StyleValue as V;
    match value {
        V::Fills(_) => &[Field::Fill],
        V::Strokes(_) => &[Field::Stroke, Field::StrokeWidth],
        V::Effects(_) => &[Field::Effect],
        V::Text(_) => &[],
        V::Layout(_) => &[Field::Spacing, Field::FlexDirection, Field::FlexWrap],
    }
}

/// The operations detaching every field bound to a variable `gone` answers for
/// in that field's family, each to the value it drew — the first half of
/// [`remove_and_scope_ops`]. A style reading one detaches it too, a style's
/// family being its slot's.
fn detach_ops(doc: &crate::Document, gone: &dyn Fn(VarId, Field) -> bool) -> Vec<crate::Operation> {
    use crate::Operation as O;
    let t = doc.variables();
    let mut ops = Vec::new();
    let mut ids: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| {
            Field::ALL
                .into_iter()
                .any(|f| bound_in(n, f).any(|v| gone(v, f)))
        })
        .map(|n| n.id)
        .collect();
    ids.sort();
    for id in ids {
        let n = doc.get(id).expect("listed from the document");
        let gone_as = |b: &BrushSource, f: Field| b.var().is_some_and(|v| gone(v, f));
        let gone_fill = |b: &BrushSource| gone_as(b, Field::Fill);
        let gone_stroke = |b: &BrushSource| gone_as(b, Field::Stroke);
        if n.paint.fills.iter().any(|f| gone_fill(&f.brush)) {
            let fills = n
                .paint
                .fills
                .iter()
                .map(|f| {
                    f.map(|f| crate::Fill {
                        brush: if gone_fill(&f.brush) {
                            doc.resolve_brush(id, &f.brush).into_owned().into()
                        } else {
                            f.brush.clone()
                        },
                        ..f.clone()
                    })
                })
                .collect();
            ops.push(O::SetFills { id, fills });
        }
        // And a stroke's width bound to one, detached to its number against the
        // committed table (§15 D1073), in the same list write.
        let width_gone =
            |s: &crate::Stroke| s.width_var.is_some_and(|v| gone(v, Field::StrokeWidth));
        // Whether slot `v` holds a variable gone in family `f`.
        let hit = |v: &Option<VarId>, f: Field| v.is_some_and(|v| gone(v, f));
        if n.paint
            .strokes
            .iter()
            .any(|s| gone_stroke(&s.brush) || width_gone(s))
        {
            let strokes = n
                .paint
                .strokes
                .iter()
                .map(|s| {
                    s.map(|s| {
                        let (width, width_var) = match s.width_var {
                            Some(v) if width_gone(s) => (doc.stroke_width_drawn_in(t, id, v), None),
                            other => (s.width, other),
                        };
                        crate::Stroke {
                            brush: if gone_stroke(&s.brush) {
                                doc.resolve_brush(id, &s.brush).into_owned().into()
                            } else {
                                s.brush.clone()
                            },
                            width,
                            width_var,
                            ..s.clone()
                        }
                    })
                })
                .collect();
            ops.push(O::SetStrokes { id, strokes });
        }
        // And an effect's numbers bound to one, each detached to its number
        // against the committed table in one list write (§15 D1074).
        if n.effects
            .iter()
            .any(|e| e.vars.iter().any(|v| hit(v, Field::Effect)))
        {
            let mut effects = n.effects.clone();
            for e in effects.iter_mut() {
                for slot in 0..4 {
                    if let Some(v) = e.vars[slot]
                        && gone(v, Field::Effect)
                    {
                        let x = doc.effect_number_drawn_in(t, id, v, slot);
                        e.value.set_number(slot, x);
                        e.vars[slot] = None;
                    }
                }
            }
            ops.push(O::SetEffects { id, effects });
        }
        // A size limit bound to one, detached to its px in one item write.
        if n.item.limit_vars.iter().any(|v| hit(v, Field::Limits)) {
            let drawn = doc.limits_drawn_in(t, id, n.item);
            let mut item = n.item;
            for k in 0..item.limit_vars.len() {
                if hit(&item.limit_vars[k], Field::Limits) {
                    *item.limit_mut(k) = drawn.limit(k);
                    item.limit_vars[k] = None;
                }
            }
            ops.push(O::SetLayoutItem { id, item });
        }
        if let Some(corner_radii) = n.kind.corner_radii()
            && n.corner_vars.iter().any(|v| hit(v, Field::Corner))
            && let Some(drawn) = doc.resolve_corners(id, corner_radii)
        {
            let mut radii = corner_radii;
            let mut kept = n.corner_vars;
            let slots = [
                (&mut radii.top_left, drawn.top_left),
                (&mut radii.top_right, drawn.top_right),
                (&mut radii.bottom_right, drawn.bottom_right),
                (&mut radii.bottom_left, drawn.bottom_left),
            ];
            for (k, (slot, value)) in slots.into_iter().enumerate() {
                if hit(&kept[k], Field::Corner) {
                    *slot = value;
                    kept[k] = None;
                }
            }
            ops.push(O::SetGeometry {
                id,
                geometry: crate::GeometryPatch::CornerRadii(radii),
            });
            ops.push(O::SetCornerVars { id, vars: kept });
        }
        let spacing = n.spacing_vars.iter().any(|v| hit(v, Field::Spacing));
        let [direction, wrap] = n.flow_vars();
        let flow = hit(&direction, Field::FlexDirection) || hit(&wrap, Field::FlexWrap);
        if let Some(d) = &n.display
            && (spacing || flow)
        {
            let drawn = doc.spacing_drawn_in(t, id, d, n.spacing_vars);
            let mut display = d.clone();
            if spacing {
                let mut s = d.spacing();
                let mut kept = n.spacing_vars;
                for k in 0..6 {
                    if hit(&kept[k], Field::Spacing) {
                        s[k] = drawn.spacing()[k];
                        kept[k] = None;
                    }
                }
                display.set_spacing(s);
                ops.push(O::SetSpacingVars { id, vars: kept });
            }
            // A direction or wrap detaches to the one it drew, in the layout
            // the binding rides in.
            if let (crate::container::Display::Flex(f), crate::container::Display::Flex(drawn)) =
                (&mut display, &drawn)
            {
                if hit(&direction, Field::FlexDirection) {
                    (f.direction, f.direction_var) = (drawn.direction, None);
                }
                if hit(&wrap, Field::FlexWrap) {
                    (f.wrap, f.wrap_var) = (drawn.wrap, None);
                }
            }
            // Before the spacing's `SetSpacingVars`, as written before.
            let at = ops.len() - usize::from(spacing);
            ops.insert(
                at,
                O::SetDisplay {
                    id,
                    display: Some(display),
                },
            );
        } else if n.display.is_none() && spacing {
            // Inert on a node with no layout, and nothing to detach it to: the
            // binding goes, or the removal would leave it dangling.
            let mut kept = n.spacing_vars;
            for slot in &mut kept {
                if hit(slot, Field::Spacing) {
                    *slot = None;
                }
            }
            ops.push(O::SetSpacingVars { id, vars: kept });
        }
        if let Some(size) = n.kind.box_size()
            && n.size_vars.iter().any(|v| hit(v, Field::Size))
        {
            let drawn = doc
                .size_drawn_in(t, id, &n.kind, n.size_vars)
                .box_size()
                .unwrap_or(size);
            let mut out = [size.width, size.height];
            let mut kept = n.size_vars;
            for k in 0..2 {
                if hit(&kept[k], Field::Size) {
                    out[k] = [drawn.width, drawn.height][k];
                    kept[k] = None;
                }
            }
            ops.push(O::SetGeometry {
                id,
                geometry: crate::GeometryPatch::Size(kurbo::Size::new(out[0], out[1])),
            });
            ops.push(O::SetSizeVars { id, vars: kept });
        }
        let which: [bool; 7] = std::array::from_fn(|k| hit(&n.text_vars[k], text_slot_field(k)));
        if which.iter().any(|x| *x) {
            ops.extend(crate::build::detach_text_ops_in(doc, t, id, which));
        }
        if hit(&n.visible_var, Field::Visible) {
            ops.push(O::SetVisible {
                id,
                visible: doc.shown_in(t, id),
            });
            ops.push(O::SetVisibleVar { id, var: None });
        }
        if hit(&n.opacity_var, Field::Opacity) {
            ops.push(O::SetOpacity {
                id,
                opacity: doc.opacity_drawn_in(t, id, n.opacity, n.opacity_var),
            });
            ops.push(O::SetOpacityVar { id, var: None });
        }
    }
    // And a style whose paints read one detaches them too, to the colour they
    // draw in the default mode (§15 D1062) — a style holds one value for layers
    // in every mode. Its users are rewritten from it by the commit, the style
    // winning over the per-layer detaches above, so they stay styled.
    // In a document with no modes of its own too: a kit's variable resolves in
    // its kit's default there (§15 D1064), and the style must not dangle.
    for s in doc.styles().iter() {
        let fields = style_fields(&s.value);
        let vars: Vec<VarId> = s
            .value
            .vars()
            .into_iter()
            .filter(|v| fields.iter().any(|f| gone(*v, *f)))
            .collect();
        if !vars.is_empty() {
            ops.push(O::SetStyleValue {
                id: s.id,
                value: crate::style::detach_vars(&s.value, t, t.default_mode(), &vars),
            });
        }
    }
    ops
}

/// The operations that delete `mode` (D1038): every frame set to it returns to
/// *Inherit*, then the mode and its column go. Refused at the commit when it is
/// the last mode and variables remain.
pub fn remove_mode_ops(doc: &crate::Document, mode: ModeId) -> Vec<crate::Operation> {
    let t = doc.variables();
    let mut ops = inherit_ops(doc, &[mode]);
    // A by-width collection drops the mode's width; the next takes the first's
    // 0 when the first goes, and with the last it is picked by width no more.
    if let Some(c) = t.collection_of_mode(mode)
        && let Some(b) = &c.breakpoints
    {
        let mut next = b.clone();
        next.remove(&mode);
        let first = c.modes.iter().find(|m| m.id != mode);
        if let Some(first) = first {
            next.insert(first.id, 0.0);
        }
        ops.push(crate::Operation::SetBreakpoints {
            id: c.id,
            breakpoints: first.map(|_| next),
        });
    }
    // And every kit mapping's entry for it (§15 D1064), which would name a mode
    // the document no longer has.
    for (kit, km) in doc.variables().kits() {
        if km.map.contains_key(&mode) {
            let mut next = km.clone();
            next.map.remove(&mode);
            ops.push(crate::Operation::SetKitModes {
                kit: kit.clone(),
                modes: Some(next),
            });
        }
    }
    ops.push(crate::Operation::RemoveMode { id: mode });
    ops
}

/// `SetModes` returning every frame set to one of `modes` to *Inherit* for
/// that mode's collection, in id order, every other mode it sets kept.
fn inherit_ops(doc: &crate::Document, modes: &[ModeId]) -> Vec<crate::Operation> {
    let mut frames: Vec<&crate::Node> = doc
        .node_map()
        .values()
        .filter(|n| n.modes.iter().any(|m| modes.contains(m)))
        .collect();
    frames.sort_by_key(|n| n.id);
    frames
        .into_iter()
        .map(|n| crate::Operation::SetModes {
            id: n.id,
            modes: n
                .modes
                .iter()
                .copied()
                .filter(|m| !modes.contains(m))
                .collect(),
        })
        .collect()
}

/// Move document mode `mode` to `index` of its collection's list — index 0
/// makes it the default every unset frame reads. The mode goes and comes back
/// under its own id with its column, so frames set to it and kit mappings
/// naming it hold. Empty when `mode` is not a mode or already sits there, and
/// in a collection picked by width, whose modes are ordered by their widths.
pub fn move_mode_ops(doc: &crate::Document, mode: ModeId, index: usize) -> Vec<crate::Operation> {
    let t = doc.variables();
    let (Some(c), Some(at)) = (t.collection_of_mode(mode), t.mode_index(mode)) else {
        return Vec::new();
    };
    if c.by_width() {
        return Vec::new();
    }
    let index = index.min(c.modes.len() - 1);
    if at == index {
        return Vec::new();
    }
    let cells = t
        .iter()
        .filter_map(|v| v.cells.get(&mode).map(|c| (v.id, c.clone())))
        .collect();
    vec![
        crate::Operation::RemoveMode { id: mode },
        crate::Operation::AddMode {
            collection: c.id,
            mode: c.modes[at].clone(),
            index,
            cells,
        },
    ]
}

/// The frames set to `mode` — what *Delete mode…* counts in its confirmation.
pub fn frames_set_to(doc: &crate::Document, mode: ModeId) -> usize {
    doc.node_map()
        .values()
        .filter(|n| n.modes.contains(&mode))
        .count()
}

/// A copy of mode `from` with its every cell, added at `index` of the list —
/// *Duplicate mode* (after its source) and *New mode* (which copies the first
/// mode and goes last) — reading every kit's column `from` reads, a
/// mapping entry written where the copy's name would match another. In a
/// collection picked by width the copy starts halfway to the next mode's width,
/// or [`WIDTH_STEP`] past the last. Empty when `from` is not a mode.
pub fn copy_mode_ops(
    doc: &crate::Document,
    from: ModeId,
    id: ModeId,
    name: &str,
    index: usize,
) -> Vec<crate::Operation> {
    let t = doc.variables();
    let Some(c) = t.collection_of_mode(from) else {
        return Vec::new();
    };
    let cells = t
        .iter()
        .filter_map(|v| v.cells.get(&from).map(|c| (v.id, c.clone())))
        .collect();
    let mut ops = vec![crate::Operation::AddMode {
        collection: c.id,
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells,
    }];
    if let Some(b) = &c.breakpoints {
        let width = |i: usize| c.modes.get(i).and_then(|m| b.get(&m.id)).copied();
        let lo = index.checked_sub(1).and_then(width).unwrap_or(0.0);
        let at = match width(index) {
            // Whole px, unless the neighbours are too close for one between.
            Some(hi) => Some(((lo + hi) / 2.0).round())
                .filter(|w| lo < *w && *w < hi)
                .unwrap_or((lo + hi) / 2.0),
            None => lo + WIDTH_STEP,
        };
        let mut next = b.clone();
        next.insert(id, at);
        ops.push(crate::Operation::SetBreakpoints {
            id: c.id,
            breakpoints: Some(next),
        });
    }
    ops.extend(keep_kit_columns(t, c.id, id, name, from));
    ops
}

/// Rename document mode `mode` to `name`, every kit's column it reads kept: a
/// kit mode matched by the old name, or a first mode the new name would trade
/// for one of its own, is written as a mapping entry — a rename relabels the
/// mode, it does not repoint what its layers draw.
pub fn rename_mode_ops(doc: &crate::Document, mode: ModeId, name: &str) -> Vec<crate::Operation> {
    let t = doc.variables();
    let Some(c) = t.collection_of_mode(mode) else {
        return Vec::new();
    };
    if t.mode(mode).is_none_or(|m| m.name == name) {
        return Vec::new();
    }
    let mut ops = vec![crate::Operation::SetModeName {
        id: mode,
        name: name.into(),
    }];
    ops.extend(keep_kit_columns(t, c.id, mode, name, mode));
    ops
}

/// `SetKitModes` giving document mode `mode` of collection `c`, named `name`, a
/// mapping entry to the column `src` reads in every kit collection mapping from
/// `c` where the name alone would map it elsewhere.
///
/// One entry per document mode serves a kit, so where two of its collections
/// map from `c` the first that needs an entry gets it.
fn keep_kit_columns(
    t: &Variables,
    c: CollectionId,
    mode: ModeId,
    name: &str,
    src: ModeId,
) -> Vec<crate::Operation> {
    let mut ops = Vec::new();
    for (kit, km) in &t.kits {
        let want = km.collections().into_iter().find_map(|kc| {
            if t.kit_onto(kc).is_none_or(|onto| onto.id != c) {
                return None;
            }
            let want = t.kit_mode_at(kit, kc, &[src])?;
            if km.map.get(&mode) == Some(&want) {
                return None;
            }
            let by_name = km
                .modes_in(kc)
                .find(|x| x.name == name)
                .or_else(|| km.modes_in(kc).next())
                .map(|x| x.id);
            (by_name != Some(want) || km.map.contains_key(&mode)).then_some(want)
        });
        let Some(want) = want else { continue };
        let mut next = km.clone();
        next.map.insert(mode, want);
        ops.push(crate::Operation::SetKitModes {
            kit: kit.clone(),
            modes: Some(next),
        });
    }
    ops
}

/// The widths *Pick mode by width* gives the modes after the first, in order;
/// each mode past these starts [`WIDTH_STEP`] past the one before.
pub const FIRST_WIDTHS: [f64; 2] = [768.0, 1200.0];

/// How far past the last mode's width a mode added at the end starts.
pub const WIDTH_STEP: f64 = 400.0;

/// *Pick mode by width* ticked (`on`) or unticked on collection `c`.
///
/// Ticked, each mode takes a starting width: 0, then [`FIRST_WIDTHS`], then
/// [`WIDTH_STEP`] more each. Refused with a sentence when another collection is
/// picked by width already. Unticked, the modes stay and the widths go, and
/// every frame that picked a mode by width is set to the mode it drew, so
/// nothing on the canvas changes. Empty when there is nothing to change.
pub fn by_width_ops(
    doc: &crate::Document,
    c: CollectionId,
    on: bool,
) -> Result<Vec<crate::Operation>, String> {
    let t = doc.variables();
    let Some(col) = t.collection(c) else {
        return Ok(Vec::new());
    };
    if col.by_width() == on {
        return Ok(Vec::new());
    }
    if on {
        if let Some(other) = t.by_width() {
            return Err(format!("{} already picks its mode by width.", other.name));
        }
        let mut at = 0.0;
        let breakpoints = col
            .modes
            .iter()
            .enumerate()
            .map(|(i, m)| {
                at = match i {
                    0 => 0.0,
                    i => FIRST_WIDTHS.get(i - 1).copied().unwrap_or(at + WIDTH_STEP),
                };
                (m.id, at)
            })
            .collect();
        return Ok(vec![crate::Operation::SetBreakpoints {
            id: c,
            breakpoints: Some(breakpoints),
        }]);
    }
    let mut frames: Vec<(NodeId, ModeId)> = doc
        .node_map()
        .keys()
        .filter_map(|id| Some((*id, doc.width_pick(*id)?)))
        .collect();
    frames.sort();
    let mut ops: Vec<crate::Operation> = frames
        .into_iter()
        .filter_map(|(id, m)| {
            let n = doc.get(id)?;
            Some(crate::Operation::SetModes {
                id,
                modes: with_mode(t, n.modes(), ModeOwner::Collection(c), Some(m)),
            })
        })
        .collect();
    ops.push(crate::Operation::SetBreakpoints {
        id: c,
        breakpoints: None,
    });
    Ok(ops)
}

/// Mode `mode`'s minimum width typed as `width`, in a collection picked by
/// width. Refused with a sentence for the first mode, whose 0 is fixed, and for
/// a width at or past a neighbour's, naming where that neighbour starts. Empty
/// when nothing changes.
pub fn breakpoint_ops(
    doc: &crate::Document,
    mode: ModeId,
    width: f64,
) -> Result<Vec<crate::Operation>, String> {
    let t = doc.variables();
    let Some(c) = t.collection_of_mode(mode) else {
        return Ok(Vec::new());
    };
    let (Some(b), Some(at)) = (&c.breakpoints, t.mode_index(mode)) else {
        return Ok(Vec::new());
    };
    if at == 0 {
        return Err(format!("{} starts at 0.", c.modes[0].name));
    }
    if !width.is_finite() {
        return Err("A width is a number of px.".into());
    }
    if b.get(&mode) == Some(&width) {
        return Ok(Vec::new());
    }
    let neighbour = |i: usize| c.modes.get(i).and_then(|m| Some((m, *b.get(&m.id)?)));
    if let Some((m, w)) = neighbour(at - 1).filter(|(_, w)| width <= *w) {
        return Err(format!("{} starts at {}.", m.name, px(w)));
    }
    if let Some((m, w)) = neighbour(at + 1).filter(|(_, w)| width >= *w) {
        return Err(format!("{} starts at {}.", m.name, px(w)));
    }
    let mut next = b.clone();
    next.insert(mode, width);
    Ok(vec![crate::Operation::SetBreakpoints {
        id: c.id,
        breakpoints: Some(next),
    }])
}

/// A width as the browser and the refusals write it: whole px bare, a fraction
/// to two places at most.
pub fn px(w: f64) -> String {
    let s = format!("{:.2}", w);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The name *New collection* gives a collection's first mode.
pub const FIRST_MODE_NAME: &str = "Mode 1";

/// The modes a new variable of collection `c` takes a cell in, with the
/// `AddMode` that gives `c` its first, `first_mode` named [`FIRST_MODE_NAME`],
/// when it has none — what every door that files a value into a variable
/// starts with. Both empty when `c` is not a collection.
pub fn columns_for_new(
    t: &Variables,
    c: CollectionId,
    first_mode: ModeId,
) -> (Vec<crate::Operation>, Vec<ModeId>) {
    match t.collection(c) {
        None => (Vec::new(), Vec::new()),
        Some(col) if col.modes.is_empty() => {
            let mut ops = vec![crate::Operation::AddMode {
                collection: c,
                mode: Mode {
                    id: first_mode,
                    name: FIRST_MODE_NAME.into(),
                },
                index: 0,
                cells: Vec::new(),
            }];
            if col.by_width() {
                ops.push(crate::Operation::SetBreakpoints {
                    id: c,
                    breakpoints: Some(Breakpoints::from([(first_mode, 0.0)])),
                });
            }
            (ops, vec![first_mode])
        }
        Some(col) => (Vec::new(), col.modes.iter().map(|m| m.id).collect()),
    }
}

/// *New collection*: collection `id` named `name` at `index` of the list, with
/// one mode, `mode`, named [`FIRST_MODE_NAME`].
pub fn new_collection_ops(
    id: CollectionId,
    mode: ModeId,
    name: &str,
    index: usize,
) -> Vec<crate::Operation> {
    vec![crate::Operation::AddCollection {
        collection: Collection {
            id,
            name: name.into(),
            modes: vec![Mode {
                id: mode,
                name: FIRST_MODE_NAME.into(),
            }],
            breakpoints: None,
        },
        index,
    }]
}

/// *Delete collection…*: its variables go as [`remove_ops`] deletes them, every
/// user detached to the value it drew; every frame set to one of its modes
/// returns to *Inherit* for it, every kit mapping entry naming one goes; then
/// the collection. One transaction. Empty for the last collection, which
/// cannot go.
pub fn remove_collection_ops(doc: &crate::Document, c: CollectionId) -> Vec<crate::Operation> {
    let t = doc.variables();
    let Some(col) = t.collection(c).filter(|_| t.collections.len() > 1) else {
        return Vec::new();
    };
    let vars: Vec<VarId> = t.of_collection(c).map(|v| v.id).collect();
    let modes: Vec<ModeId> = col.modes.iter().map(|m| m.id).collect();
    let mut ops = remove_ops(doc, &vars);
    ops.extend(inherit_ops(doc, &modes));
    for (kit, km) in t.kits() {
        if km.map.keys().any(|m| modes.contains(m)) {
            let mut next = km.clone();
            next.map.retain(|m, _| !modes.contains(m));
            ops.push(crate::Operation::SetKitModes {
                kit: kit.clone(),
                modes: Some(next),
            });
        }
    }
    ops.push(crate::Operation::RemoveCollection { id: c });
    ops
}

/// What *Delete collection…* says before it goes: its variables, the layers
/// bound to them, and the frames set to one of its modes.
pub fn collection_reach(doc: &crate::Document, c: CollectionId) -> (usize, usize, usize) {
    let t = doc.variables();
    let vars: Vec<VarId> = t.of_collection(c).map(|v| v.id).collect();
    let layers = doc.node_map().values().filter(|n| reads(n, &vars)).count();
    let frames = t.collection(c).map_or(0, |col| {
        doc.node_map()
            .values()
            .filter(|n| n.modes.iter().any(|m| col.has(*m)))
            .count()
    });
    (vars.len(), layers, frames)
}

/// *Duplicate collection*: a copy of `c` named `name` after it, its modes and
/// variables under fresh ids from `ids`, each copy's cells its source's — an
/// alias to a variable of `c` pointing at that one's copy, one elsewhere kept.
/// The copy is not picked by width, since only one collection may be. Empty
/// when `c` is not a collection.
pub fn duplicate_collection_ops(
    doc: &crate::Document,
    c: CollectionId,
    name: &str,
    ids: &mut crate::IdSource,
) -> Vec<crate::Operation> {
    let t = doc.variables();
    let (Some(col), Some(at)) = (t.collection(c), t.collection_index(c)) else {
        return Vec::new();
    };
    let modes: BTreeMap<ModeId, ModeId> =
        col.modes.iter().map(|m| (m.id, ids.mint_mode())).collect();
    let sources: Vec<&Variable> = t.of_collection(c).collect();
    let copies: BTreeMap<VarId, VarId> = sources.iter().map(|v| (v.id, ids.mint_var())).collect();
    let mut ops = vec![crate::Operation::AddCollection {
        collection: Collection {
            id: ids.mint_collection(),
            name: name.into(),
            modes: col
                .modes
                .iter()
                .map(|m| Mode {
                    id: modes[&m.id],
                    name: m.name.clone(),
                })
                .collect(),
            breakpoints: None,
        },
        index: at + 1,
    }];
    for v in sources {
        let cells = v
            .cells
            .iter()
            .map(|(m, cell)| {
                let cell = match cell {
                    Cell::Alias(to) => Cell::Alias(copies.get(to).copied().unwrap_or(*to)),
                    other => other.clone(),
                };
                (modes[m], cell)
            })
            .collect();
        ops.push(crate::Operation::AddVariable {
            var: Variable {
                id: copies[&v.id],
                cells,
                forked: None,
                ..v.clone()
            },
        });
    }
    ops
}

/// *Move to*: `vars` (the document's own) into collection `to`, each cell of
/// `to`'s modes taken from the source's mode of the same name, else from the
/// source's default. The ids stay, so bindings and aliases keep pointing at
/// them, and nothing draws differently in a frame whose modes match. `Err` with
/// the first name `to` already holds; a variable already in `to` is passed by,
/// and nothing moves into a collection with no mode.
pub fn move_ops(
    doc: &crate::Document,
    vars: &[VarId],
    to: CollectionId,
) -> Result<Vec<crate::Operation>, String> {
    let t = doc.variables();
    // A collection with no mode has no column to take the cells.
    let Some(target) = t.collection(to).filter(|c| !c.modes.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut ops = Vec::new();
    let mut taken: Vec<&str> = t.of_collection(to).map(|o| o.name.as_str()).collect();
    for &id in vars {
        let Some(v) = t.get(id).filter(|v| v.origin.is_none()) else {
            continue;
        };
        let Some(from) = t.collection_of(id).filter(|c| c.id != to) else {
            continue;
        };
        if taken.contains(&v.name.as_str()) {
            return Err(v.name.clone());
        }
        taken.push(&v.name);
        let cells = target
            .modes
            .iter()
            .filter_map(|m| {
                let src = from
                    .modes
                    .iter()
                    .find(|s| s.name == m.name)
                    .or(from.modes.first())?;
                Some((m.id, v.cells.get(&src.id)?.clone()))
            })
            .collect();
        ops.push(crate::Operation::SetVariable {
            var: Variable { cells, ..v.clone() },
        });
    }
    Ok(ops)
}

/// The group a variable's name puts it in — everything before its last `/` —
/// and its leaf. *brand/primary/500* is in *brand/primary*, named *500*; a name
/// with no slash is in the unnamed top group.
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rsplit_once('/') {
        Some((group, leaf)) => (group, leaf),
        None => ("", name),
    }
}

/// The renames that move every variable of collection `c` in group `from` (and its subgroups) to
/// `to` — *Rename group*, the bindings following because they name ids. `Err`
/// with the first name that would land on a variable already there: merging two
/// groups is not ruled, so a taken name is refused, as a single rename's is.
pub fn rename_group_ops(
    doc: &crate::Document,
    c: CollectionId,
    from: &str,
    to: &str,
) -> Result<Vec<crate::Operation>, String> {
    let t = doc.variables();
    let local = || t.of_collection(c);
    let moved = |name: &str| -> Option<String> {
        if name.starts_with(&format!("{from}/")) {
            let rest = &name[from.len() + 1..];
            Some(if to.is_empty() {
                rest.to_string()
            } else {
                format!("{to}/{rest}")
            })
        } else {
            None
        }
    };
    // The document's own groups only, in one collection: a kit's variables are
    // read-only here (§15 D1064), and each collection's names a namespace of
    // their own.
    let mut ops = Vec::new();
    let moving: Vec<VarId> = local()
        .filter(|v| moved(&v.name).is_some())
        .map(|v| v.id)
        .collect();
    for v in local() {
        let Some(next) = moved(&v.name) else { continue };
        if local().any(|o| o.name == next && !moving.contains(&o.id)) {
            return Err(next);
        }
        ops.push(crate::Operation::SetVariableName {
            id: v.id,
            name: next,
        });
    }
    Ok(ops)
}

/// Why the variables table or a binding was refused ([`check`]).
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum VarRule {
    #[error("there are variables and no mode")]
    NoModes,
    #[error("mode {0:?} appears twice")]
    DuplicateMode(ModeId),
    #[error("{0:?} has no value in mode {1:?}")]
    MissingCell(VarId, ModeId),
    #[error("{0:?} has a value in mode {1:?}, which the document does not have")]
    StrayCell(VarId, ModeId),
    #[error("{0:?} holds a value of the wrong kind")]
    WrongKind(VarId),
    #[error("{0:?} aliases a variable that is not there or of another kind")]
    BadAlias(VarId),
    #[error("{0:?}'s aliases make a loop")]
    AliasLoop(VarId),
    #[error("the name {0:?} is used twice")]
    DuplicateName(String),
    #[error("{0:?} holds a number that is not finite")]
    NonFinite(VarId),
    #[error("node {0:?} is bound to a variable that is not there or of another kind")]
    BadBinding(NodeId),
    #[error("node {0:?} sets a mode that is not there, or is not a frame or a page")]
    BadMode(NodeId),
    #[error("node {0:?} binds corners it does not have")]
    BadCorners(NodeId),
    #[error("{0:?} names a kit whose modes the document does not hold, or a malformed kit id")]
    NoKit(VarId),
    #[error("kit {0:?}'s modes or mapping name a mode that is not there")]
    BadKitModes(String),
    #[error("node {0:?} is in the vault and bound to a variable of the document's own")]
    InVault(NodeId),
    #[error("the document has no collection")]
    NoCollection,
    #[error("collection {0:?} appears twice")]
    DuplicateCollection(CollectionId),
    #[error("two collections are named {0:?}")]
    DuplicateCollectionName(String),
    #[error("{0:?} is a boolean and has a scope")]
    BooleanScope(VarId),
    #[error("node {0:?} binds a variable outside its scope")]
    OutOfScope(NodeId),
    #[error("node {0:?} sets two modes of one collection")]
    TwoModes(NodeId),
    #[error("two collections are picked by width")]
    TwoByWidth,
    #[error("{0:?} is scoped to a flex direction or wrap and holds another word")]
    NotAWord(VarId),
    #[error("collection {0:?}'s widths are not one per mode, from 0 and ascending")]
    BadBreakpoints(CollectionId),
}

/// Whether string `v`, when scoped to flex directions or wraps and nothing
/// else, holds only their words in every literal cell (an alias is judged at
/// its target). True for every other variable.
pub fn holds_its_words(v: &Variable) -> bool {
    let Some(scope) = v.scope.filter(|s| !s.is_empty()) else {
        return true;
    };
    if v.kind != VarKind::String || !scope.fields().all(Field::needs_scope) {
        return true;
    }
    v.cells.values().all(|c| match c {
        Cell::String(s) => scope.fields().any(|f| f.takes(s)),
        _ => true,
    })
}

/// Whether every value `var` can take, through its aliases, is a word `field`
/// takes ([`Field::takes`]) — what a flex direction or wrap stays bound to. A
/// string scoped wider than them may hold a label in some mode. True for any
/// other field.
pub fn names_one_everywhere(t: &Variables, var: VarId, field: Field) -> bool {
    !field.needs_scope()
        || t.every_value(var)
            .iter()
            .all(|v| v.as_str().is_some_and(|s| field.takes(s)))
}

/// The table's own rules, without the bindings.
///
/// A kit's variable (§15 D1064) has a cell in every mode **of its kit** and no
/// other, aliases only a kit's variable (its own kit's or another's), and is named uniquely among
/// its kit's; one of the document's own has a cell in every mode of **one** of
/// its collections and no other, and is named uniquely in it. A kit's modes and
/// the document's never share an id, and a mapping maps a mode of the document
/// onto a mode of its kit. The collections are at least one, their ids and
/// names each unique. Only a number, a colour or a string has a scope.
pub(crate) fn check_table(t: &Variables) -> Result<(), VarRule> {
    if t.collections.is_empty() {
        return Err(VarRule::NoCollection);
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut cnames = std::collections::BTreeSet::new();
    for c in &t.collections {
        if !ids.insert(c.id) {
            return Err(VarRule::DuplicateCollection(c.id));
        }
        if !cnames.insert(c.name.as_str()) {
            return Err(VarRule::DuplicateCollectionName(c.name.clone()));
        }
        if let Some(b) = &c.breakpoints {
            let widths: Vec<Option<f64>> = c.modes.iter().map(|m| b.get(&m.id).copied()).collect();
            let whole = b.len() == c.modes.len()
                && widths.iter().all(|w| w.is_some_and(f64::is_finite))
                && widths.first().is_none_or(|w| *w == Some(0.0))
                && widths.windows(2).all(|p| p[0] < p[1]);
            if !whole {
                return Err(VarRule::BadBreakpoints(c.id));
            }
        }
    }
    if t.collections.iter().filter(|c| c.by_width()).count() > 1 {
        return Err(VarRule::TwoByWidth);
    }
    let mut seen = std::collections::BTreeSet::new();
    let every = t
        .modes()
        .map(|m| m.id)
        .chain(t.kits.values().flat_map(|k| k.modes.iter().map(|m| m.id)));
    for m in every {
        if !seen.insert(m) {
            return Err(VarRule::DuplicateMode(m));
        }
    }
    for (kit, km) in &t.kits {
        if !crate::meta::DocumentMeta::is_wellformed_id(kit)
            || km
                .map
                .iter()
                .any(|(from, to)| t.mode(*from).is_none() || km.mode(*to).is_none())
        {
            return Err(VarRule::BadKitModes(kit.clone()));
        }
    }
    let mut names = std::collections::BTreeSet::new();
    for v in t.vars.values() {
        if v.kind == VarKind::Boolean && v.scope.is_some() {
            return Err(VarRule::BooleanScope(v.id));
        }
        if !holds_its_words(v) {
            return Err(VarRule::NotAWord(v.id));
        }
        let collection = match v.kit() {
            None => Some(t.collection_of(v.id).ok_or(VarRule::NoModes)?.id),
            Some(_) => None,
        };
        if !names.insert((v.kit(), collection, t.kit_collection_of(v), v.name.as_str())) {
            return Err(VarRule::DuplicateName(v.name.clone()));
        }
        let columns: Vec<ModeId> = match v.kit() {
            None => t.columns(v),
            // A kit with no modes has no column to read: every read would
            // fall back to transparent or 0.
            Some(kit) => match t.kits.get(kit) {
                Some(km) if !km.modes.is_empty() => t.columns(v),
                _ => return Err(VarRule::NoKit(v.id)),
            },
        };
        for m in &columns {
            if !v.cells.contains_key(m) {
                return Err(VarRule::MissingCell(v.id, *m));
            }
        }
        for (mode, cell) in &v.cells {
            if !columns.contains(mode) {
                return Err(VarRule::StrayCell(v.id, *mode));
            }
            if !cell.is_finite() {
                return Err(VarRule::NonFinite(v.id));
            }
            match cell {
                Cell::Alias(target) => {
                    // A kit's variable aliases a kit's — its own, or one its
                    // kit held when it published (each step reads its own
                    // kit's column, `kit_mode`) — never the document's own,
                    // which its kit never had.
                    let kits_only =
                        v.kit().is_none() || t.get(*target).and_then(Variable::kit).is_some();
                    if t.kind_of(*target) != Some(v.kind) || !kits_only {
                        return Err(VarRule::BadAlias(v.id));
                    }
                    if t.would_loop(v.id, *mode, *target) {
                        return Err(VarRule::AliasLoop(v.id));
                    }
                }
                c => {
                    if c.kind() != Some(v.kind) {
                        return Err(VarRule::WrongKind(v.id));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The variables rules, over the table and every node's bindings — a
/// post-condition of every transaction (`Document::apply`) and of the loader.
///
/// Over the whole document for `component::check`'s reason: deleting a variable
/// breaks a binding on a node the delete never dirtied.
///
/// **A vault node binds only a kit's variables and sets only a kit's mode**
/// (§15 D1064), `crate::kit::check`'s `LinksOut` made for the table: a frozen
/// copy reading a variable of the document's own would change with every edit of
/// it, and be put back by the next update.
pub(crate) fn check<'a>(
    t: &Variables,
    nodes: impl IntoIterator<Item = &'a crate::Node>,
    in_vault: impl Fn(NodeId) -> bool,
) -> Result<(), VarRule> {
    check_table(t)?;
    for n in nodes {
        if in_vault(n.id) {
            let own = |v: VarId| t.get(v).is_some_and(|v| v.origin.is_none());
            let reads_own = n
                .paint
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
                .chain(n.flow_vars().into_iter().flatten())
                .chain(n.size_vars.iter().flatten().copied())
                .chain(n.text_vars.iter().flatten().copied())
                .chain(n.item.limit_vars.iter().flatten().copied())
                .chain(n.visible_var)
                .chain(n.opacity_var)
                .any(own);
            if reads_own || n.modes.iter().any(|m| t.mode(*m).is_some()) {
                return Err(VarRule::InVault(n.id));
            }
        }
        let colour_ok = |b: &BrushSource| match b.var() {
            Some(v) => t.kind_of(v) == Some(VarKind::Color),
            None => true,
        };
        let fills = n.paint.fills.iter().all(|f| colour_ok(&f.brush));
        // And a stroke's width binds a number (§15 D1073).
        let strokes = n.paint.strokes.iter().all(|s| {
            colour_ok(&s.brush)
                && s.width_var
                    .is_none_or(|v| t.kind_of(v) == Some(VarKind::Number))
        });
        // And an effect's numbers bind numbers, each only where its kind has
        // the number (§15 D1074) — no binding on a layer blur's X.
        let effects = n.effects.iter().all(|e| {
            let numbers = e.numbers();
            (0..4).all(|k| match e.vars[k] {
                Some(v) => numbers[k].is_some() && t.kind_of(v) == Some(VarKind::Number),
                None => true,
            })
        });
        if !(fills && strokes && effects) {
            return Err(VarRule::BadBinding(n.id));
        }
        if n.corner_vars.iter().any(Option::is_some) {
            if n.kind.corner_radii().is_none() {
                return Err(VarRule::BadCorners(n.id));
            }
            if n.corner_vars
                .iter()
                .flatten()
                .any(|v| t.kind_of(*v) != Some(VarKind::Number))
            {
                return Err(VarRule::BadBinding(n.id));
            }
        }
        if n.spacing_vars
            .iter()
            .flatten()
            .any(|v| t.kind_of(*v) != Some(VarKind::Number))
        {
            return Err(VarRule::BadBinding(n.id));
        }
        // A flex direction or wrap binds a string.
        if n.flow_vars()
            .iter()
            .flatten()
            .any(|v| t.kind_of(*v) != Some(VarKind::String))
        {
            return Err(VarRule::BadBinding(n.id));
        }
        // A width or a height binds a number, and only on a kind with a stored
        // size to stand in for (§15 D1061).
        if n.size_vars.iter().any(Option::is_some)
            && (n.kind.box_size().is_none()
                || n.size_vars
                    .iter()
                    .flatten()
                    .any(|v| t.kind_of(*v) != Some(VarKind::Number)))
        {
            return Err(VarRule::BadBinding(n.id));
        }
        // A size limit binds a number.
        if n.item
            .limit_vars
            .iter()
            .flatten()
            .any(|v| t.kind_of(*v) != Some(VarKind::Number))
        {
            return Err(VarRule::BadBinding(n.id));
        }
        if n.visible_var
            .is_some_and(|v| t.kind_of(v) != Some(VarKind::Boolean))
        {
            return Err(VarRule::BadBinding(n.id));
        }
        // An opacity binds a number, on any layer (§15 D1072).
        if n.opacity_var
            .is_some_and(|v| t.kind_of(v) != Some(VarKind::Number))
        {
            return Err(VarRule::BadBinding(n.id));
        }
        // A content or a family binds a string, a type number a number, and
        // only on a text.
        if n.text_vars.iter().any(Option::is_some)
            && (!matches!(n.kind, crate::NodeKind::Text { .. })
                || n.text_vars
                    .iter()
                    .enumerate()
                    .any(|(k, v)| v.is_some_and(|v| t.kind_of(v) != Some(text_slot_kind(k)))))
        {
            return Err(VarRule::BadBinding(n.id));
        }
        // And every binding inside its variable's scope, the kinds being right.
        if Field::ALL
            .into_iter()
            .any(|f| bound_in(n, f).any(|v| t.get(v).is_some_and(|v| !v.binds(f))))
        {
            return Err(VarRule::OutOfScope(n.id));
        }
        // A frame sets modes of the document's or a kit's; a page's root only
        // the document's own, since no kit's variables live on a page. One per
        // collection.
        let page = matches!(n.kind, crate::NodeKind::Root) && !in_vault(n.id);
        for (i, &m) in n.modes.iter().enumerate() {
            if !(matches!(n.kind, crate::NodeKind::Artboard { .. }) && t.has_mode(m)
                || page && t.mode(m).is_some())
            {
                return Err(VarRule::BadMode(n.id));
            }
            if n.modes[..i].iter().any(|x| same_collection(t, *x, m)) {
                return Err(VarRule::TwoModes(n.id));
            }
        }
    }
    Ok(())
}

/// The variables `n` binds in fields of family `f`.
pub fn bound_in(n: &crate::Node, f: Field) -> impl Iterator<Item = VarId> + '_ {
    let list: Vec<VarId> = match f {
        Field::Fill => n.paint.fills.iter().filter_map(|x| x.brush.var()).collect(),
        Field::Stroke => n
            .paint
            .strokes
            .iter()
            .filter_map(|x| x.brush.var())
            .collect(),
        Field::StrokeWidth => n.paint.strokes.iter().filter_map(|x| x.width_var).collect(),
        Field::Effect => n
            .effects
            .iter()
            .flat_map(|e| e.vars.iter().flatten().copied())
            .collect(),
        Field::Limits => n.item.limit_vars.iter().flatten().copied().collect(),
        Field::Corner => n.corner_vars.iter().flatten().copied().collect(),
        Field::Spacing => n.spacing_vars.iter().flatten().copied().collect(),
        Field::FlexDirection => n.flow_vars()[0].into_iter().collect(),
        Field::FlexWrap => n.flow_vars()[1].into_iter().collect(),
        Field::Size => n.size_vars.iter().flatten().copied().collect(),
        Field::Visible => n.visible_var.into_iter().collect(),
        Field::Opacity => n.opacity_var.into_iter().collect(),
        Field::Type | Field::Weight | Field::Content | Field::Family => n
            .text_vars
            .iter()
            .enumerate()
            .filter(|(k, _)| text_slot_field(*k) == f)
            .filter_map(|(_, v)| *v)
            .collect(),
    };
    list.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(seq: u64) -> NodeId {
        NodeId { actor: 7, seq }
    }

    fn table() -> (Variables, ModeId, ModeId, VarId, VarId) {
        let (light, dark) = (ModeId(id(1)), ModeId(id(2)));
        let (brand, accent) = (VarId(id(3)), VarId(id(4)));
        let mut t = Variables {
            collections: vec![Collection {
                id: CollectionId::FIRST,
                name: FIRST_COLLECTION_NAME.into(),
                modes: vec![
                    Mode {
                        id: light,
                        name: "Light".into(),
                    },
                    Mode {
                        id: dark,
                        name: "Dark".into(),
                    },
                ],
                breakpoints: None,
            }],
            vars: BTreeMap::new(),
            kits: Default::default(),
            previewed_width: None,
        };
        t.vars.insert(
            brand,
            Variable {
                id: brand,
                name: "brand/500".into(),
                kind: VarKind::Color,
                cells: [
                    (light, Cell::Color(Color::WHITE)),
                    (dark, Cell::Color(Color::BLACK)),
                ]
                .into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        );
        t.vars.insert(
            accent,
            Variable {
                id: accent,
                name: "accent".into(),
                kind: VarKind::Color,
                cells: [
                    (light, Cell::Alias(brand)),
                    (dark, Cell::Color(Color::WHITE)),
                ]
                .into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        );
        (t, light, dark, brand, accent)
    }

    /// An alias reads its target **in the same mode**, and only in the modes it
    /// aliases in.
    #[test]
    fn an_alias_resolves_in_its_own_mode() {
        let (t, light, dark, _, accent) = table();
        assert_eq!(t.resolve(accent, light), Some(Value::Color(Color::WHITE)));
        assert_eq!(t.resolve(accent, dark), Some(Value::Color(Color::WHITE)));
        check_table(&t).unwrap();
    }

    /// A loop is refused, and a read that meets one anyway ends.
    #[test]
    fn an_alias_loop_is_refused_and_a_read_of_one_ends() {
        let (mut t, light, _, brand, accent) = table();
        t.vars
            .get_mut(&brand)
            .unwrap()
            .cells
            .insert(light, Cell::Alias(accent));
        assert!(t.would_loop(brand, light, accent));
        assert_eq!(t.resolve(brand, light), None);
        assert!(matches!(check_table(&t), Err(VarRule::AliasLoop(_))));
    }

    #[test]
    fn a_missing_column_and_a_wrong_kind_are_refused() {
        let (mut t, _, dark, brand, _) = table();
        t.vars.get_mut(&brand).unwrap().cells.remove(&dark);
        assert_eq!(check_table(&t), Err(VarRule::MissingCell(brand, dark)));
        t.vars
            .get_mut(&brand)
            .unwrap()
            .cells
            .insert(dark, Cell::Number(4.0));
        assert_eq!(check_table(&t), Err(VarRule::WrongKind(brand)));
    }

    /// A cell in a mode the variable has no column for is refused.
    ///
    /// flip: `check_table`'s `StrayCell` arm removed → the table is accepted.
    #[test]
    fn a_stray_cell_is_refused() {
        let (mut t, ..) = table();
        let gone = ModeId(id(9));
        let brand = VarId(id(3));
        t.vars
            .get_mut(&brand)
            .unwrap()
            .cells
            .insert(gone, Cell::Color(Color::WHITE));
        assert_eq!(check_table(&t), Err(VarRule::StrayCell(brand, gone)));
    }

    /// **Names are unique within the document's own and within each kit's**,
    /// never across them (§15 D1064): two of the document's own named alike are
    /// refused, a kit's `accent` beside the document's is not, and two of one
    /// kit's named alike are.
    ///
    /// flips: `DuplicateName` disabled → fails *"two of the document's own"*;
    /// the key's kit dropped → fails *"a kit's namesake is its own"*.
    #[test]
    fn names_are_unique_per_namespace() {
        let (mut t, light, dark, _, accent) = table();
        let mut twin = t.vars[&accent].clone();
        twin.id = VarId(id(5));
        twin.cells = [
            (light, Cell::Color(Color::WHITE)),
            (dark, Cell::Color(Color::WHITE)),
        ]
        .into();
        t.vars.insert(twin.id, twin.clone());
        assert_eq!(
            check_table(&t),
            Err(VarRule::DuplicateName("accent".into())),
            "two of the document's own"
        );
        let kit = "5eed0000000000000000000000000011".to_string();
        let day = ModeId(id(6));
        t.kits.insert(
            kit.clone(),
            KitModes {
                name: None,
                modes: vec![KitMode {
                    id: day,
                    name: "Day".into(),
                    from: day,
                    collection: String::new(),
                }],
                map: Default::default(),
            },
        );
        let of_kit = |v: &mut Variable, var: u64| {
            v.id = VarId(id(var));
            v.cells = [(day, Cell::Color(Color::WHITE))].into();
            v.origin = Some(VarOrigin {
                kit: kit.clone(),
                var: v.id,
                version: 1,
            });
        };
        let mut kits = twin.clone();
        of_kit(&mut kits, 5);
        t.vars.insert(kits.id, kits.clone());
        assert_eq!(check_table(&t), Ok(()), "a kit's namesake is its own");
        let mut second = kits;
        of_kit(&mut second, 8);
        t.vars.insert(second.id, second);
        assert_eq!(
            check_table(&t),
            Err(VarRule::DuplicateName("accent".into())),
            "two of one kit's"
        );
    }

    /// A literal saves as the brush alone — a file with no bindings is the bytes
    /// it was — and a binding as `{"var": …}`, its `%` left out at 1; all read back.
    #[test]
    fn a_brush_source_saves_as_its_brush_or_as_a_var() {
        let lit: BrushSource = Color::WHITE.into();
        let as_brush = serde_json::to_string(&crate::Brush::Solid(Color::WHITE)).unwrap();
        assert_eq!(serde_json::to_string(&lit).unwrap(), as_brush);
        assert_eq!(serde_json::from_str::<BrushSource>(&as_brush).unwrap(), lit);
        for (alpha, want) in [
            (1.0, r#"{"var":"7:3"}"#),
            (0.5, r#"{"var":"7:3","alpha":0.5}"#),
        ] {
            let bound = BrushSource::Bound {
                var: VarId(id(3)),
                alpha,
            };
            let s = serde_json::to_string(&bound).unwrap();
            assert_eq!(s, want);
            assert_eq!(serde_json::from_str::<BrushSource>(&s).unwrap(), bound);
        }
    }
}
