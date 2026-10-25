//! The DOM core: layers to elements and declarations, each in the first of the
//! three tiers that draws it exactly — CSS, inline SVG, PNG.
//!
//! **Every element establishes its layer's local space**: its top-left is the
//! layer's box origin, so a child's numbers are the model's numbers. Every element
//! is positioned (`relative` in a flow, `absolute` out of one) so siblings paint in
//! tree order, as the model's z-order does.

use super::Report;
use super::css::{self, px};
use super::dom::{Arg, Child, Classes, Element, Instance, When};
use super::image;
use super::tokens::{self, Tokens};
use crate::svg::fmt;
use ondin_core::container::{
    self, AlignContent, AlignItems, Dimension, Display, FlexDirection, FlexWrap, GridAutoFlow,
    JustifyContent, LengthPct,
};
use ondin_core::effect::{EFFECT_BLUR, EFFECT_X, EFFECT_Y};
use ondin_core::kurbo::{Affine, Rect, Size};
use ondin_core::peniko::ImageQuality;
use ondin_core::variable::VarId;
use ondin_core::variant::{self, PropKind, PropValue, Property};
use ondin_core::{
    Brush, BrushSource, Document, EffectKind, Fill, ItemId, Node, NodeId, NodeKind, Resolved,
    Stroke, StrokeAlign, StrokeSides, TextSizing, TextStyle,
};
use ondin_core::{component, reset};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// The page the walk builds, and what it saw on the way.
pub(crate) struct Page {
    pub root: Element,
    pub report: Report,
    /// Pictures the page links to, by path under the export's folder.
    pub assets: Vec<(String, Vec<u8>)>,
    /// `tokens.css`, when the page reads a variable.
    pub tokens: Option<String>,
    /// The components the page uses, at any depth, each after those it uses.
    pub components: Vec<Component>,
}

/// A main, or a component set, written as a component.
pub(crate) struct Component {
    /// Its name as code reads it, unique in the export.
    pub name: String,
    /// The folder its file is in, from the export's top: empty for the page,
    /// `components`, or a kit's folder under it.
    pub dir: String,
    /// Each variant's tree with the variant values it is drawn for, the default
    /// variant's last and drawn for any others. A main outside a set, and a set
    /// whose trees merge (`merge::merge`), has one tree, drawn always.
    pub bodies: Vec<(When, Element)>,
    /// Its props: each one's name in code, its type and its default, which a
    /// forwarded prop has none of — the slot it reaches supplies its own.
    pub props: Vec<(String, Ty, Option<Arg>)>,
}

impl Component {
    /// The page as a component of one tree and no properties, named `App`.
    pub fn page(root: Element) -> Self {
        Component {
            name: "App".into(),
            dir: String::new(),
            bodies: vec![(Vec::new(), root)],
            props: Vec::new(),
        }
    }
}

/// A prop's type.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ty {
    Bool,
    Text,
    /// A variant property's values.
    Union(Vec<String>),
    /// A component, which a slot draws.
    Component,
}

/// One parameter of the component made from an owner.
#[derive(Clone)]
struct Param {
    /// Its name in code, unique in the component.
    code: String,
    kind: ParamKind,
    ty: Ty,
    /// Its value on the default tree; `None` for a forwarded one.
    default: Option<PropValue>,
}

/// A layer's place below a tree's root: each layer down to it as its name and
/// which sibling of that name it is.
type NamePath = Vec<(String, usize)>;

/// One parameter a shown slot's use passes on.
#[derive(Clone)]
struct Forwarded {
    /// The slot's component's parameter.
    inner: String,
    /// The main's own, which it takes.
    outer: String,
    ty: Ty,
    /// The slot's value where it differs from what its component draws unset,
    /// which stands in when `outer` is left unset.
    fallback: Option<PropValue>,
}

/// What a parameter reads on an instance.
#[derive(Clone)]
enum ParamKind {
    /// The set's variant property at this index: the variant the instance shows.
    Choice(usize),
    /// A boolean, text or swap property, by item.
    Prop(ItemId, PropKind),
    /// Parameter `inner` of the component made from `owner`, read on the shown
    /// nested instance at `key`, its name path from the instance's root.
    Forward {
        key: NamePath,
        owner: NodeId,
        inner: usize,
    },
}

/// What [`Walk::bound`] knows of a property while its main is written.
#[derive(Clone)]
struct BoundProp {
    name: String,
    kind: PropKind,
    /// Whether the main's text under it has runs or paragraphs one value cannot keep.
    styled: bool,
}

/// How a layer sits in its parent's element.
#[derive(Clone, Copy)]
enum Place {
    /// An item of its parent's flex or grid layout.
    Flow { grid: bool },
    /// Positioned in its parent element's space by `local`, its parent's
    /// transform to it (with `outer` folded in for an export's origins). `boxed`
    /// when the parent has edges for insets to measure from.
    Free { local: Affine, boxed: bool },
    /// A component's root: sized as its main is, placed by whoever uses it.
    Root,
}

/// `origins` as a page. With `components`, an instance of a main is written as a
/// use of a component made from the main, where it can be; without, every
/// instance is written out as its layers.
pub(crate) fn page(doc: &Document, res: &Resolved, origins: &[NodeId], components: bool) -> Page {
    // A kit's mains live in the vault, which is never resolved: a component
    // written from one is drawn from a copy of the document with the vault's
    // tops at the root, which leaves the page's own layers where they are.
    let (placed, placed_res);
    let (doc, res) = match doc.vault().filter(|_| components) {
        Some(vault) => {
            let tops = doc.get(vault).map(|v| v.children().to_vec());
            placed = ondin_core::kit::as_placed(doc, &tops.unwrap_or_default());
            placed_res = Resolved::rebuild(&placed);
            (&placed, &placed_res)
        }
        None => (doc, res),
    };
    let view = crate::extent(res, origins).unwrap_or(Rect::new(0.0, 0.0, 100.0, 100.0));
    let mut walk = Walk {
        doc,
        res,
        classes: Classes::default(),
        report: Report::default(),
        assets: Vec::new(),
        tokens: Tokens::new(doc),
        // The page's name, and the component types React and Vue import.
        components: components.then(|| Components {
            names: ["app", "component", "componenttype"]
                .map(String::from)
                .into(),
            ..Default::default()
        }),
        bound: HashMap::new(),
        forwards: HashMap::new(),
        params_memo: RefCell::default(),
        inline_memo: RefCell::default(),
    };
    let mut root = Element::new("div", walk.classes.mint("page", "page"));
    root.set("position", "relative");
    root.set("width", px(view.width()));
    root.set("height", px(view.height()));
    let offset = Affine::translate(-view.origin().to_vec2());
    for id in origins {
        let Some(node) = doc.get(*id) else { continue };
        // An origin's parent is not in the export: it is placed where it is drawn.
        let parent = node
            .parent()
            .and_then(|p| res.world_transform(p))
            .unwrap_or(Affine::IDENTITY);
        let local = offset * parent * res.used_local_of(node);
        if let Some(el) = walk.node(
            *id,
            Place::Free {
                local,
                boxed: false,
            },
        ) {
            let mut el = el;
            // The mode its ancestors set, which the export does not hold.
            let inherited = node
                .parent()
                .and_then(|p| doc.mode_at(p))
                .filter(|m| Some(*m) != doc.variables().default_mode());
            if node.mode().is_none()
                && let Some(mode) = inherited.and_then(|m| walk.tokens.mode(m))
            {
                if el.instance.is_some() {
                    let mode = mode.to_string();
                    el = walk.with_mode(el, mode);
                } else {
                    el.attrs.push(("data-mode", mode.to_string()));
                }
            }
            root.children.push(Child::Element(Box::new(el)));
        }
    }
    let tokens = walk.tokens.css(doc);
    let components = walk.components.map(|c| c.list).unwrap_or_default();
    Page {
        root,
        report: walk.report,
        assets: walk.assets,
        tokens,
        components,
    }
}

struct Walk<'a> {
    doc: &'a Document,
    res: &'a Resolved,
    /// The classes of the file being written: the page's, or a component's.
    classes: Classes,
    report: Report,
    assets: Vec<(String, Vec<u8>)>,
    tokens: Tokens,
    components: Option<Components>,
    /// The properties of the main being written as a component, by bound layer.
    bound: HashMap<NodeId, BoundProp>,
    /// The shown slots of the main being written, each with what its use
    /// passes on.
    forwards: HashMap<NodeId, Vec<Forwarded>>,
    /// [`Self::params`] by owner, and [`Self::why_inline`] by root: both read
    /// only the document and are asked again at every level of a nesting.
    params_memo: RefCell<HashMap<NodeId, Rc<Vec<Param>>>>,
    inline_memo: RefCell<HashMap<NodeId, Option<String>>>,
}

/// The components made so far.
#[derive(Default)]
struct Components {
    /// By owner: the set for a variant, else the main.
    by_owner: HashMap<NodeId, Signature>,
    /// In the order finished, so a component comes after those it uses.
    list: Vec<Component>,
    /// Component names taken, lowercase ([`unique_file`]).
    names: HashSet<String>,
    /// Each kit's folder under `components`, by kit id.
    kit_dirs: HashMap<String, String>,
}

/// What a use of a component needs to know of it.
#[derive(Clone)]
struct Signature {
    name: String,
    params: Rc<Vec<Param>>,
}

/// What a layer's name reads as in the report: its name in quotes, or its kind.
fn label(node: &Node) -> String {
    if node.name().is_empty() {
        node.kind().default_name().to_lowercase()
    } else {
        format!("“{}”", node.name())
    }
}

impl Walk<'_> {
    /// The element for `id` placed by `place`, or `None` for a layer that draws
    /// nothing here (hidden, or a mask, which its parent's picture carries).
    ///
    /// A layer a boolean property shows is written hidden or not, and shown by it.
    fn node(&mut self, id: NodeId, place: Place) -> Option<Element> {
        let doc = self.doc;
        let node = doc.get(id)?;
        let shown_by = self
            .bound
            .get(&id)
            .filter(|b| b.kind == PropKind::Boolean)
            .map(|b| b.name.clone());
        if node.mask() || (shown_by.is_none() && !doc.shown(id)) {
            return None;
        }
        let mut used = None;
        if self.components.is_some()
            && node.link().is_some()
            && component::instance_root(doc, id) == Some(id)
        {
            match self.why_inline(id) {
                None => used = Some(self.instance(node, place)),
                Some(why) => self
                    .report
                    .inline(format!("instance {}: {why}", label(node))),
            }
        }
        let mut el = match used {
            Some(el) => el,
            None => match self.css_blocker(node) {
                None => self.css_element(node, place),
                Some(why) => self.picture(node, place, &why)?,
            },
        };
        el.shown_by = shown_by;
        el.key = Some(name_step(doc, node));
        Some(el)
    }

    /// Why the instance at `root` cannot be a use of its main's component, or
    /// `None` when it can: when every value it differs from its main in is a
    /// property's, up the chain of copies to the main, apart from its root's
    /// size, which its use sets, and its root's mode, which an element around the
    /// use sets.
    fn why_inline(&self, root: NodeId) -> Option<String> {
        if let Some(why) = self.inline_memo.borrow().get(&root) {
            return why.clone();
        }
        let why = self.why_inline_uncached(root);
        self.inline_memo.borrow_mut().insert(root, why.clone());
        why
    }

    fn why_inline_uncached(&self, root: NodeId) -> Option<String> {
        let doc = self.doc;
        let main = component::main_of(doc, root)?;
        if let Some(why) = self.unwritable(main) {
            return Some(why);
        }
        const OVERRIDES: &str = "inlined because of overrides; make it a property to keep the link";
        // The root's own mode goes on an element around the use, which a mode
        // the component's root sets itself would override.
        let main_mode = doc.get(main)?.mode();
        if main_mode.is_some() && doc.get(root)?.mode() != main_mode {
            return Some(OVERRIDES.into());
        }
        if doc.mode_at(root) != doc.mode_at(main) && self.varies_by_mode(main) {
            return Some(
                "drawn in another mode than its main, whose pictures, strokes, effects, \
                 visibility or content read a variable that changes with the mode"
                    .into(),
            );
        }
        let mut at = root;
        loop {
            let n = doc.get(at)?;
            if n.swap().is_some() {
                return Some(OVERRIDES.into());
            }
            let drift = reset::drift(doc, at);
            if drift.removed + drift.order + drift.local > 0 {
                return Some(OVERRIDES.into());
            }
            // The fields a use can set: those its component's parameters read.
            let mut props: HashSet<(NodeId, PropKind)> = HashSet::new();
            for p in self.params(main).iter() {
                match self.fields(at, &p.kind) {
                    Some(f) => props.extend(f),
                    None => return Some(OVERRIDES.into()),
                }
            }
            for id in reset::scope_nodes(doc, at) {
                if id != at
                    && let Some(swap) = doc.get(id)?.swap()
                    && (!props.contains(&(id, PropKind::Swap)) || self.unwritable(swap).is_some())
                {
                    return Some(OVERRIDES.into());
                }
                for o in reset::overrides(doc, id) {
                    use ondin_core::Operation as O;
                    let allowed = match o.reset {
                        O::SetVisible { .. } | O::SetVisibleVar { .. } => {
                            id == at || props.contains(&(id, PropKind::Boolean))
                        }
                        O::SetText { .. } => props.contains(&(id, PropKind::Text)),
                        O::SetSwap { .. } => props.contains(&(id, PropKind::Swap)),
                        O::SetGeometry { .. } | O::SetMode { .. } => id == at,
                        _ => false,
                    };
                    if !allowed {
                        return Some(OVERRIDES.into());
                    }
                }
            }
            let src = reset::source_of(doc, at)?;
            if src == main {
                return None;
            }
            at = src;
        }
    }

    /// Why no component can be made from `main`, or `None` when one can.
    fn unwritable(&self, main: NodeId) -> Option<String> {
        let doc = self.doc;
        if let Some(set) = variant::set_of(doc, main)
            && variant::find_variant(doc, set, doc.get(main)?.variant()) != Some(main)
        {
            return Some("its variant has the values of another, which is written".into());
        }
        // Every variant is a tree of the component, so each must be a CSS root.
        let trees = self.trees(main).0;
        for t in &trees {
            if self.css_blocker(doc.get(*t)?).is_some() {
                return Some("its main is drawn as a picture".into());
            }
        }
        // A use cannot reach into a picture, which holds the main's values.
        let owner = variant::owner_of_main(doc, main).unwrap_or(main);
        let mut bound: HashMap<NodeId, Vec<PropKind>> = HashMap::new();
        for p in doc.get(owner)?.props() {
            for b in &p.bound {
                bound.entry(*b).or_default().push(p.kind);
            }
        }
        for t in &trees {
            for c in doc.get(*t)?.children() {
                if self.pictures_bound(*c, &bound) {
                    return Some("its main draws a property's layer as a picture".into());
                }
            }
        }
        None
    }

    /// Whether the layer at `id`, or one under it written as it is, is a picture
    /// holding a layer a property is `bound` to — but for a boolean on the
    /// picture itself, which shows the picture's element.
    fn pictures_bound(&self, id: NodeId, bound: &HashMap<NodeId, Vec<PropKind>>) -> bool {
        let doc = self.doc;
        let Some(n) = doc.get(id) else {
            return false;
        };
        // A use is its own component's to write.
        if n.link().is_some()
            && component::instance_root(doc, id) == Some(id)
            && self.why_inline(id).is_none()
        {
            return false;
        }
        if self.css_blocker(n).is_none() {
            return n.children().iter().any(|c| self.pictures_bound(*c, bound));
        }
        let shows = bound
            .get(&id)
            .is_some_and(|k| k.iter().all(|k| *k == PropKind::Boolean));
        let mut under = vec![id];
        while let Some(at) = under.pop() {
            if (at != id || !shows) && bound.contains_key(&at) {
                return true;
            }
            under.extend(doc.get(at).map(Node::children).unwrap_or_default());
        }
        false
    }

    /// Whether a component written from `main` in its own mode would draw wrong
    /// in another: a picture reading a variable that changes with the mode, which
    /// the picture bakes, or a layer whose stroke, effects, visibility or content
    /// read one, which decide what CSS is written rather than a `var()` in it.
    fn varies_by_mode(&self, main: NodeId) -> bool {
        let doc = self.doc;
        let varies =
            |vars: &[Option<VarId>]| vars.iter().flatten().any(|v| tokens::varies(doc, *v));
        let mut under: Vec<(NodeId, bool)> =
            self.trees(main).0.into_iter().map(|t| (t, false)).collect();
        while let Some((at, pictured)) = under.pop() {
            let Some(n) = doc.get(at) else { continue };
            let pictured = pictured || self.css_blocker(n).is_some();
            let paint = n.paint();
            let mut structural: Vec<Option<VarId>> = vec![n.visible_var(), n.text_vars()[0]];
            structural.extend(paint.strokes.iter().map(|s| s.width_var));
            structural.extend(n.effects().iter().flat_map(|e| e.vars));
            if varies(&structural) {
                return true;
            }
            if pictured {
                let mut drawn: Vec<Option<VarId>> = vec![n.opacity_var()];
                drawn.extend(&n.text_vars()[1..]);
                drawn.extend(n.item().limit_vars);
                drawn.extend(n.size_vars());
                drawn.extend(n.corner_vars());
                drawn.extend(n.spacing_vars());
                for b in paint
                    .fills
                    .iter()
                    .map(|f| &f.brush)
                    .chain(paint.strokes.iter().map(|s| &s.brush))
                {
                    if let BrushSource::Bound { var, .. } = b {
                        drawn.push(Some(*var));
                    }
                }
                if varies(&drawn) {
                    return true;
                }
            }
            under.extend(n.children().iter().map(|c| (*c, pictured)));
        }
        false
    }

    /// The trees the component made from `main` draws — its set's variants, the
    /// first of any that share their values, or the main alone — and its default:
    /// the set's default variant, else the main.
    fn trees(&self, main: NodeId) -> (Vec<NodeId>, NodeId) {
        let doc = self.doc;
        let Some(set) = variant::set_of(doc, main) else {
            return (vec![main], main);
        };
        let mut seen = HashSet::new();
        let trees: Vec<NodeId> = variant::variants(doc, set)
            .into_iter()
            .filter(|v| seen.insert(doc.get(*v).map(|n| n.variant().to_vec())))
            .collect();
        let default = ondin_core::kit::default_variant(doc, set)
            .filter(|d| trees.contains(d))
            .unwrap_or(main);
        (trees, default)
    }

    /// Property `p`'s default: its value on the default tree, where it reaches it.
    fn prop_default(&self, p: &Property, default: NodeId) -> Option<PropValue> {
        let doc = self.doc;
        let b = p
            .bound
            .iter()
            .find(|b| under(doc, **b, default))
            .or(p.bound.first())?;
        variant::field_value(doc, *b, p.kind)
    }

    /// The main a swap property's slots show, when the property can be a prop:
    /// every slot it is bound to shows that main, as a use setting nothing.
    fn swap_default(&self, p: &Property) -> Option<NodeId> {
        let doc = self.doc;
        let mut shown = None;
        for b in &p.bound {
            let m = component::main_of(doc, *b)?;
            if shown.is_some_and(|s| s != m)
                || component::instance_root(doc, *b) != Some(*b)
                || self.why_inline(*b).is_some()
                || !self.differences(*b).is_empty()
            {
                return None;
            }
            shown = Some(m);
        }
        shown
    }

    /// The parameters of the component made from `main`: its set's variant
    /// properties, its boolean and text properties, its swap properties that a
    /// use can set, and the parameters of each nested instance it shows,
    /// forwarded under the slot's name.
    fn params(&self, main: NodeId) -> Rc<Vec<Param>> {
        let owner = variant::owner_of_main(self.doc, main).unwrap_or(main);
        if let Some(p) = self.params_memo.borrow().get(&owner) {
            return p.clone();
        }
        let p = Rc::new(self.params_uncached(main));
        self.params_memo.borrow_mut().insert(owner, p.clone());
        p
    }

    fn params_uncached(&self, main: NodeId) -> Vec<Param> {
        let doc = self.doc;
        let owner_id = variant::owner_of_main(doc, main).unwrap_or(main);
        let Some(owner) = doc.get(owner_id) else {
            return Vec::new();
        };
        let (trees, default) = self.trees(main);
        // What a component's props object already holds.
        let mut taken: HashSet<String> = ["className", "key", "ref", "children"]
            .map(String::from)
            .into();
        let mut out = Vec::new();
        let default_values = doc.get(default).map(Node::variant).unwrap_or_default();
        for (i, (vp, d)) in owner
            .set()
            .map(|s| s.props.as_slice())
            .unwrap_or_default()
            .iter()
            .zip(default_values)
            .enumerate()
        {
            out.push(Param {
                code: unique(prop_name(&vp.name), &mut taken),
                kind: ParamKind::Choice(i),
                ty: Ty::Union(vp.values.clone()),
                default: Some(PropValue::Text(d.clone())),
            });
        }
        for p in owner.props() {
            let (ty, value) = match p.kind {
                PropKind::Boolean => (Ty::Bool, self.prop_default(p, default)),
                PropKind::Text => (Ty::Text, self.prop_default(p, default)),
                PropKind::Swap => (Ty::Component, self.swap_default(p).map(PropValue::Swap)),
                PropKind::Nested => continue,
            };
            let Some(value) = value else { continue };
            out.push(Param {
                code: unique(prop_name(&p.name), &mut taken),
                kind: ParamKind::Prop(p.id, p.kind),
                ty,
                default: Some(value),
            });
        }
        // The shown slots, by name path, in every tree.
        let Some(showing) = owner.props().iter().find(|p| p.kind == PropKind::Nested) else {
            return out;
        };
        let mut keys: Vec<(NamePath, Vec<NodeId>)> = Vec::new();
        for b in &showing.bound {
            let Some(t) = trees.iter().find(|t| under(doc, *b, **t)) else {
                continue;
            };
            let key = name_path(doc, *t, *b);
            match keys.iter_mut().find(|(k, _)| *k == key) {
                Some((_, slots)) => slots.push(*b),
                None => keys.push((key, vec![*b])),
            }
        }
        let swapped: HashSet<NodeId> = owner
            .props()
            .iter()
            .filter(|p| p.kind == PropKind::Swap)
            .flat_map(|p| p.bound.iter().copied())
            .collect();
        'keys: for (key, slots) in keys {
            // Every slot at the path a use this component can make, of one
            // component, and none of them a swap property's, whose component
            // the use chooses.
            let mut inner_owner = None;
            for s in &slots {
                let Some(m) = component::main_of(doc, *s) else {
                    continue 'keys;
                };
                let o = variant::owner_of_main(doc, m).unwrap_or(m);
                if inner_owner.is_some_and(|i| i != o)
                    || swapped.contains(s)
                    || self.why_inline(*s).is_some()
                {
                    continue 'keys;
                }
                inner_owner = Some(o);
            }
            let (Some(inner_owner), Some(first)) = (inner_owner, slots.first()) else {
                continue;
            };
            let slot_name = doc.get(*first).map(Node::name).unwrap_or_default();
            let inner = component::main_of(doc, *first).map(|m| self.params(m));
            for (j, ip) in inner.iter().flat_map(|p| p.iter()).enumerate() {
                out.push(Param {
                    code: unique(prop_name(&format!("{slot_name} {}", ip.code)), &mut taken),
                    kind: ParamKind::Forward {
                        key: key.clone(),
                        owner: inner_owner,
                        inner: j,
                    },
                    ty: ip.ty.clone(),
                    default: None,
                });
            }
        }
        out
    }

    /// The value parameter `kind` reads on the instance at `root`, or `None`
    /// where it reaches nothing there.
    fn read(&self, root: NodeId, kind: &ParamKind) -> Option<PropValue> {
        let doc = self.doc;
        match kind {
            ParamKind::Choice(i) => {
                let main = component::main_of(doc, root)?;
                doc.get(main)?
                    .variant()
                    .get(*i)
                    .map(|v| PropValue::Text(v.clone()))
            }
            ParamKind::Prop(item, k) => {
                let props = variant::instance_properties(doc, root);
                let p = props.iter().find(|p| p.id == *item)?;
                let c = *variant::counterparts(doc, root, p).first()?;
                variant::field_value(doc, c, *k)
            }
            ParamKind::Forward { key, owner, inner } => {
                let copy = self.shown_copy(root, key)?;
                let m = component::main_of(doc, copy)?;
                if variant::owner_of_main(doc, m).unwrap_or(m) != *owner {
                    return None;
                }
                let kind = self.params(m).get(*inner)?.kind.clone();
                self.read(copy, &kind)
            }
        }
    }

    /// The value the instance at `root` draws for parameter `p` when its use
    /// passes nothing: the default, or for a forwarded one, its slot's value in
    /// the tree the instance shows.
    fn expected(&self, root: NodeId, p: &Param) -> Option<PropValue> {
        match &p.kind {
            ParamKind::Forward { key, inner, .. } => {
                let tree = component::main_of(self.doc, root)?;
                let slot = at_name_path(self.doc, tree, key)?;
                let m = component::main_of(self.doc, slot)?;
                let kind = self.params(m).get(*inner)?.kind.clone();
                self.read(slot, &kind)
            }
            _ => p.default.clone(),
        }
    }

    /// The fields parameter `kind` reads on the instance at `root`, which its
    /// use sets rather than overriding, or `None` when the instance holds a
    /// value the parameter cannot say: a shown copy swapped to another
    /// component.
    fn fields(&self, root: NodeId, kind: &ParamKind) -> Option<Vec<(NodeId, PropKind)>> {
        let doc = self.doc;
        match kind {
            // The instance's own variant is its link, never an override.
            ParamKind::Choice(_) => Some(Vec::new()),
            ParamKind::Prop(item, k) => {
                let props = variant::instance_properties(doc, root);
                let Some(p) = props.iter().find(|p| p.id == *item) else {
                    return Some(Vec::new());
                };
                Some(
                    variant::counterparts(doc, root, p)
                        .into_iter()
                        .map(|c| (c, *k))
                        .collect(),
                )
            }
            ParamKind::Forward { key, owner, inner } => {
                let Some(copy) = self.shown_copy(root, key) else {
                    return Some(Vec::new());
                };
                let m = component::main_of(doc, copy)?;
                if variant::owner_of_main(doc, m).unwrap_or(m) != *owner {
                    return None;
                }
                let kind = self.params(m).get(*inner)?.kind.clone();
                let mut f = self.fields(copy, &kind)?;
                // A shown copy's variant switch is a swap within its set.
                if matches!(kind, ParamKind::Choice(_)) {
                    f.push((copy, PropKind::Swap));
                }
                Some(f)
            }
        }
    }

    /// The copy of the slot at `key` that the instance at `root` shows.
    fn shown_copy(&self, root: NodeId, key: &[(String, usize)]) -> Option<NodeId> {
        variant::shown_nested(self.doc, root)
            .into_iter()
            .find(|s| s.path.len() == 1 && s.key == key)
            .map(|s| s.copy)
    }

    /// What the instance at `root` sets apart from what its component draws by
    /// default, by parameter index.
    fn differences(&self, root: NodeId) -> Vec<(usize, PropValue)> {
        let Some(main) = component::main_of(self.doc, root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (i, p) in self.params(main).iter().enumerate() {
            if let Some(v) = self.read(root, &p.kind)
                && Some(&v) != self.expected(root, p).as_ref()
            {
                out.push((i, v));
            }
        }
        out
    }

    /// `v` as an argument, making the component a swap's value names.
    fn arg(&mut self, v: PropValue) -> Arg {
        match v {
            PropValue::Boolean(b) => Arg::Bool(b),
            PropValue::Text(t) => Arg::Text(t),
            PropValue::Swap(m) => Arg::Component(self.component(m).name),
        }
    }

    /// The instance at `node` as a use of its main's component, placed as `place`
    /// says, with the variant and the properties it sets apart from the defaults.
    fn instance(&mut self, node: &Node, place: Place) -> Element {
        let doc = self.doc;
        let main = component::main_of(doc, node.id()).expect("checked by why_inline");
        let sig = self.component(main);
        let kind = self.res.used_kind_of(node).clone();
        let mut el = Element::new("div", self.classes.mint(node.name(), kind.default_name()));
        let bx = self.box_rect(node);
        self.place(&mut el, node, &kind, bx, place);
        let mut args = Vec::new();
        match self.forwards.get(&node.id()).cloned() {
            // A shown slot passes on the main's own parameters, its value where
            // it differs standing in for one left unset.
            Some(forwarded) => {
                for f in forwarded {
                    let fallback = f.fallback.map(|v| Box::new(self.arg(v)));
                    let var = Arg::Var {
                        name: f.outer,
                        component: f.ty == Ty::Component,
                        fallback,
                    };
                    args.push((f.inner, var));
                }
            }
            None => {
                for (i, v) in self.differences(node.id()) {
                    let arg = self.arg(v);
                    args.push((sig.params[i].code.clone(), arg));
                }
            }
        }
        let slot = self
            .bound
            .get(&node.id())
            .filter(|b| b.kind == PropKind::Swap)
            .map(|b| b.name.clone());
        el.instance = Some(Instance {
            component: sig.name,
            args,
            slot,
        });
        // `why_inline` let a root's own mode through only where the main sets none.
        let own = node
            .mode()
            .filter(|m| doc.get(main).and_then(Node::mode) != Some(*m));
        match own.and_then(|m| self.tokens.mode(m)) {
            Some(mode) => {
                let mode = mode.to_string();
                self.with_mode(el, mode)
            }
            None => el,
        }
    }

    /// `used` inside an element that draws no box and sets `mode`: a component
    /// use writes no attributes of its own.
    fn with_mode(&mut self, used: Element, mode: String) -> Element {
        let mut around = Element::new("div", self.classes.mint("mode", "mode"));
        around.set("display", "contents");
        around.attrs.push(("data-mode", mode));
        around.children.push(Child::Element(Box::new(used)));
        around
    }

    /// The component written from `main`, or from the set it is a variant of,
    /// made the first time it is asked for.
    fn component(&mut self, main: NodeId) -> Signature {
        let doc = self.doc;
        let owner_id = variant::owner_of_main(doc, main).unwrap_or(main);
        let comps = self.components.as_mut().expect("components are on");
        if let Some(sig) = comps.by_owner.get(&owner_id) {
            return sig.clone();
        }
        let owner = doc.get(owner_id).expect("a main in the document");
        let name = unique_file(component_name(owner.name()), "Component", &mut comps.names);
        // A kit's components go in a folder named for the kit.
        let dir = match owner.origin() {
            None => "components".to_string(),
            Some(o) => {
                if !comps.kit_dirs.contains_key(&o.kit) {
                    let named = o.name.clone().or_else(|| {
                        let kits = doc.variables().kits();
                        kits.get(&o.kit).and_then(|k| k.name.clone())
                    });
                    let mut taken = comps
                        .kit_dirs
                        .values()
                        .map(|d| d.to_ascii_lowercase())
                        .collect();
                    let folder = unique_file(
                        component_name(named.as_deref().unwrap_or("Kit")),
                        "Kit",
                        &mut taken,
                    );
                    comps.kit_dirs.insert(o.kit.clone(), folder);
                }
                format!("components/{}", comps.kit_dirs[&o.kit])
            }
        };
        let params = self.params(main);
        let sig = Signature {
            name: name.clone(),
            params: params.clone(),
        };
        let comps = self.components.as_mut().expect("components are on");
        comps.by_owner.insert(owner_id, sig.clone());

        // The defaults as arguments, which makes the components they name first,
        // so they are written before this one.
        let mut props = Vec::new();
        for p in params.iter() {
            let default = p.default.clone().map(|v| self.arg(v));
            props.push((p.code.clone(), p.ty.clone(), default));
        }

        let (trees, default) = self.trees(main);
        let mut bound = HashMap::new();
        let mut forwards: HashMap<NodeId, Vec<_>> = HashMap::new();
        for p in params.iter() {
            match &p.kind {
                ParamKind::Prop(item, kind) => {
                    let Some(prop) = owner.props().iter().find(|q| q.id == *item) else {
                        continue;
                    };
                    for b in &prop.bound {
                        let styled = match doc.get(*b).map(Node::kind) {
                            Some(NodeKind::Text { content, spans, .. }) => {
                                content.contains('\n') || !spans.is_empty()
                            }
                            _ => false,
                        };
                        bound.insert(
                            *b,
                            BoundProp {
                                name: p.code.clone(),
                                kind: *kind,
                                styled,
                            },
                        );
                    }
                }
                ParamKind::Forward { key, inner, .. } => {
                    for t in &trees {
                        let Some(slot) = at_name_path(doc, *t, key) else {
                            continue;
                        };
                        let Some(m) = component::main_of(doc, slot) else {
                            continue;
                        };
                        let Some(ip) = self.params(m).get(*inner).cloned() else {
                            continue;
                        };
                        let fallback = self
                            .differences(slot)
                            .into_iter()
                            .find(|(i, _)| i == inner)
                            .map(|(_, v)| v);
                        forwards.entry(slot).or_default().push(Forwarded {
                            inner: ip.code,
                            outer: p.code.clone(),
                            ty: p.ty.clone(),
                            fallback,
                        });
                    }
                }
                ParamKind::Choice(_) => {}
            }
        }

        // One file's classes across every variant's tree.
        let classes = std::mem::take(&mut self.classes);
        let outer_bound = std::mem::replace(&mut self.bound, bound);
        let outer_forwards = std::mem::replace(&mut self.forwards, forwards);
        let mut bodies = Vec::new();
        // The default's tree is walked first, so its classes take the plain
        // names, and moved last, where it is the fallback.
        for t in [&default]
            .into_iter()
            .chain(trees.iter().filter(|t| **t != default))
        {
            let Some(node) = doc.get(*t) else { continue };
            let mut root = self.css_element(node, Place::Root);
            root.takes_class = true;
            let when = match *t == default {
                true => Vec::new(),
                false => params
                    .iter()
                    .filter_map(|p| match p.kind {
                        ParamKind::Choice(i) => {
                            Some((p.code.clone(), node.variant().get(i)?.clone()))
                        }
                        _ => None,
                    })
                    .collect(),
            };
            bodies.push((when, root));
        }
        bodies.rotate_left(1);
        self.bound = outer_bound;
        self.forwards = outer_forwards;
        self.classes = classes;

        // Pushed once finished, so after the components it uses.
        let comps = self.components.as_mut().expect("components are on");
        comps.list.push(Component {
            name,
            dir,
            bodies: super::merge::merge(bodies),
            props,
        });
        sig
    }

    /// Why `node` cannot be a CSS element, or `None` when it can. Only the
    /// layer's own features count: its children choose their own tier.
    fn css_blocker(&self, node: &Node) -> Option<String> {
        let doc = self.doc;
        let id = node.id();
        let kind = self.res.used_kind_of(node);
        match kind {
            NodeKind::Artboard { .. } | NodeKind::Group | NodeKind::Rect { .. } => {}
            NodeKind::Ellipse { .. } => {}
            NodeKind::Text { on_path, .. } => {
                if on_path.is_some() {
                    return Some("type on a path".into());
                }
            }
            NodeKind::Polygon { .. } => return Some("a polygon".into()),
            NodeKind::Star { .. } => return Some("a star".into()),
            NodeKind::Line { .. } => return Some("a line".into()),
            NodeKind::Path { .. } => return Some("a vector path".into()),
            NodeKind::Boolean { .. } => return Some("a boolean".into()),
            NodeKind::Root => return Some("the document".into()),
        }
        if node
            .children()
            .iter()
            .any(|c| doc.get(*c).is_some_and(Node::mask) && doc.shown(*c))
        {
            return Some("a mask".into());
        }
        // As drawn, so a gradient layout stretched is judged in its drawn form.
        let paint = ondin_core::paint_box::node_paint(doc, self.res, id)
            .unwrap_or_else(|| doc.resolved_paint(id, node.paint()));
        let fills: Vec<&Fill> = paint
            .fills
            .iter()
            .filter(|f| f.visible)
            .map(|f| &f.value)
            .collect();
        let strokes: Vec<&Stroke> = paint
            .strokes
            .iter()
            .filter(|s| s.visible && s.width > 0.0)
            .map(|s| &s.value)
            .collect();
        let size = self.box_rect(node).map(|r| r.size()).unwrap_or_default();
        if let NodeKind::Text { .. } = kind {
            match fills.as_slice() {
                [] => {}
                [f] if matches!(f.brush.drawn(), Brush::Solid(_)) => {}
                _ => return Some("text painted with more than one solid colour".into()),
            }
            if !strokes.is_empty() {
                return Some("stroked text".into());
            }
        } else if !matches!(kind, NodeKind::Group) {
            // `image-rendering` is the element's, so its pictures must agree on it.
            let mut nearest = fills.iter().filter_map(|f| match f.brush.drawn() {
                Brush::Image(img) => Some(img.sampler.quality == ImageQuality::Low),
                _ => None,
            });
            if let Some(first) = nearest.next()
                && nearest.any(|n| n != first)
            {
                return Some("pictures sampled two ways".into());
            }
            for f in &fills {
                match f.brush.drawn() {
                    Brush::Image(img) => {
                        if let Err(why) = image::layer(doc, img, size) {
                            return Some(why);
                        }
                    }
                    b => {
                        if css::background_layer(b, size).is_none() {
                            return Some("a gradient CSS cannot draw".into());
                        }
                    }
                }
            }
            match strokes.as_slice() {
                [] => {}
                [s] => {
                    if let Some(why) = self.stroke_blocker(node, kind, s) {
                        return Some(why);
                    }
                }
                _ => return Some("more than one stroke".into()),
            }
        }
        let effects = doc.resolved_effects(id, node.effects());
        let mut shadows = 0;
        for e in effects.iter().filter(|e| e.visible) {
            match &e.kind {
                EffectKind::LayerBlur { .. } => {}
                EffectKind::DropShadow(s) if s.spread == 0.0 => shadows += 1,
                EffectKind::DropShadow(_) => return Some("a shadow with spread".into()),
                EffectKind::InnerShadow(_) => return Some("an inner shadow".into()),
                EffectKind::Filters(_) => return Some("colour filters".into()),
            }
        }
        // CSS chains `drop-shadow()`s, each casting the last one's shadow too.
        if shadows > 1 {
            return Some("more than one drop shadow".into());
        }
        None
    }

    /// Why one stroke has no CSS form here, or `None`. All four sides are an
    /// `outline` offset to the alignment ([`Self::paint`]); per-side widths are
    /// `border`s, which only an inside stroke on a childless rect matches — a
    /// border moves the padding box absolutely positioned children are measured
    /// from.
    fn stroke_blocker(&self, node: &Node, kind: &NodeKind, s: &Stroke) -> Option<String> {
        if !matches!(s.brush.drawn(), Brush::Solid(_)) {
            return Some("a gradient or image stroke".into());
        }
        if !s.dashes.is_empty() {
            return Some("a dashed stroke".into());
        }
        // An outline offsets an ellipse's radii, which is its parallel curve
        // only for a circle.
        if let NodeKind::Ellipse { size } = kind
            && size.width != size.height
        {
            return Some("a stroked ellipse that is not a circle".into());
        }
        let (square, any_square) = match kind {
            NodeKind::Rect { corner_radii, .. } => (
                ondin_core::geometry::is_square_cornered(corner_radii),
                [
                    corner_radii.top_left,
                    corner_radii.top_right,
                    corner_radii.bottom_right,
                    corner_radii.bottom_left,
                ]
                .iter()
                .any(|r| *r <= 0.0),
            ),
            NodeKind::Artboard { .. } => (true, true),
            _ => (false, false),
        };
        let sides = ondin_core::geometry::effective_sides(kind, s);
        if !sides.is_all() {
            // Through a rounded corner CSS tapers one width into the other where
            // the canvas splits the arc between the sides.
            if s.align != StrokeAlign::Inside || !node.children().is_empty() || !square {
                return Some("a per-side stroke CSS borders cannot draw".into());
            }
            return None;
        }
        // An outline's corners are mitred where the radius is zero; the model
        // joins them as asked.
        let mitred =
            s.join == ondin_core::kurbo::Join::Miter && s.miter_limit >= std::f64::consts::SQRT_2;
        if any_square && s.align != StrokeAlign::Inside && !mitred {
            return Some("a stroke with rounded or bevelled corners".into());
        }
        None
    }

    /// `node`'s box in its own space, or `None` for a group without a layout,
    /// which has none.
    fn box_rect(&self, node: &Node) -> Option<Rect> {
        let kind = self.res.used_kind_of(node);
        match kind {
            NodeKind::Group => self
                .res
                .used_frame(node.id())
                .filter(|_| node.display().is_some())
                .map(|s| Rect::from_origin_size((0.0, 0.0), s)),
            _ => ondin_core::geometry::local_bounds(kind, self.res.text_layout(node.id())),
        }
    }

    /// Tier 1: `node` as an element with declarations, children recursed.
    fn css_element(&mut self, node: &Node, place: Place) -> Element {
        let doc = self.doc;
        let id = node.id();
        let kind = self.res.used_kind_of(node).clone();
        let mut el = Element::new("div", self.classes.mint(node.name(), kind.default_name()));
        let bx = self.box_rect(node);
        self.place(&mut el, node, &kind, bx, place);
        if let Some(mode) = node.mode().and_then(|m| self.tokens.mode(m)) {
            el.attrs.push(("data-mode", mode.to_string()));
        }
        if let Some(var) = node.visible_var()
            && tokens::varies(doc, var)
        {
            self.report.approximate(format!(
                "{} {}: its visibility is bound to a variable and written for one mode",
                kind_word(node),
                label(node)
            ));
        }

        if let Some(var) = node.opacity_var() {
            el.set("opacity", self.tokens.opacity(var));
        } else {
            let opacity = doc.drawn_opacity(id);
            if opacity < 1.0 {
                el.set("opacity", fmt(f64::from(opacity.max(0.0))));
            }
        }
        self.effects(&mut el, node);

        let mut over_children = None;
        match &kind {
            NodeKind::Rect { corner_radii, size } => {
                let half = size.width.min(size.height).max(0.0) * 0.5;
                // Clamped per corner as the outline is, so CSS's own proportional
                // scaling never has an overlap to resolve — against the bound size
                // when there is one, since it changes with the mode.
                let sized = node.size_vars().iter().any(Option::is_some);
                let half_css = if sized {
                    let w = self.axis_size(node, 0, size.width);
                    let h = self.axis_size(node, 1, size.height);
                    format!("calc(min({w}, {h}) / 2)")
                } else {
                    px(half)
                };
                let bound = node.corner_vars();
                let r = [
                    corner_radii.top_left,
                    corner_radii.top_right,
                    corner_radii.bottom_right,
                    corner_radii.bottom_left,
                ];
                if r.iter().any(|r| *r > 0.0) || bound.iter().any(Option::is_some) {
                    let all: Vec<String> = r
                        .iter()
                        .zip(bound)
                        .map(|(r, var)| match var {
                            Some(v) => format!("min({}, {half_css})", self.tokens.px_floored(v)),
                            None if sized && *r > 0.0 => format!("min({}, {half_css})", px(*r)),
                            None => px(r.clamp(0.0, half)),
                        })
                        .collect();
                    el.set(
                        "border-radius",
                        if all.iter().all(|x| *x == all[0]) {
                            all[0].clone()
                        } else {
                            all.join(" ")
                        },
                    );
                }
                self.paint(&mut el, node, *size);
            }
            NodeKind::Ellipse { size } => {
                el.set("border-radius", "50%");
                self.paint(&mut el, node, *size);
            }
            NodeKind::Artboard { size } => {
                if node.clip() {
                    el.set("overflow", "hidden");
                }
                over_children = self.paint(&mut el, node, *size);
            }
            NodeKind::Text { .. } => {
                self.text(&mut el, node);
                if let Some(b) = self.bound.get(&id).filter(|b| b.kind == PropKind::Text) {
                    // One value in the first paragraph's style, which is what the
                    // canvas does with a value that is not the main's.
                    if b.styled {
                        self.report.approximate(format!(
                            "text {}: its property is written in one style, which the \
                             main's own text has more than",
                            label(node)
                        ));
                    }
                    el.children.truncate(1);
                    if let NodeKind::Text {
                        content,
                        style,
                        spans,
                        ..
                    } = &kind
                    {
                        let first = spans.runs(style, content.len()).into_iter().next();
                        let s = first.map_or_else(|| (**style).clone(), |(_, s)| s);
                        let value =
                            self.run(Child::Prop(b.name.clone()), &s, style, node, &el.class);
                        if let Some(Child::Element(p)) = el.children.first_mut() {
                            // The value is cased as it arrives, by CSS's rules.
                            if let Some(t) = text_transform(s.case) {
                                p.set("text-transform", t);
                                self.report.approximate(format!(
                                    "text {}: its property is cased by CSS's `text-transform`, \
                                     which cases some letters differently",
                                    label(node)
                                ));
                            }
                            p.children = vec![value];
                        }
                    }
                }
            }
            _ => {}
        }

        if let Some(display) = doc.resolved_display(id)
            && container::is_container(&kind)
        {
            self.display(&mut el, &display, node.spacing_vars());
        } else if matches!(kind, NodeKind::Group) {
            self.report.position(format!(
                "group {}: no layout, so its children are absolutely positioned",
                label(node)
            ));
        }

        let lays_out = node.display().is_some() && container::is_container(&kind);
        let grid = matches!(node.display(), Some(Display::Grid(_)));
        let boxed = matches!(kind, NodeKind::Artboard { .. }) || lays_out;
        for child in node.children() {
            let Some(c) = doc.get(*child) else { continue };
            let in_flow = lays_out && !c.insets().is_authored();
            let place = if in_flow {
                Place::Flow { grid }
            } else {
                // Where layout put it: a picture is placed by this alone, and an
                // inset-pinned layer's stored transform is stale.
                Place::Free {
                    local: self.res.used_local_of(c),
                    boxed,
                }
            };
            if let Some(e) = self.node(*child, place) {
                el.children.push(Child::Element(Box::new(e)));
            }
        }
        if let Some(e) = over_children {
            el.children.push(Child::Element(Box::new(e)));
        }
        el
    }

    /// Position and size `el` as `place` says. The box's slot is its top-left
    /// before its own transform, which turns it about its centre, CSS's default
    /// `transform-origin` — a group without a layout has no box, so it turns about
    /// its own origin.
    fn place(
        &mut self,
        el: &mut Element,
        node: &Node,
        kind: &NodeKind,
        bx: Option<Rect>,
        place: Place,
    ) {
        let item = node.item();
        let bx_or_point = bx.unwrap_or(Rect::ZERO);
        match place {
            Place::Flow { grid } => {
                el.set("position", "relative");
                self.sizes(el, node, kind, bx, (false, false));
                if !grid {
                    if item.grow != 0.0 {
                        el.set("flex-grow", fmt(item.grow));
                    }
                    if item.shrink != 1.0 {
                        el.set("flex-shrink", fmt(item.shrink));
                    }
                    if item.basis != Dimension::Auto {
                        el.set("flex-basis", dimension(item.basis));
                    }
                } else {
                    if item.grid_column != Default::default() {
                        el.set(
                            "grid-column",
                            format!(
                                "{} / {}",
                                container::placement_css(item.grid_column.start),
                                container::placement_css(item.grid_column.end)
                            ),
                        );
                    }
                    if item.grid_row != Default::default() {
                        el.set(
                            "grid-row",
                            format!(
                                "{} / {}",
                                container::placement_css(item.grid_row.start),
                                container::placement_css(item.grid_row.end)
                            ),
                        );
                    }
                    if let Some(j) = item.justify_self {
                        el.set("justify-self", align_items(j));
                    }
                }
                if let Some(a) = item.align_self {
                    el.set("align-self", align_items(a));
                }
                // A shape is a replaced element to the layout: it never shrinks or
                // is squeezed below its own size, which an empty `div` would be.
                if replaced(node, kind)
                    && let Some(b) = bx
                {
                    if item.min_width == Dimension::Auto {
                        let w = self.axis_size(node, 0, b.width());
                        el.set("min-width", w);
                    }
                    if item.min_height == Dimension::Auto {
                        let h = self.axis_size(node, 1, b.height());
                        el.set("min-height", h);
                    }
                }
                if let Some(m) = css::matrix(css::linear(node.transform())) {
                    el.set("transform", m);
                }
            }
            Place::Free { local, boxed } => {
                el.set("position", "absolute");
                let slot = container::slot_of(local, bx_or_point);
                let insets = node.insets();
                let insets_apply = boxed && insets.is_authored() && container::takes_insets(kind);
                let mut stretched = (false, false);
                if insets_apply {
                    stretched.0 = axis(el, "left", "right", insets.left, insets.right, slot.x);
                    stretched.1 = axis(el, "top", "bottom", insets.top, insets.bottom, slot.y);
                    let m = insets.margin_auto;
                    for (on, prop) in [
                        (m.top, "margin-top"),
                        (m.right, "margin-right"),
                        (m.bottom, "margin-bottom"),
                        (m.left, "margin-left"),
                    ] {
                        if on {
                            el.set(prop, "auto");
                        }
                    }
                    if !container::can_stretch(kind) {
                        stretched = (false, false);
                    }
                } else {
                    el.set("left", px(slot.x));
                    el.set("top", px(slot.y));
                }
                self.sizes(el, node, kind, bx, stretched);
                if bx.is_none() {
                    el.set("transform-origin", "0 0");
                }
                if let Some(m) = css::matrix(css::linear(local)) {
                    el.set("transform", m);
                }
            }
            Place::Root => {
                el.set("position", "relative");
                self.sizes(el, node, kind, bx, (false, false));
                if bx.is_none() {
                    el.set("transform-origin", "0 0");
                }
            }
        }
    }

    /// A box's own size on `axis` (0 width, 1 height): its bound variable's, else
    /// `used` px.
    fn axis_size(&mut self, node: &Node, axis: usize, used: f64) -> String {
        match node.size_vars()[axis] {
            Some(v) => self.tokens.px_floored(v),
            None => px(used),
        }
    }

    /// Width and height as specified: a shape's or a frame's own size, a group's
    /// `LayoutItem` sizes (`auto` hugging, as the model reads it), a text box's
    /// sizing mode. An axis `stretched` between two insets is left to them.
    fn sizes(
        &mut self,
        el: &mut Element,
        node: &Node,
        kind: &NodeKind,
        bx: Option<Rect>,
        stretched: (bool, bool),
    ) {
        let item = node.item();
        // A bound size stands for the kind's own size, the axis's px.
        let [bound_w, bound_h] = node.size_vars();
        let (w, h): (Option<String>, Option<String>) = match kind {
            NodeKind::Group => {
                if node.display().is_none() {
                    (Some("0".into()), Some("0".into()))
                } else {
                    (Some(hug(item.width)), Some(hug(item.height)))
                }
            }
            NodeKind::Artboard { size } if node.display().is_some() => (
                Some(match (item.width, bound_w) {
                    (Dimension::Auto, Some(v)) => self.tokens.px_floored(v),
                    (d, _) => own_or(d, size.width),
                }),
                Some(match (item.height, bound_h) {
                    (Dimension::Auto, Some(v)) => self.tokens.px_floored(v),
                    (d, _) => own_or(d, size.height),
                }),
            ),
            NodeKind::Text { sizing, .. } => match sizing {
                TextSizing::Auto => (None, None),
                TextSizing::AutoHeight(w) => (Some(px(*w)), None),
                TextSizing::Fixed(s) => (Some(px(s.width)), Some(px(s.height))),
            },
            _ => {
                let s = bx.map(|b| b.size()).unwrap_or_default();
                (
                    Some(self.axis_size(node, 0, s.width)),
                    Some(self.axis_size(node, 1, s.height)),
                )
            }
        };
        if let Some(w) = w.filter(|_| !stretched.0) {
            el.set("width", w);
        }
        if let Some(h) = h.filter(|_| !stretched.1) {
            el.set("height", h);
        }
        // `k` is `LayoutItem::limit`'s place; a bound limit is its variable in px.
        for (k, prop) in [
            (0, "min-width"),
            (2, "min-height"),
            (1, "max-width"),
            (3, "max-height"),
        ] {
            match (item.limit_vars[k], item.limit(k)) {
                (Some(var), _) => el.set(prop, self.tokens.px_floored(var)),
                (None, Dimension::Auto) => {}
                (None, d) => el.set(prop, dimension(d)),
            }
        }
    }

    /// Fills as `background` layers (CSS lists the top one first) and the one
    /// stroke as an `outline` or `border`s. [`Self::css_blocker`] has already
    /// checked that each has a form. Answers the element drawing a frame's
    /// stroke over its children, which goes after them.
    fn paint(&mut self, el: &mut Element, node: &Node, size: Size) -> Option<Element> {
        let paint = ondin_core::paint_box::node_paint(self.doc, self.res, node.id())
            .unwrap_or_else(|| self.doc.resolved_paint(node.id(), node.paint()));
        // The resolved list for the tier's checks, the stored one for bindings;
        // resolving keeps every item in place.
        let stored = node.paint();
        let mut pixelated = false;
        let mut pictured = false;
        let layers: Vec<String> = paint
            .fills
            .iter()
            .zip(&stored.fills)
            .filter(|(f, _)| f.visible)
            .rev()
            .filter_map(|(f, s)| match &s.brush {
                BrushSource::Bound { var, alpha } => {
                    let c = self.tokens.color(*var, *alpha);
                    Some(format!("linear-gradient({c}, {c})"))
                }
                BrushSource::Literal(Brush::Image(img)) => {
                    let layer = image::layer(self.doc, img, size).ok()?;
                    if !self.assets.iter().any(|(p, _)| *p == layer.path)
                        && let Some(bytes) = image::file(self.doc, &img.image)
                    {
                        self.assets.push((layer.path.clone(), bytes));
                    }
                    pixelated |= layer.pixelated;
                    pictured = true;
                    Some(layer.css)
                }
                BrushSource::Literal(_) => css::background_layer(f.brush.drawn(), size),
            })
            .collect();
        if !layers.is_empty() {
            el.set("background", layers.join(", "));
        }
        if pixelated {
            el.set("image-rendering", "pixelated");
        }
        let kind = self.res.used_kind_of(node);
        let (s, raw) = paint
            .strokes
            .iter()
            .zip(&stored.strokes)
            .find(|(s, _)| s.visible && s.width > 0.0)?;
        let c = match raw.brush {
            BrushSource::Bound { var, alpha } => self.tokens.color(var, alpha),
            BrushSource::Literal(_) => {
                let Brush::Solid(c) = s.brush.drawn() else {
                    return None;
                };
                css::color(*c, 1.0)
            }
        };
        let sides = ondin_core::geometry::effective_sides(kind, s);
        if sides.is_all() {
            let bound = raw.width_var.map(|var| self.tokens.px_floored(var));
            // `k` times the stroke's width.
            let times = |k: f64| match &bound {
                Some(w) if k == 1.0 => w.clone(),
                Some(w) => format!("calc({w} * {})", fmt(k)),
                None => px(s.width * k),
            };
            let offset = match s.align {
                StrokeAlign::Inside => -1.0,
                StrokeAlign::Center => -0.5,
                StrokeAlign::Outside => 0.0,
            };
            // The canvas strokes a frame over its children, which paint over the
            // frame's own outline: the part inside the box goes on a last child,
            // the part outside stays on a clipping frame, which would clip it.
            let (own, over) = match (node.children().is_empty(), node.clip(), s.align) {
                (true, ..) => (Some((1.0, offset)), None),
                (false, true, StrokeAlign::Outside) => (Some((1.0, 0.0)), None),
                (false, true, StrokeAlign::Center) => (Some((0.5, 0.0)), Some((0.5, -0.5))),
                (false, ..) => (None, Some((1.0, offset))),
            };
            let outline = |el: &mut Element, (width, offset): (f64, f64)| {
                el.set("outline", format!("{} solid {c}", times(width)));
                if offset != 0.0 {
                    el.set("outline-offset", times(offset));
                }
            };
            if let Some(own) = own {
                outline(el, own);
            }
            over.map(|over| {
                let class = self.classes.mint(&format!("{} stroke", el.class), "stroke");
                let mut e = Element::new("div", class);
                for (p, v) in [
                    ("position", "absolute"),
                    ("left", "0"),
                    ("top", "0"),
                    ("width", "100%"),
                    ("height", "100%"),
                    ("pointer-events", "none"),
                ] {
                    e.set(p, v);
                }
                outline(&mut e, over);
                // No layer's name path: a merged set matches it as its own.
                e.key = Some((String::new(), usize::MAX));
                e
            })
        } else {
            el.set("box-sizing", "border-box");
            // A picture is framed on the whole box, not inside the borders.
            if pictured {
                el.set("background-origin", "border-box");
            }
            let [t, r, b, l] = side_widths(sides, s.width);
            // A bound width is every drawn side's, but for custom widths, which
            // carry their own numbers.
            let bound = raw
                .width_var
                .filter(|_| !matches!(sides, StrokeSides::Custom { .. }));
            for (w, prop) in [
                (t, "border-top"),
                (r, "border-right"),
                (b, "border-bottom"),
                (l, "border-left"),
            ] {
                if w > 0.0 {
                    let width = match bound {
                        Some(var) => self.tokens.px_floored(var),
                        None => px(w),
                    };
                    el.set(prop, format!("{width} solid {c}"));
                }
            }
            None
        }
    }

    /// The effect stack as `filter`: blurs first, the one drop shadow after, as
    /// the model casts shadows from the blurred layer.
    fn effects(&mut self, el: &mut Element, node: &Node) {
        let effects = self.doc.resolved_effects(node.id(), node.effects());
        // Resolved for the numbers, stored for the bindings, item for item.
        let both: Vec<_> = effects
            .iter()
            .zip(node.effects())
            .filter(|(e, _)| e.visible)
            .map(|(e, s)| (e.kind.clone(), s.vars))
            .collect();
        let mut parts = Vec::new();
        for (kind, vars) in &both {
            if let EffectKind::LayerBlur { radius } = kind {
                let blur = match vars[EFFECT_BLUR] {
                    Some(v) => format!("calc({} * 0.5)", self.tokens.px_floored(v)),
                    None => px(ondin_core::effect::deviation(*radius)),
                };
                parts.push(format!("blur({blur})"));
            }
        }
        for (kind, vars) in &both {
            if let EffectKind::DropShadow(s) = kind {
                let x = match vars[EFFECT_X] {
                    Some(v) => self.tokens.px(v),
                    None => px(s.offset.x),
                };
                let y = match vars[EFFECT_Y] {
                    Some(v) => self.tokens.px(v),
                    None => px(s.offset.y),
                };
                let blur = match vars[EFFECT_BLUR] {
                    Some(v) => format!("calc({} * 0.5)", self.tokens.px_floored(v)),
                    None => px(ondin_core::effect::deviation(s.blur)),
                };
                parts.push(format!(
                    "drop-shadow({x} {y} {blur} {})",
                    css::color(s.color, 1.0)
                ));
            }
        }
        if !parts.is_empty() {
            el.set("filter", parts.join(" "));
        }
    }

    /// A container's own layout declarations; `bound` its gap and padding
    /// bindings, in `Display::spacing`'s order.
    fn display(&mut self, el: &mut Element, display: &Display, bound: [Option<VarId>; 6]) {
        let pad = display.padding();
        let (column_gap, row_gap) = display.gaps();
        match display {
            Display::Flex(f) => {
                el.set("display", "flex");
                if f.direction != FlexDirection::Row {
                    el.set(
                        "flex-direction",
                        match f.direction {
                            FlexDirection::Row => "row",
                            FlexDirection::RowReverse => "row-reverse",
                            FlexDirection::Column => "column",
                            FlexDirection::ColumnReverse => "column-reverse",
                        },
                    );
                }
                if f.wrap != FlexWrap::NoWrap {
                    el.set(
                        "flex-wrap",
                        match f.wrap {
                            FlexWrap::NoWrap => "nowrap",
                            FlexWrap::Wrap => "wrap",
                            FlexWrap::WrapReverse => "wrap-reverse",
                        },
                    );
                }
                el.set("justify-content", justify(f.justify_content));
                el.set("align-items", align_items(f.align_items));
                if f.wrap != FlexWrap::NoWrap {
                    el.set("align-content", align_content(f.align_content));
                }
            }
            Display::Grid(g) => {
                el.set("display", "grid");
                el.set("grid-template-columns", container::tracks_css(&g.columns));
                el.set("grid-template-rows", container::tracks_css(&g.rows));
                if g.auto_flow != GridAutoFlow::Row {
                    el.set("grid-auto-flow", "column");
                }
                el.set("justify-content", align_content(g.justify_content));
                if let Some(j) = g.justify_items {
                    el.set("justify-items", align_items(j));
                }
                if let Some(a) = g.align_items {
                    el.set("align-items", align_items(a));
                }
                el.set("align-content", align_content(g.align_content));
            }
        }
        let mut length = |v: f64, var: Option<VarId>| match var {
            Some(var) => Some(self.tokens.px(var)),
            None => (v != 0.0).then(|| px(v)),
        };
        if let Some(g) = length(column_gap, bound[0]) {
            el.set("column-gap", g);
        }
        if let Some(g) = length(row_gap, bound[1]) {
            el.set("row-gap", g);
        }
        let sides: Vec<Option<String>> = (0..4).map(|i| length(pad[i], bound[2 + i])).collect();
        if sides.iter().any(Option::is_some) {
            el.set(
                "padding",
                sides
                    .into_iter()
                    .map(|s| s.unwrap_or_else(|| "0".into()))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
    }

    /// A text node: the face on the element, a `p` per paragraph, its runs as
    /// [`Self::run`] writes them, and the content already cased as the canvas
    /// cases it.
    fn text(&mut self, el: &mut Element, node: &Node) {
        let kind = self.res.used_kind_of(node);
        let NodeKind::Text {
            content,
            style,
            spans,
            para_spans,
            paragraph,
            block,
            sizing,
            ..
        } = kind
        else {
            return;
        };
        let paint = self.doc.resolved_paint(node.id(), node.paint());
        let ink = paint
            .fills
            .iter()
            .zip(&node.paint().fills)
            .find(|(f, _)| f.visible)
            .map(|(f, stored)| (f.brush.drawn().clone(), &stored.brush));
        match ink {
            Some((_, BrushSource::Bound { var, alpha })) => {
                let c = self.tokens.color(*var, *alpha);
                el.set("color", c);
            }
            Some((Brush::Solid(c), _)) => el.set("color", css::color(c, 1.0)),
            // Text with no ink is black, as on the canvas.
            _ => el.set("color", "#000000"),
        }
        for (p, v) in face(style, None) {
            el.set(p, v);
        }
        let vars = node.text_vars();
        let content_var = vars[ondin_core::variable::TEXT_CONTENT];
        if let Some(var) = vars[ondin_core::variable::TEXT_FAMILY] {
            el.set(
                "font-family",
                format!("{}, sans-serif", self.tokens.var(var)),
            );
        }
        // A bound type number in the unit its literal holds, a % as hundredths of
        // an em, within the ranges the model keeps it to (`variable::TextVars`).
        let em = |l: Option<ondin_core::Length>| matches!(l, Some(ondin_core::Length::Em(_)));
        if let Some(var) = vars[ondin_core::variable::TEXT_SIZE] {
            el.set("font-size", format!("max(1px, {})", self.tokens.px(var)));
        }
        if let Some(var) = vars[ondin_core::variable::TEXT_WEIGHT] {
            el.set(
                "font-weight",
                format!("clamp(1, round({}), 1000)", self.tokens.var(var)),
            );
        }
        if let Some(var) = vars[ondin_core::variable::TEXT_LINE_HEIGHT] {
            let v = match em(style.line_height) {
                true => format!("max(0, calc({} / 100))", self.tokens.var(var)),
                false => self.tokens.px_floored(var),
            };
            el.set("line-height", v);
        }
        if let Some(var) = vars[ondin_core::variable::TEXT_LETTER_SPACING] {
            let v = match em(Some(style.letter_spacing)) {
                true => format!("calc({} * 0.01em)", self.tokens.var(var)),
                false => self.tokens.px(var),
            };
            el.set("letter-spacing", v);
        }
        if let Some(var) = content_var
            && tokens::varies(self.doc, var)
        {
            self.report.approximate(format!(
                "text {}: its content is bound to a variable and written for one mode",
                label(node)
            ));
        }
        // CSS's `normal` reads a different metrics table per browser and platform,
        // so an automatic line height is written as the height the canvas laid.
        let runs = spans.runs(style, content.len());
        if style.line_height.is_none()
            && let Some(layout) = self.res.text_layout(node.id())
        {
            el.set("line-height", px(layout.line_height));
            if runs.iter().any(|(_, s)| {
                s.line_height.is_none()
                    && (s.font_size != style.font_size || s.font_family != style.font_family)
            }) {
                self.report.approximate(format!(
                    "text {}: an automatic line height under a run of another size or font",
                    label(node)
                ));
            }
        }
        // On each paragraph, not here: the indentation between `p`s would be kept.
        let white_space = if matches!(sizing, TextSizing::Auto) {
            "pre"
        } else {
            "pre-wrap"
        };
        if block.vertical_align != ondin_core::VerticalAlign::Top {
            el.set("display", "flex");
            el.set("flex-direction", "column");
            el.set(
                "justify-content",
                match block.vertical_align {
                    ondin_core::VerticalAlign::Middle => "center",
                    _ => "flex-end",
                },
            );
        }
        // CSS's `text-overflow` never fires on text that wraps, and the canvas's
        // cut is its own.
        if block.overflow != ondin_core::TextOverflow::Visible {
            el.set("overflow", "hidden");
            if block.overflow == ondin_core::TextOverflow::Ellipsis
                && self.res.text_layout(node.id()).is_some_and(|l| l.truncated)
            {
                self.report.approximate(format!(
                    "text {}: an ellipsis where the text overflows",
                    label(node)
                ));
            }
        }
        if block.max_lines > 0 || block.trim != ondin_core::BoxTrim::None {
            self.report
                .approximate(format!("text {}: a line limit or box trim", label(node)));
        }
        // The canvas's shaper infers the base direction, which the page does not.
        if content.chars().any(right_to_left) {
            self.report
                .approximate(format!("text {}: right-to-left text", label(node)));
        }
        // Length-preserving, so the runs' ranges index it as they do `content`.
        let cased = ondin_core::text::cased_text(content, &runs);
        let shown = cased.as_deref().unwrap_or(content);

        let mut start = 0;
        let mut first = true;
        for para in content.split('\n') {
            let end = start + para.len();
            let p_style = para_spans.resolve(paragraph, start);
            let mut p = Element::new("p", String::new());
            let mut decl: Vec<(&'static str, String)> = Vec::new();
            decl.push(("margin", "0".into()));
            let wraps = p_style.wrap == ondin_core::WrapMode::Wrap;
            decl.push((
                "white-space",
                if wraps { white_space } else { "pre" }.into(),
            ));
            // A paragraph with no spacing of its own draws the bound default —
            // asked of the spans, since one of its own may equal this mode's value.
            let own = para_spans.as_slice().iter().any(|s| {
                (s.start..s.end).contains(&start)
                    && matches!(s.attr, ondin_core::ParaAttr::Spacing(_))
            });
            let bound = vars[ondin_core::variable::TEXT_PARAGRAPH_SPACING].filter(|_| !own);
            if !first && let Some(var) = bound {
                let v = match paragraph.spacing {
                    ondin_core::Length::Em(_) => {
                        format!("calc({} * 0.01em)", self.tokens.var(var))
                    }
                    ondin_core::Length::Px(_) => self.tokens.px(var),
                };
                decl.push(("margin-top", v));
            } else if !first && !p_style.spacing.is_zero() {
                decl.push(("margin-top", px(p_style.spacing.resolve(style.font_size))));
            }
            decl.push((
                "text-align",
                text_align(p_style.align, p_style.direction).into(),
            ));
            // The model only translates the last line, as `text-align-last` does.
            if p_style.align == ondin_core::TextAlign::Justify {
                match p_style.justify_last {
                    ondin_core::JustifyLast::Start => {}
                    ondin_core::JustifyLast::Center => {
                        decl.push(("text-align-last", "center".into()))
                    }
                    ondin_core::JustifyLast::End => decl.push(("text-align-last", "right".into())),
                }
            }
            match p_style.word_break {
                ondin_core::WordBreak::Normal => {}
                ondin_core::WordBreak::BreakAll => decl.push(("word-break", "break-all".into())),
                ondin_core::WordBreak::KeepAll => decl.push(("word-break", "keep-all".into())),
            }
            match p_style.overflow_wrap {
                ondin_core::OverflowWrap::Normal => {}
                ondin_core::OverflowWrap::Anywhere => {
                    decl.push(("overflow-wrap", "anywhere".into()))
                }
                ondin_core::OverflowWrap::BreakWord => {
                    decl.push(("overflow-wrap", "break-word".into()))
                }
            }
            if p_style.optical_margins {
                self.report
                    .approximate(format!("text {}: optical margin alignment", label(node)));
            }
            if !p_style.indent.is_zero() {
                if p_style.hanging {
                    self.report
                        .approximate(format!("text {}: a hanging indent", label(node)));
                }
                decl.push(("text-indent", px(p_style.indent.resolve(style.font_size))));
            }
            if !p_style.indent_start.is_zero() {
                decl.push((
                    "padding-inline-start",
                    px(p_style.indent_start.resolve(style.font_size)),
                ));
            }
            if !p_style.indent_end.is_zero() {
                decl.push((
                    "padding-inline-end",
                    px(p_style.indent_end.resolve(style.font_size)),
                ));
            }
            if p_style.marker.is_some() || p_style.level > 0 {
                self.report
                    .approximate(format!("text {}: list markers", label(node)));
            }
            p.style = decl;
            p.class = self.classes.mint(&format!("{} p", el.class), "p");
            for (range, s) in &runs {
                let lo = range.start.max(start);
                let hi = range.end.min(end);
                if lo >= hi {
                    continue;
                }
                let text = Child::Text(shown[lo..hi].to_string());
                let run = self.run(text, s, style, node, &el.class);
                p.children.push(run);
            }
            // An empty paragraph still takes a line.
            if p.children.is_empty() {
                p.children.push(Child::Break);
            }
            el.children.push(Child::Element(Box::new(p)));
            start = end + 1;
            first = false;
        }
    }

    /// One run's `content` in its own inline box where it needs one: for a face
    /// differing from the node's `base`, a baseline shift (moving the ink, not the
    /// line, as the model does), or a decoration. A decoration is drawn by the box
    /// carrying it and cannot be taken off below it, so it is never the node's:
    /// both on one run nest, the underline outside.
    fn run(
        &mut self,
        content: Child,
        s: &TextStyle,
        base: &TextStyle,
        node: &Node,
        class: &str,
    ) -> Child {
        let mut own = face(s, Some(base));
        let shift = s.baseline_shift.resolve(s.font_size);
        if shift != 0.0 {
            own.push(("position", "relative".into()));
            own.push(("top", px(-shift)));
        }
        let under = s
            .underline
            .as_ref()
            .map(|d| self.decoration(node, "underline", d, s));
        let strike = s
            .strikethrough
            .as_ref()
            .map(|d| self.decoration(node, "line-through", d, s));
        let span = |walk: &mut Self, style: Vec<(&'static str, String)>, child: Child| {
            let mut e = Element::new("span", walk.classes.mint(&format!("{class} span"), "span"));
            e.style = style;
            e.children.push(child);
            Child::Element(Box::new(e))
        };
        match (under, strike) {
            (Some(mut u), Some(t)) => {
                let inner = span(self, t, content);
                own.append(&mut u);
                span(self, own, inner)
            }
            (None, None) if own.is_empty() => content,
            (u, t) => {
                own.extend(u.or(t).unwrap_or_default());
                span(self, own, content)
            }
        }
    }

    /// An underline or a strikethrough (`line`) as the declarations of the box
    /// drawing it. The font decides an absent thickness or offset on both sides;
    /// a style CSS draws its own way, or a strikethrough's offset, is reported.
    fn decoration(
        &mut self,
        node: &Node,
        line: &'static str,
        d: &ondin_core::Decoration,
        s: &TextStyle,
    ) -> Vec<(&'static str, String)> {
        let what = if line == "underline" {
            "underline"
        } else {
            "strikethrough"
        };
        let mut out = vec![("text-decoration-line", line.to_string())];
        let style = match d.style {
            ondin_core::LineStyle::Solid => None,
            ondin_core::LineStyle::Dashed => Some("dashed"),
            ondin_core::LineStyle::Dotted => Some("dotted"),
            ondin_core::LineStyle::Wavy => Some("wavy"),
        };
        if let Some(style) = style {
            out.push(("text-decoration-style", style.into()));
            self.report.approximate(format!(
                "text {}: a dashed, dotted or wavy {what}",
                label(node)
            ));
        }
        if let Some(c) = d.color {
            out.push(("text-decoration-color", css::color(c, 1.0)));
        }
        let thickness = d
            .thickness
            .map_or("from-font".into(), |t| px(t.resolve(s.font_size)));
        out.push(("text-decoration-thickness", thickness));
        if line == "underline" {
            match d.offset {
                // The model's offset is up from the baseline, CSS's down.
                Some(o) => out.push(("text-underline-offset", px(-o.resolve(s.font_size)))),
                None => out.push(("text-underline-position", "from-font".into())),
            }
            if !d.skip_ink {
                out.push(("text-decoration-skip-ink", "none".into()));
            }
        } else if d.offset.is_some() {
            self.report.approximate(format!(
                "text {}: a strikethrough moved off the font's position",
                label(node)
            ));
        }
        out
    }

    /// Tiers 2 and 3: the subtree as inline SVG, or as a PNG when the SVG writer
    /// would approximate it. `why` names what CSS could not draw.
    fn picture(&mut self, node: &Node, place: Place, why: &str) -> Option<Element> {
        let id = node.id();
        let ink = self.res.ink_bounds(id)?;
        let world = self.res.world_transform(id)?;
        let class = self.classes.mint(node.name(), node.kind().default_name());
        // Pictures are linked, so every layer drawing one shares its file.
        let mut linked = HashMap::new();
        let mut files = Vec::new();
        let mut under = vec![id];
        while let Some(at) = under.pop() {
            let Some(n) = self.doc.get(at) else { continue };
            let paint = self.doc.resolved_paint(at, n.paint());
            let brushes = paint.fills.iter().map(|f| f.brush.drawn());
            for b in brushes.chain(paint.strokes.iter().map(|s| s.brush.drawn())) {
                if let Brush::Image(img) = b
                    && !linked.contains_key(&img.image.id)
                    && let Some((path, bytes)) = image::payload(self.doc, &img.image.id)
                {
                    files.push((path.clone(), bytes));
                    linked.insert(img.image.id.clone(), path);
                }
            }
            under.extend(n.children());
        }
        let svg = crate::svg::svg_linking(self.doc, self.res, &[id], linked);
        let mut pic = if svg.approximated.is_empty() {
            self.report.position(format!(
                "{} {}: {why}, written as inline SVG",
                kind_word(node),
                label(node)
            ));
            for (path, bytes) in files {
                if !self.assets.iter().any(|(p, _)| *p == path) {
                    self.assets.push((path, bytes));
                }
            }
            let mut e = Element::new("svg", class.clone());
            e.markup = Some(
                svg.svg
                    .replacen("<svg ", &format!("<svg class=\"{class}\" "), 1),
            );
            e
        } else {
            for what in &svg.approximated {
                self.report.approximate(format!(
                    "{} {}: {what}, so it is a PNG",
                    kind_word(node),
                    label(node)
                ));
            }
            let opts = crate::png::RasterOpts {
                scale: 2.0,
                ..Default::default()
            };
            let raster = crate::png::raster_of(self.doc, self.res, &[id], &opts)?;
            let path = format!("assets/{class}.png");
            self.assets.push((
                path.clone(),
                crate::png::encode_rgba8(&raster.rgba, raster.width, raster.height),
            ));
            let mut e = Element::new("img", class.clone());
            e.attrs.push(("src", path));
            e.attrs.push(("alt", node.name().to_string()));
            e
        };
        pic.set("position", "absolute");
        pic.set("left", "0");
        pic.set("top", "0");
        pic.set("width", px(ink.width()));
        pic.set("height", px(ink.height()));
        pic.set("transform-origin", "0 0");
        let at_ink = Affine::translate(ink.origin().to_vec2());
        match place {
            // A component's root is only ever a CSS element (`Walk::why_inline`).
            Place::Free { .. } | Place::Root => {
                let local = match place {
                    Place::Free { local, .. } => local,
                    _ => Affine::IDENTITY,
                };
                // The picture is in world space; `local` maps this layer into the
                // parent element, `world` into the world.
                let to_parent = local * world.inverse() * at_ink;
                set_transform(&mut pic, to_parent);
                Some(pic)
            }
            Place::Flow { .. } => {
                // A box in the flow holding the picture, so the layout around it
                // is kept.
                let kind = self.res.used_kind_of(node).clone();
                let bx = self.box_rect(node);
                let mut holder =
                    Element::new("div", self.classes.mint(&format!("{class} box"), "box"));
                self.place(&mut holder, node, &kind, bx, place);
                let origin = bx.map(|b| b.origin().to_vec2()).unwrap_or_default();
                let to_box = Affine::translate(-origin) * world.inverse() * at_ink;
                set_transform(&mut pic, to_box);
                holder.children.push(Child::Element(Box::new(pic)));
                Some(holder)
            }
        }
    }
}

/// `left`/`top` for a translation, else a `transform`.
fn set_transform(el: &mut Element, t: Affine) {
    if css::is_translation(t) {
        let [.., e, f] = t.as_coeffs();
        el.set("left", px(e));
        el.set("top", px(f));
    } else if let Some(m) = css::matrix(t) {
        el.set("transform", m);
    }
}

/// One axis of insets. Answers whether both are set, which stretches the layer.
fn axis(
    el: &mut Element,
    start: &'static str,
    end: &'static str,
    a: Option<LengthPct>,
    b: Option<LengthPct>,
    slot: f64,
) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            el.set(start, length_pct(a));
            el.set(end, length_pct(b));
            true
        }
        (Some(a), None) => {
            el.set(start, length_pct(a));
            false
        }
        (None, Some(b)) => {
            el.set(end, length_pct(b));
            false
        }
        (None, None) => {
            el.set(start, px(slot));
            false
        }
    }
}

fn length_pct(l: LengthPct) -> String {
    match l {
        LengthPct::Px(v) => px(v),
        LengthPct::Percent(v) => format!("{}%", fmt(v)),
    }
}

fn dimension(d: Dimension) -> String {
    match d {
        Dimension::Auto => "auto".into(),
        Dimension::Px(v) => px(v),
        Dimension::Percent(v) => format!("{}%", fmt(v)),
        Dimension::FitContent => "fit-content".into(),
    }
}

/// A group's size: `auto` is the model's hug, which on a block is `fit-content`.
fn hug(d: Dimension) -> String {
    match d {
        Dimension::Auto => "fit-content".into(),
        d => dimension(d),
    }
}

/// A frame's size: its own unless its `LayoutItem` asks otherwise.
fn own_or(d: Dimension, own: f64) -> String {
    match d {
        Dimension::Auto => px(own),
        d => dimension(d),
    }
}

/// Whether the layout treats `node` as a replaced element — anything but text and
/// a container with a layout of its own.
fn replaced(node: &Node, kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Text { .. } => false,
        k => !(container::is_container(k) && node.display().is_some()),
    }
}

fn side_widths(sides: StrokeSides, width: f64) -> [f64; 4] {
    sides.widths(width).map(|w| w.unwrap_or(0.0))
}

fn justify(j: JustifyContent) -> &'static str {
    match j {
        JustifyContent::Start => "flex-start",
        JustifyContent::End => "flex-end",
        JustifyContent::Center => "center",
        JustifyContent::SpaceBetween => "space-between",
        JustifyContent::SpaceAround => "space-around",
        JustifyContent::SpaceEvenly => "space-evenly",
    }
}

fn align_items(a: AlignItems) -> &'static str {
    match a {
        AlignItems::Stretch => "stretch",
        AlignItems::Start => "flex-start",
        AlignItems::End => "flex-end",
        AlignItems::Center => "center",
        AlignItems::Baseline => "baseline",
    }
}

fn align_content(a: AlignContent) -> &'static str {
    match a {
        AlignContent::Stretch => "stretch",
        AlignContent::Start => "flex-start",
        AlignContent::End => "flex-end",
        AlignContent::Center => "center",
        AlignContent::SpaceBetween => "space-between",
        AlignContent::SpaceAround => "space-around",
        AlignContent::SpaceEvenly => "space-evenly",
    }
}

/// `a` under `direction`, as the model resolves it: an explicit direction only
/// picks the side `Start` and `End` mean, the base direction staying the
/// content's.
fn text_align(a: ondin_core::TextAlign, direction: ondin_core::TextDirection) -> &'static str {
    use ondin_core::TextAlign as A;
    use ondin_core::TextDirection as D;
    match (a, direction) {
        (A::Start, D::Auto) => "start",
        (A::End, D::Auto) => "end",
        (A::Start, D::Ltr) | (A::End, D::Rtl) | (A::Left, _) => "left",
        (A::End, D::Ltr) | (A::Start, D::Rtl) | (A::Right, _) => "right",
        (A::Center, _) => "center",
        (A::Justify, _) => "justify",
    }
}

/// CSS's nearest case transform for `case`, `None` for none.
fn text_transform(case: ondin_core::TextCase) -> Option<&'static str> {
    match case {
        ondin_core::TextCase::Original => None,
        ondin_core::TextCase::Upper => Some("uppercase"),
        ondin_core::TextCase::Lower => Some("lowercase"),
        ondin_core::TextCase::Title => Some("capitalize"),
    }
}

/// Whether `c` belongs to a right-to-left script.
fn right_to_left(c: char) -> bool {
    matches!(
        c as u32,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF
    )
}

/// What a layer is, for the report.
fn kind_word(node: &Node) -> &'static str {
    match node.kind() {
        NodeKind::Text { .. } => "text",
        NodeKind::Artboard { .. } => "frame",
        NodeKind::Group => "group",
        _ => "layer",
    }
}

/// The layers from below `top` down to `id`, each as its name and which sibling
/// of that name it is: how the model matches a shown slot across a set's
/// variants and an instance's copies (`variant::ShownNested::key`).
fn name_path(doc: &Document, top: NodeId, id: NodeId) -> Vec<(String, usize)> {
    let mut steps = Vec::new();
    let mut at = Some(id);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if n.id() == top {
            steps.reverse();
            return steps;
        }
        steps.push(name_step(doc, n));
        at = n.parent();
    }
    Vec::new()
}

/// `n`'s step in a name path: its name and which sibling of that name it is.
fn name_step(doc: &Document, n: &Node) -> (String, usize) {
    let k = n
        .parent()
        .and_then(|p| doc.get(p))
        .map(|p| {
            p.children()
                .iter()
                .take_while(|c| **c != n.id())
                .filter(|c| doc.get(**c).is_some_and(|c| c.name() == n.name()))
                .count()
        })
        .unwrap_or(0);
    (n.name().to_string(), k)
}

/// The layer below `top` at `path`, [`name_path`]'s inverse.
fn at_name_path(doc: &Document, top: NodeId, path: &[(String, usize)]) -> Option<NodeId> {
    let mut at = top;
    for (name, k) in path {
        at = doc
            .get(at)?
            .children()
            .iter()
            .copied()
            .filter(|c| doc.get(*c).is_some_and(|c| c.name() == name))
            .nth(*k)?;
    }
    Some(at)
}

/// Whether `id` is `ancestor` or below it.
fn under(doc: &Document, id: NodeId, ancestor: NodeId) -> bool {
    let mut at = Some(id);
    while let Some(a) = at {
        if a == ancestor {
            return true;
        }
        at = doc.get(a).and_then(Node::parent);
    }
    false
}

/// The runs of ASCII letters and digits in `name`.
fn words(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// `"primary button / hover"` → `PrimaryButtonHover`.
fn component_name(name: &str) -> String {
    let mut out: String = words(name)
        .iter()
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect();
    if out.is_empty() {
        out.push_str("Component");
    } else if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'C');
    }
    // A swap prop's binding ends in `Slot` (`jsx::slot_binding`), so no
    // component name does.
    if out.ends_with("Slot") {
        out.push_str("Component");
    }
    out
}

/// `"Show icon"` → `showIcon`; a reserved word gets `Prop` after it.
fn prop_name(name: &str) -> String {
    let mut out = String::new();
    for (i, w) in words(name).iter().enumerate() {
        let mut c = w.chars();
        let Some(f) = c.next() else { continue };
        if i == 0 {
            out.push(f.to_ascii_lowercase());
            out.push_str(c.as_str());
        } else {
            out.push(f.to_ascii_uppercase());
            out.push_str(c.as_str());
        }
    }
    if out.is_empty() {
        out.push_str("prop");
    } else if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'p');
    }
    const RESERVED: &[&str] = &[
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "new",
        "null",
        "return",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
        "let",
        "static",
        "await",
        // Reserved in strict code, which every ES module is.
        "implements",
        "interface",
        "package",
        "private",
        "protected",
        "public",
        "arguments",
        "eval",
        "style",
        "is",
        "slot",
        // The CSS module every React component imports its classes as.
        "styles",
    ];
    if RESERVED.contains(&out.as_str()) {
        out.push_str("Prop");
    }
    out
}

/// `base` as a file or folder name too: off the Windows device names (`Con` +
/// `suffix`), and unique in `taken` ignoring case, as NTFS and APFS compare.
/// `taken` holds lowercase names.
fn unique_file(mut base: String, suffix: &str, taken: &mut HashSet<String>) -> String {
    const DEVICES: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
        "LPT9",
    ];
    if DEVICES.contains(&base.to_ascii_uppercase().as_str()) {
        base.push_str(suffix);
    }
    let mut candidate = base.clone();
    let mut n = 1;
    while taken.contains(&candidate.to_ascii_lowercase()) {
        n += 1;
        candidate = format!("{base}{n}");
    }
    taken.insert(candidate.to_ascii_lowercase());
    candidate
}

/// `base`, or `base2`, `base3`… for the first not in `taken`, which it joins.
fn unique(base: String, taken: &mut HashSet<String>) -> String {
    let mut candidate = base.clone();
    let mut n = 1;
    while taken.contains(&candidate) {
        n += 1;
        candidate = format!("{base}{n}");
    }
    taken.insert(candidate.clone());
    candidate
}

/// A face as declarations: all of it, or with `base` only what differs from it.
/// Case, decorations and baseline shift are a run's own ([`Walk::run`]).
fn face(s: &TextStyle, base: Option<&TextStyle>) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let differs = |f: &dyn Fn(&TextStyle) -> String| base.is_none_or(|b| f(b) != f(s));
    let family = |t: &TextStyle| format!("\"{}\", sans-serif", t.font_family.replace('"', ""));
    if differs(&family) {
        out.push(("font-family", family(s)));
    }
    let size = |t: &TextStyle| px(t.font_size);
    if differs(&size) {
        out.push(("font-size", size(s)));
    }
    let weight = |t: &TextStyle| t.weight.to_string();
    if differs(&weight) {
        out.push(("font-weight", weight(s)));
    }
    let style = |t: &TextStyle| {
        if t.italic {
            "italic".into()
        } else {
            "normal".into()
        }
    };
    if differs(&style) {
        out.push(("font-style", style(s)));
    }
    let line = |t: &TextStyle| match t.line_height {
        None => "normal".to_string(),
        Some(ondin_core::Length::Px(v)) => px(v),
        Some(ondin_core::Length::Em(m)) => fmt(m),
    };
    if differs(&line) {
        out.push(("line-height", line(s)));
    }
    let tracking = |t: &TextStyle| px(t.letter_spacing.resolve(t.font_size));
    if differs(&tracking) && (base.is_some() || !s.letter_spacing.is_zero()) {
        out.push(("letter-spacing", tracking(s)));
    }
    let words = |t: &TextStyle| px(t.word_spacing.resolve(t.font_size));
    if differs(&words) && (base.is_some() || !s.word_spacing.is_zero()) {
        out.push(("word-spacing", words(s)));
    }
    let ink = |t: &TextStyle| t.color.map(|c| css::color(c, 1.0)).unwrap_or_default();
    if s.color.is_some() && differs(&ink) {
        out.push(("color", ink(s)));
    }
    let features = |t: &TextStyle| {
        t.features
            .iter()
            .map(|f| format!("\"{}\" {}", f.tag.as_str(), f.value))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !s.features.is_empty() && differs(&features) {
        out.push(("font-feature-settings", features(s)));
    }
    let axes = |t: &TextStyle| {
        t.variations
            .iter()
            .map(|a| format!("\"{}\" {}", a.tag.as_str(), fmt(a.value)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !s.variations.is_empty() && differs(&axes) {
        out.push(("font-variation-settings", axes(s)));
    }
    out
}
