//! The DOM core: layers to elements and declarations, each in the first of the
//! three tiers that draws it exactly — CSS, inline SVG, PNG.
//!
//! **Every element establishes its layer's local space**: its top-left is the
//! layer's box origin, so a child's numbers are the model's numbers. Every element
//! is positioned (`relative` in a flow, `absolute` out of one) so siblings paint in
//! tree order, as the model's z-order does.

use super::Report;
use super::css::{self, px};
use super::dom::{Arg, Child, Classes, Element, Instance};
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
use ondin_core::variant::{self, PropKind, PropValue};
use ondin_core::{
    Brush, BrushSource, Document, EffectKind, Fill, ItemId, Node, NodeId, NodeKind, Resolved,
    Stroke, StrokeAlign, StrokeSides, TextSizing, TextStyle,
};
use ondin_core::{component, reset};
use std::collections::{HashMap, HashSet};

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
    /// Each variant's tree with the variant values it is drawn for, the default
    /// variant's last and drawn for any others. A main outside a set has one
    /// tree, drawn always.
    pub bodies: Vec<(Vec<(String, String)>, Element)>,
    /// Its variant properties: the name code reads each by, its values and the
    /// default variant's value.
    pub choices: Vec<(String, Vec<String>, String)>,
    /// Its properties, each with the main's own value as the default.
    pub props: Vec<(String, Arg)>,
}

impl Component {
    /// The page as a component of one tree and no properties, named `App`.
    pub fn page(root: Element) -> Self {
        Component {
            name: "App".into(),
            bodies: vec![(Vec::new(), root)],
            choices: Vec::new(),
            props: Vec::new(),
        }
    }
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
    let view = crate::extent(res, origins).unwrap_or(Rect::new(0.0, 0.0, 100.0, 100.0));
    let mut walk = Walk {
        doc,
        res,
        classes: Classes::default(),
        report: Report::default(),
        assets: Vec::new(),
        tokens: Tokens::new(doc),
        components: components.then(|| Components {
            names: HashSet::from(["App".to_string()]),
            ..Default::default()
        }),
        bound: HashMap::new(),
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
                    // A component use writes no attributes of its own, so the mode
                    // goes on an element around it that draws no box.
                    let mut around = Element::new("div", walk.classes.mint("mode", "mode"));
                    around.set("display", "contents");
                    around.attrs.push(("data-mode", mode.to_string()));
                    around.children.push(Child::Element(Box::new(el)));
                    el = around;
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
}

/// The components made so far.
#[derive(Default)]
struct Components {
    /// By owner: the set for a variant, else the main.
    by_owner: HashMap<NodeId, Signature>,
    /// In the order finished, so a component comes after those it uses.
    list: Vec<Component>,
    /// Component names taken; `App` is the page's.
    names: HashSet<String>,
}

/// What a use of a component needs to know of it.
#[derive(Clone)]
struct Signature {
    name: String,
    /// Per property item, its name in code and its default.
    props: Vec<(ItemId, String, Arg)>,
    /// Per variant property, in the set's order, its name in code and the
    /// default variant's value.
    choices: Vec<(String, String)>,
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
        Some(el)
    }

    /// Why the instance at `root` cannot be a use of its main's component, or
    /// `None` when it can: when every value it differs from its main in is a
    /// property's, up the chain of copies to the main, apart from its root's
    /// size, which its use sets.
    fn why_inline(&self, root: NodeId) -> Option<String> {
        let doc = self.doc;
        let main = component::main_of(doc, root)?;
        if doc.in_vault(main) {
            return Some("its main is from a kit, which is written out as layers".into());
        }
        // Every variant is a tree of the component, so each must be a CSS root.
        let trees = match variant::set_of(doc, main) {
            Some(set) => {
                let values = doc.get(main)?.variant();
                if variant::find_variant(doc, set, values) != Some(main) {
                    return Some("its variant has the values of another, which is written".into());
                }
                variant::variants(doc, set)
            }
            None => vec![main],
        };
        for t in trees {
            if self.css_blocker(doc.get(t)?).is_some() {
                return Some("its main is drawn as a picture".into());
            }
        }
        const OVERRIDES: &str = "inlined because of overrides; make it a property to keep the link";
        let mut at = root;
        loop {
            let n = doc.get(at)?;
            if n.swap().is_some() || n.mode() != doc.get(main)?.mode() {
                return Some(OVERRIDES.into());
            }
            let drift = reset::drift(doc, at);
            if drift.removed + drift.order + drift.local > 0 {
                return Some(OVERRIDES.into());
            }
            let props: HashSet<(NodeId, PropKind)> = variant::instance_properties(doc, at)
                .iter()
                .filter(|p| matches!(p.kind, PropKind::Boolean | PropKind::Text))
                .flat_map(|p| {
                    variant::counterparts(doc, at, p)
                        .into_iter()
                        .map(move |c| (c, p.kind))
                })
                .collect();
            for id in reset::scope_nodes(doc, at) {
                if id != at && doc.get(id)?.swap().is_some() {
                    return Some(OVERRIDES.into());
                }
                for o in reset::overrides(doc, id) {
                    use ondin_core::Operation as O;
                    let allowed = match o.reset {
                        O::SetVisible { .. } | O::SetVisibleVar { .. } => {
                            id == at || props.contains(&(id, PropKind::Boolean))
                        }
                        O::SetText { .. } => props.contains(&(id, PropKind::Text)),
                        O::SetGeometry { .. } => id == at,
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
        let values = doc.get(main).map(Node::variant).unwrap_or_default();
        for ((prop, default), value) in sig.choices.iter().zip(values) {
            if value != default {
                args.push((prop.clone(), Arg::Text(value.clone())));
            }
        }
        for p in variant::instance_properties(doc, node.id()).iter() {
            let Some((_, prop, default)) = sig.props.iter().find(|(id, ..)| *id == p.id) else {
                continue;
            };
            let value = variant::counterparts(doc, node.id(), p)
                .first()
                .and_then(|c| variant::field_value(doc, *c, p.kind))
                .and_then(arg);
            if let Some(v) = value.filter(|v| v != default) {
                args.push((prop.clone(), v));
            }
        }
        el.instance = Some(Instance {
            component: sig.name,
            args,
        });
        el
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
        let name = unique(component_name(owner.name()), &mut comps.names);

        // What a component's props object already holds.
        let mut taken: HashSet<String> = ["className", "key", "ref", "children"]
            .map(String::from)
            .into();
        // The variants, the first of any that share their values, and the default.
        let (trees, default) = match owner.set() {
            Some(_) => {
                let mut seen = HashSet::new();
                let trees: Vec<NodeId> = variant::variants(doc, owner_id)
                    .into_iter()
                    .filter(|v| seen.insert(doc.get(*v).map(|n| n.variant().to_vec())))
                    .collect();
                let default = ondin_core::kit::default_variant(doc, owner_id)
                    .filter(|d| trees.contains(d))
                    .unwrap_or(main);
                (trees, default)
            }
            None => (vec![main], main),
        };
        let default_values = doc.get(default).map(Node::variant).unwrap_or_default();
        let choices: Vec<(String, Vec<String>, String)> = owner
            .set()
            .map(|s| s.props.as_slice())
            .unwrap_or_default()
            .iter()
            .zip(default_values)
            .map(|(vp, d)| {
                (
                    unique(prop_name(&vp.name), &mut taken),
                    vp.values.clone(),
                    d.clone(),
                )
            })
            .collect();

        let mut bound = HashMap::new();
        let mut props = Vec::new();
        for p in owner.props() {
            if !matches!(p.kind, PropKind::Boolean | PropKind::Text) {
                continue;
            }
            // The default variant's value, where the property reaches it.
            let Some(default) = p
                .bound
                .iter()
                .find(|b| under(doc, **b, default))
                .or(p.bound.first())
                .and_then(|b| variant::field_value(doc, *b, p.kind))
                .and_then(arg)
            else {
                continue;
            };
            let prop = unique(prop_name(&p.name), &mut taken);
            for b in &p.bound {
                let styled = match doc.get(*b).map(Node::kind) {
                    Some(NodeKind::Text { content, spans, .. }) => {
                        content.contains('\n') || !spans.is_empty()
                    }
                    _ => false,
                };
                bound.insert(
                    *b,
                    BoundProp {
                        name: prop.clone(),
                        kind: p.kind,
                        styled,
                    },
                );
            }
            props.push((p.id, prop, default));
        }
        let sig = Signature {
            name: name.clone(),
            props,
            choices: choices
                .iter()
                .map(|(p, _, d)| (p.clone(), d.clone()))
                .collect(),
        };
        let comps = self.components.as_mut().expect("components are on");
        comps.by_owner.insert(owner_id, sig.clone());

        // One file's classes across every variant's tree.
        let classes = std::mem::take(&mut self.classes);
        let outer = std::mem::replace(&mut self.bound, bound);
        let mut bodies = Vec::new();
        for t in trees.iter().filter(|t| **t != default).chain([&default]) {
            let Some(node) = doc.get(*t) else { continue };
            let mut root = self.css_element(node, Place::Root);
            root.takes_class = true;
            let when = match *t == default {
                true => Vec::new(),
                false => choices
                    .iter()
                    .zip(node.variant())
                    .map(|((p, ..), v)| (p.clone(), v.clone()))
                    .collect(),
            };
            bodies.push((when, root));
        }
        self.bound = outer;
        self.classes = classes;

        // Pushed once finished, so after the components it uses.
        let comps = self.components.as_mut().expect("components are on");
        comps.list.push(Component {
            name,
            bodies,
            choices,
            props: sig
                .props
                .iter()
                .map(|(_, p, d)| (p.clone(), d.clone()))
                .collect(),
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
        let paint = doc.resolved_paint(id, node.paint());
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
    /// `outline` offset to the alignment; per-side widths are `border`s, which
    /// only an inside stroke on a childless rect matches — a border moves the
    /// padding box absolutely positioned children are measured from.
    fn stroke_blocker(&self, node: &Node, kind: &NodeKind, s: &Stroke) -> Option<String> {
        if !matches!(s.brush.drawn(), Brush::Solid(_)) {
            return Some("a gradient or image stroke".into());
        }
        if !s.dashes.is_empty() {
            return Some("a dashed stroke".into());
        }
        let square = match kind {
            NodeKind::Rect { corner_radii, .. } => {
                ondin_core::geometry::is_square_cornered(corner_radii)
            }
            NodeKind::Artboard { .. } => true,
            _ => false,
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
        // An outline's corners are square where the radius is zero; the model
        // joins them as asked.
        let mitred =
            s.join == ondin_core::kurbo::Join::Miter && s.miter_limit >= std::f64::consts::SQRT_2;
        if square && s.align != StrokeAlign::Inside && !mitred {
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
                self.paint(&mut el, node, *size);
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
                    if let Some(Child::Element(p)) = el.children.first_mut() {
                        p.children = vec![Child::Prop(b.name.clone())];
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
                Place::Free {
                    local: c.transform(),
                    boxed,
                }
            };
            if let Some(e) = self.node(*child, place) {
                el.children.push(Child::Element(Box::new(e)));
            }
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
        for (d, prop) in [
            (item.min_width, "min-width"),
            (item.min_height, "min-height"),
            (item.max_width, "max-width"),
            (item.max_height, "max-height"),
        ] {
            if d != Dimension::Auto {
                el.set(prop, dimension(d));
            }
        }
    }

    /// Fills as `background` layers (CSS lists the top one first) and the one
    /// stroke as an `outline` or `border`s. [`Self::css_blocker`] has already
    /// checked that each has a form.
    fn paint(&mut self, el: &mut Element, node: &Node, size: Size) {
        let paint = self.doc.resolved_paint(node.id(), node.paint());
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
        let Some((s, raw)) = paint
            .strokes
            .iter()
            .zip(&stored.strokes)
            .find(|(s, _)| s.visible && s.width > 0.0)
        else {
            return;
        };
        let c = match raw.brush {
            BrushSource::Bound { var, alpha } => self.tokens.color(var, alpha),
            BrushSource::Literal(_) => {
                let Brush::Solid(c) = s.brush.drawn() else {
                    return;
                };
                css::color(*c, 1.0)
            }
        };
        let sides = ondin_core::geometry::effective_sides(kind, s);
        if sides.is_all() {
            let w = s.width;
            match raw.width_var {
                Some(var) => {
                    let width = self.tokens.px_floored(var);
                    el.set("outline", format!("{width} solid {c}"));
                    let k = match s.align {
                        StrokeAlign::Inside => Some("-1"),
                        StrokeAlign::Center => Some("-0.5"),
                        StrokeAlign::Outside => None,
                    };
                    if let Some(k) = k {
                        el.set("outline-offset", format!("calc({width} * {k})"));
                    }
                }
                None => {
                    el.set("outline", format!("{} solid {c}", px(w)));
                    let offset = match s.align {
                        StrokeAlign::Inside => -w,
                        StrokeAlign::Center => -w * 0.5,
                        StrokeAlign::Outside => 0.0,
                    };
                    if offset != 0.0 {
                        el.set("outline-offset", px(offset));
                    }
                }
            }
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

    /// A text node: the face on the element, a `p` per paragraph, a `span` per
    /// run whose face differs from the node's.
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
        let [content_var, family_var] = node.text_vars();
        if let Some(var) = family_var {
            el.set(
                "font-family",
                format!("{}, sans-serif", self.tokens.var(var)),
            );
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
        if block.overflow != ondin_core::TextOverflow::Visible {
            el.set("overflow", "hidden");
            if block.overflow == ondin_core::TextOverflow::Ellipsis {
                el.set("text-overflow", "ellipsis");
            }
        }
        if block.max_lines > 0 || block.trim != ondin_core::BoxTrim::None {
            self.report
                .approximate(format!("text {}: a line limit or box trim", label(node)));
        }

        let mut start = 0;
        let mut first = true;
        for para in content.split('\n') {
            let end = start + para.len();
            let p_style = para_spans.resolve(paragraph, start);
            let mut p = Element::new("p", String::new());
            let mut decl: Vec<(&'static str, String)> = Vec::new();
            decl.push(("margin", "0".into()));
            decl.push(("white-space", white_space.into()));
            if !first && !p_style.spacing.is_zero() {
                decl.push(("margin-top", px(p_style.spacing.resolve(style.font_size))));
            }
            decl.push(("text-align", text_align(p_style.align).into()));
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
                let text = content[lo..hi].to_string();
                let own = face(s, Some(style));
                if own.is_empty() {
                    p.children.push(Child::Text(text));
                } else {
                    let mut span = Element::new(
                        "span",
                        self.classes.mint(&format!("{} span", el.class), "span"),
                    );
                    span.style = own;
                    span.children.push(Child::Text(text));
                    p.children.push(Child::Element(Box::new(span)));
                }
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

    /// Tiers 2 and 3: the subtree as inline SVG, or as a PNG when the SVG writer
    /// would approximate it. `why` names what CSS could not draw.
    fn picture(&mut self, node: &Node, place: Place, why: &str) -> Option<Element> {
        let id = node.id();
        let ink = self.res.ink_bounds(id)?;
        let world = self.res.world_transform(id)?;
        let class = self.classes.mint(node.name(), node.kind().default_name());
        let svg = crate::svg::svg_of_reported(self.doc, self.res, &[id]);
        let mut pic = if svg.approximated.is_empty() {
            self.report.position(format!(
                "{} {}: {why}, written as inline SVG",
                kind_word(node),
                label(node)
            ));
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

fn text_align(a: ondin_core::TextAlign) -> &'static str {
    use ondin_core::TextAlign as A;
    match a {
        A::Start => "start",
        A::End => "end",
        A::Left => "left",
        A::Center => "center",
        A::Right => "right",
        A::Justify => "justify",
    }
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

/// A property value as an argument; a swap is not one.
fn arg(v: PropValue) -> Option<Arg> {
    match v {
        PropValue::Boolean(b) => Some(Arg::Bool(b)),
        PropValue::Text(t) => Some(Arg::Text(t)),
        PropValue::Swap(_) => None,
    }
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
        "style",
        "is",
        "slot",
    ];
    if RESERVED.contains(&out.as_str()) {
        out.push_str("Prop");
    }
    out
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
    let case = |t: &TextStyle| {
        match t.case {
            ondin_core::TextCase::Original => "none",
            ondin_core::TextCase::Upper => "uppercase",
            ondin_core::TextCase::Lower => "lowercase",
            ondin_core::TextCase::Title => "capitalize",
        }
        .to_string()
    };
    if differs(&case) && (base.is_some() || s.case != ondin_core::TextCase::Original) {
        out.push(("text-transform", case(s)));
    }
    let deco = |t: &TextStyle| {
        let names: Vec<&str> = [
            t.underline.map(|_| "underline"),
            t.strikethrough.map(|_| "line-through"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(" ")
        }
    };
    if differs(&deco) && (base.is_some() || deco(s) != "none") {
        out.push(("text-decoration", deco(s)));
    }
    let shift = |t: &TextStyle| px(t.baseline_shift.resolve(t.font_size));
    if differs(&shift) && (base.is_some() || !s.baseline_shift.is_zero()) {
        out.push(("vertical-align", shift(s)));
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
