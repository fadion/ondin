//! Variables and modes (§5.3f, §15 D1037, D1038, D1039).
//!
//! A **variable** is a named value of one type — a colour or a number in build
//! step 1 — with **one value per mode**. The document has **one mode list**, no
//! collections, and its default mode is the first (D1038 (a)). A frame may set
//! one mode for itself and everything under it ([`crate::Node::mode`]); anything
//! below no frame that sets one resolves in the default.
//!
//! A cell is a value or an **alias** — another variable of the same type, named
//! per mode — never a fifth type. A field bound to a variable holds
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

impl crate::IdSource {
    /// A fresh [`VarId`].
    pub fn mint_var(&mut self) -> VarId {
        VarId(self.mint())
    }
    /// A fresh [`ModeId`].
    pub fn mint_mode(&mut self) -> ModeId {
        ModeId(self.mint())
    }
}

/// One of the document's modes — a column of the variables table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mode {
    pub id: ModeId,
    pub name: String,
}

/// What a variable holds. Build step 1's two, and step 2's boolean, bound to a
/// layer's visibility (D1038 (d), §15 D1059); string arrives with the fields it
/// binds to (roadmap step 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VarKind {
    Color,
    Number,
    Boolean,
}

/// One variable's value in one mode.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cell {
    Color(Color),
    Number(f64),
    Boolean(bool),
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
            Cell::Alias(_) => None,
        }
    }

    fn is_finite(&self) -> bool {
        match self {
            Cell::Color(c) => c.components.iter().all(|x| x.is_finite()),
            Cell::Number(n) => n.is_finite(),
            Cell::Boolean(_) | Cell::Alias(_) => true,
        }
    }
}

/// A resolved value: what a cell comes to once its aliases are followed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Color(Color),
    Number(f64),
    Boolean(bool),
}

impl From<Value> for Cell {
    /// The cell holding `v` itself — what an alias becomes when its target goes.
    fn from(v: Value) -> Cell {
        match v {
            Value::Color(c) => Cell::Color(c),
            Value::Number(n) => Cell::Number(n),
            Value::Boolean(b) => Cell::Boolean(b),
        }
    }
}

/// A variable: a name, a kind and one cell per mode.
///
/// **The name is a path** — `brand/500` — and the slashes are the browser's
/// groups (D1038 G); there is no group object. Names are unique in the document
/// ([`check`]), which is what lets a paste from another document rebind by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub id: VarId,
    pub name: String,
    pub kind: VarKind,
    /// One cell per mode of the document, keyed by mode — **every** mode, which
    /// [`check`] holds, so a read never meets a missing column. Ordered by id for
    /// byte-stable saves (invariant 9), not by the modes' order.
    pub cells: BTreeMap<ModeId, Cell>,
}

/// The document's variables and its mode list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variables {
    pub(crate) modes: Vec<Mode>,
    pub(crate) vars: BTreeMap<VarId, Variable>,
}

impl Variables {
    pub fn modes(&self) -> &[Mode] {
        &self.modes
    }
    /// The default mode — the first (D1038 (a)) — or `None` in a document with
    /// no modes, which has no variables either ([`check`]).
    pub fn default_mode(&self) -> Option<ModeId> {
        self.modes.first().map(|m| m.id)
    }
    pub fn mode(&self, id: ModeId) -> Option<&Mode> {
        self.modes.iter().find(|m| m.id == id)
    }
    pub fn mode_index(&self, id: ModeId) -> Option<usize> {
        self.modes.iter().position(|m| m.id == id)
    }
    pub fn get(&self, id: VarId) -> Option<&Variable> {
        self.vars.get(&id)
    }
    /// Every variable, in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Variable> {
        self.vars.values()
    }
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.modes.is_empty()
    }
    pub fn by_name(&self, name: &str) -> Option<&Variable> {
        self.vars.values().find(|v| v.name == name)
    }

    /// `var`'s value in `mode`, its aliases followed.
    ///
    /// `None` only for a table [`check`] refuses — a variable or a mode that is
    /// not there, a missing cell, or an alias loop — so every caller holding a
    /// committed document can treat it as unreachable. The walk is bounded by the
    /// variable count all the same: a loop is refused at the commit, and a read
    /// that met one anyway must end.
    pub fn resolve(&self, var: VarId, mode: ModeId) -> Option<Value> {
        let mut at = var;
        for _ in 0..=self.vars.len() {
            match self.vars.get(&at)?.cells.get(&mode)? {
                Cell::Color(c) => return Some(Value::Color(*c)),
                Cell::Number(n) => return Some(Value::Number(*n)),
                Cell::Boolean(b) => return Some(Value::Boolean(*b)),
                Cell::Alias(next) => at = *next,
            }
        }
        None
    }

    /// This table with `var`'s cell in `mode` set to `cell` — what a previewed
    /// `SetVariableCell` resolves against (§15 D1056), the document's own table
    /// left as committed. `None` when the variable or the mode is not here. A
    /// loop the cell would close resolves to nothing, as [`Self::resolve`]'s walk
    /// is bounded; the commit refuses it.
    pub fn with_cell(&self, var: VarId, mode: ModeId, cell: Cell) -> Option<Variables> {
        self.mode(mode)?;
        let mut t = self.clone();
        t.vars.get_mut(&var)?.cells.insert(mode, cell);
        Some(t)
    }

    /// The kind `var` resolves to.
    pub fn kind_of(&self, var: VarId) -> Option<VarKind> {
        self.vars.get(&var).map(|v| v.kind)
    }

    /// Whether `var`'s cell in `mode` aliasing `target` would make a loop — the
    /// browser's dimmed rows (D1038 G). True when `target` is `var` or reaches it
    /// in that mode.
    pub fn would_loop(&self, var: VarId, mode: ModeId, target: VarId) -> bool {
        let mut at = target;
        for _ in 0..=self.vars.len() {
            if at == var {
                return true;
            }
            match self.vars.get(&at).and_then(|v| v.cells.get(&mode)) {
                Some(Cell::Alias(next)) => at = *next,
                _ => return false,
            }
        }
        true
    }

    /// Every id in the table, for `reserve_existing_ids`.
    pub(crate) fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.modes
            .iter()
            .map(|m| m.id.0)
            .chain(self.vars.keys().map(|v| v.0))
    }
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

/// `node` — a node of `doc`, or a copy of one still carrying its id — with every
/// binding **detached to the value it draws** in `doc` (D1038's detach): a bound
/// paint becomes a solid of its variable's colour times its `%`, a bound corner,
/// gap or padding its number (§15 D1058), a bound visibility what it shows (§15
/// D1059), and a frame's mode is dropped.
///
/// For a node leaving the document whose table it reads, into one that has none
/// of it: a kit's frozen copy until kits carry variables (roadmap step 4, §15
/// D1039), and a paste into another document with no variable of the name.
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
    }
    if let crate::NodeKind::Rect { corner_radii, .. } = &mut node.kind
        && let Some(drawn) = doc.resolve_corners(id, *corner_radii)
    {
        *corner_radii = drawn;
    }
    node.corner_vars = Default::default();
    if let Some(d) = &node.display {
        node.display = Some(doc.spacing_drawn_in(doc.variables(), id, d, node.spacing_vars));
    }
    node.spacing_vars = Default::default();
    node.kind = doc.size_drawn_in(doc.variables(), id, &node.kind, node.size_vars);
    node.size_vars = Default::default();
    if node.visible_var.is_some() {
        node.visible = doc.shown(id);
        node.visible_var = None;
    }
    node.mode = None;
    node
}

/// One variable a copy's layers read, as the copy knows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarriedVar {
    pub id: VarId,
    pub name: String,
    pub kind: VarKind,
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
    pub modes: Vec<Mode>,
    pub detached: Vec<crate::Node>,
}

impl Carried {
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.modes.is_empty() && self.detached.is_empty()
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
            .chain(n.corner_vars.iter().flatten().copied())
            .chain(n.spacing_vars.iter().flatten().copied())
            .chain(n.size_vars.iter().flatten().copied())
            .chain(n.visible_var);
        for v in vars {
            if out.vars.iter().any(|c| c.id == v) {
                continue;
            }
            if let Some(var) = doc.variables().get(v) {
                out.vars.push(CarriedVar {
                    id: v,
                    name: var.name.clone(),
                    kind: var.kind,
                });
            }
        }
        if let Some(m) = n.mode
            && !out.modes.iter().any(|c| c.id == m)
            && let Some(mode) = doc.variables().mode(m)
        {
            out.modes.push(mode.clone());
        }
        if n.reads_variables() {
            out.detached.push(detached(doc, n.clone()));
        }
    }
    out
}

/// D1038's paste rule for one binding, landing in table `t`: `v` itself where
/// `t` has it at the kind the copy knew, else `t`'s variable of the same name and
/// kind, else `None` — the caller detaches. Shared by [`land`] and a property
/// paste (`build::paste_properties`, §15 D1047), so the two cannot drift.
pub fn rebind(t: &Variables, carried: &[CarriedVar], v: VarId) -> Option<VarId> {
    let known = carried.iter().find(|c| c.id == v);
    if let Some(var) = t.get(v)
        && known.is_none_or(|c| c.kind == var.kind)
    {
        return Some(v);
    }
    let c = known?;
    t.by_name(&c.name)
        .filter(|var| var.kind == c.kind)
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
    let rebind = |v: VarId| rebind(t, &carried.vars, v);
    nodes
        .into_iter()
        .map(|mut n| {
            if !n.reads_variables() && n.mode.is_none() {
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
                    f.brush = match rebind(var) {
                        Some(var) => BrushSource::Bound { var, alpha },
                        None => twin_brush(true, i).into(),
                    };
                }
            }
            for (i, s) in n.paint.strokes.iter_mut().enumerate() {
                if let BrushSource::Bound { var, alpha } = s.brush {
                    s.brush = match rebind(var) {
                        Some(var) => BrushSource::Bound { var, alpha },
                        None => twin_brush(false, i).into(),
                    };
                }
            }
            if n.corner_vars.iter().any(Option::is_some) {
                let drawn = twin.and_then(|d| match d.kind {
                    crate::NodeKind::Rect { corner_radii, .. } => Some(corner_radii),
                    _ => None,
                });
                let mut vars = n.corner_vars;
                if let crate::NodeKind::Rect { corner_radii, .. } = &mut n.kind {
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
                            vars[k] = rebind(v);
                            if vars[k].is_none() {
                                *slot = fallback.map_or(*slot, |f| f[k]);
                            }
                        }
                    }
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
                        *slot = rebind(v);
                        if slot.is_none() {
                            s[k] = fallback.map_or(s[k], |f| f[k]);
                        }
                    }
                }
                d.set_spacing(s);
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
                            vars[k] = rebind(v);
                            if vars[k].is_none() {
                                *slot = fallback.map_or(*slot, |f| [f.width, f.height][k]);
                            }
                        }
                    }
                }
                n.size_vars = vars;
            }
            // A visibility's binding the same way, its fallback whether the twin
            // was shown (§15 D1059).
            if let Some(v) = n.visible_var {
                n.visible_var = rebind(v);
                if n.visible_var.is_none() {
                    n.visible = twin.map_or(n.visible, |t| t.visible);
                }
            }
            if let Some(m) = n.mode
                && t.mode(m).is_none()
            {
                n.mode = carried
                    .modes
                    .iter()
                    .find(|c| c.id == m)
                    .and_then(|c| t.modes().iter().find(|here| here.name == c.name))
                    .map(|here| here.id);
            }
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
        || n.paint.strokes.iter().any(|s| hit(s.brush.var()))
        || n.corner_vars.iter().any(|v| hit(*v))
        || n.spacing_vars.iter().any(|v| hit(*v))
        || n.size_vars.iter().any(|v| hit(*v))
        || hit(n.visible_var)
}

/// The operations that delete `vars` (D1038): **every field bound to one detaches
/// to the value it drew**, every other variable's alias to one becomes the value
/// it resolved to in that mode, and then the variables go — one transaction, so
/// one `Ctrl+Z` brings back the variables and every binding. The commit refuses
/// a bare `RemoveVariable` of a variable in use (`check`), so this is the only
/// shape a delete of one can take.
pub fn remove_ops(doc: &crate::Document, vars: &[VarId]) -> Vec<crate::Operation> {
    use crate::Operation as O;
    let t = doc.variables();
    let mut ops = Vec::new();
    let mut ids: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| reads(n, vars))
        .map(|n| n.id)
        .collect();
    ids.sort();
    for id in ids {
        let n = doc.get(id).expect("listed from the document");
        let gone = |b: &BrushSource| b.var().is_some_and(|v| vars.contains(&v));
        if n.paint.fills.iter().any(|f| gone(&f.brush)) {
            let fills = n
                .paint
                .fills
                .iter()
                .map(|f| {
                    f.map(|f| crate::Fill {
                        brush: if gone(&f.brush) {
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
        if n.paint.strokes.iter().any(|s| gone(&s.brush)) {
            let strokes = n
                .paint
                .strokes
                .iter()
                .map(|s| {
                    s.map(|s| crate::Stroke {
                        brush: if gone(&s.brush) {
                            doc.resolve_brush(id, &s.brush).into_owned().into()
                        } else {
                            s.brush.clone()
                        },
                        ..s.clone()
                    })
                })
                .collect();
            ops.push(O::SetStrokes { id, strokes });
        }
        if let crate::NodeKind::Rect { corner_radii, .. } = &n.kind
            && n.corner_vars.iter().flatten().any(|v| vars.contains(v))
            && let Some(drawn) = doc.resolve_corners(id, *corner_radii)
        {
            let mut radii = *corner_radii;
            let mut kept = n.corner_vars;
            let slots = [
                (&mut radii.top_left, drawn.top_left),
                (&mut radii.top_right, drawn.top_right),
                (&mut radii.bottom_right, drawn.bottom_right),
                (&mut radii.bottom_left, drawn.bottom_left),
            ];
            for (k, (slot, value)) in slots.into_iter().enumerate() {
                if kept[k].is_some_and(|v| vars.contains(&v)) {
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
        if let Some(d) = &n.display
            && n.spacing_vars.iter().flatten().any(|v| vars.contains(v))
        {
            let drawn = doc.spacing_drawn_in(t, id, d, n.spacing_vars).spacing();
            let mut s = d.spacing();
            let mut kept = n.spacing_vars;
            for k in 0..6 {
                if kept[k].is_some_and(|v| vars.contains(&v)) {
                    s[k] = drawn[k];
                    kept[k] = None;
                }
            }
            let mut display = d.clone();
            display.set_spacing(s);
            ops.push(O::SetDisplay {
                id,
                display: Some(display),
            });
            ops.push(O::SetSpacingVars { id, vars: kept });
        }
        if let Some(size) = n.kind.box_size()
            && n.size_vars.iter().flatten().any(|v| vars.contains(v))
        {
            let drawn = doc
                .size_drawn_in(t, id, &n.kind, n.size_vars)
                .box_size()
                .unwrap_or(size);
            let mut out = [size.width, size.height];
            let mut kept = n.size_vars;
            for k in 0..2 {
                if kept[k].is_some_and(|v| vars.contains(&v)) {
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
        if n.visible_var.is_some_and(|v| vars.contains(&v)) {
            ops.push(O::SetVisible {
                id,
                visible: doc.shown_in(t, id),
            });
            ops.push(O::SetVisibleVar { id, var: None });
        }
    }
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

/// The operations that delete `mode` (D1038): every frame set to it returns to
/// *Inherit*, then the mode and its column go. Refused at the commit when it is
/// the last mode and variables remain.
pub fn remove_mode_ops(doc: &crate::Document, mode: ModeId) -> Vec<crate::Operation> {
    let mut frames: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| n.mode == Some(mode))
        .map(|n| n.id)
        .collect();
    frames.sort();
    let mut ops: Vec<crate::Operation> = frames
        .into_iter()
        .map(|id| crate::Operation::SetMode { id, mode: None })
        .collect();
    ops.push(crate::Operation::RemoveMode { id: mode });
    ops
}

/// The frames set to `mode` — what *Delete mode…* counts in its confirmation.
pub fn frames_set_to(doc: &crate::Document, mode: ModeId) -> usize {
    doc.node_map()
        .values()
        .filter(|n| n.mode == Some(mode))
        .count()
}

/// A copy of mode `from` with its every cell, added at `index` of the list —
/// *Duplicate mode* (after its source) and *New mode* (which copies the first
/// mode and goes last, D1038 G). `None` when `from` is not a mode.
pub fn copy_mode_ops(
    doc: &crate::Document,
    from: ModeId,
    id: ModeId,
    name: &str,
    index: usize,
) -> Option<crate::Operation> {
    let t = doc.variables();
    t.mode(from)?;
    let cells = t
        .iter()
        .filter_map(|v| v.cells.get(&from).map(|c| (v.id, *c)))
        .collect();
    Some(crate::Operation::AddMode {
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells,
    })
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

/// The renames that move every variable in group `from` (and its subgroups) to
/// `to` — *Rename group*, the bindings following because they name ids. `Err`
/// with the first name that would land on a variable already there: G13's merge
/// is not ruled, and G14's refusal of a taken name is the answer taken here.
pub fn rename_group_ops(
    doc: &crate::Document,
    from: &str,
    to: &str,
) -> Result<Vec<crate::Operation>, String> {
    let t = doc.variables();
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
    let mut ops = Vec::new();
    let moving: Vec<VarId> = t
        .iter()
        .filter(|v| moved(&v.name).is_some())
        .map(|v| v.id)
        .collect();
    for v in t.iter() {
        let Some(next) = moved(&v.name) else { continue };
        if t.iter().any(|o| o.name == next && !moving.contains(&o.id)) {
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
    #[error("node {0:?} sets a mode that is not there, or is not a frame")]
    BadMode(NodeId),
    #[error("node {0:?} binds corners it does not have")]
    BadCorners(NodeId),
}

/// The table's own rules, without the bindings.
pub(crate) fn check_table(t: &Variables) -> Result<(), VarRule> {
    if t.modes.is_empty() {
        return if t.vars.is_empty() {
            Ok(())
        } else {
            Err(VarRule::NoModes)
        };
    }
    for (i, m) in t.modes.iter().enumerate() {
        if t.modes[..i].iter().any(|n| n.id == m.id) {
            return Err(VarRule::DuplicateMode(m.id));
        }
    }
    let mut names = std::collections::BTreeSet::new();
    for v in t.vars.values() {
        if !names.insert(v.name.as_str()) {
            return Err(VarRule::DuplicateName(v.name.clone()));
        }
        for m in &t.modes {
            if !v.cells.contains_key(&m.id) {
                return Err(VarRule::MissingCell(v.id, m.id));
            }
        }
        for (mode, cell) in &v.cells {
            if t.mode(*mode).is_none() {
                return Err(VarRule::StrayCell(v.id, *mode));
            }
            if !cell.is_finite() {
                return Err(VarRule::NonFinite(v.id));
            }
            match cell {
                Cell::Alias(target) => {
                    if t.kind_of(*target) != Some(v.kind) {
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
pub(crate) fn check<'a>(
    t: &Variables,
    nodes: impl IntoIterator<Item = &'a crate::Node>,
) -> Result<(), VarRule> {
    check_table(t)?;
    for n in nodes {
        let colour_ok = |b: &BrushSource| match b.var() {
            Some(v) => t.kind_of(v) == Some(VarKind::Color),
            None => true,
        };
        let fills = n.paint.fills.iter().all(|f| colour_ok(&f.brush));
        let strokes = n.paint.strokes.iter().all(|s| colour_ok(&s.brush));
        if !(fills && strokes) {
            return Err(VarRule::BadBinding(n.id));
        }
        if n.corner_vars.iter().any(Option::is_some) {
            if !matches!(n.kind, crate::NodeKind::Rect { .. }) {
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
        if n.visible_var
            .is_some_and(|v| t.kind_of(v) != Some(VarKind::Boolean))
        {
            return Err(VarRule::BadBinding(n.id));
        }
        if let Some(m) = n.mode
            && (t.mode(m).is_none() || !matches!(n.kind, crate::NodeKind::Artboard { .. }))
        {
            return Err(VarRule::BadMode(n.id));
        }
    }
    Ok(())
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
            vars: BTreeMap::new(),
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
