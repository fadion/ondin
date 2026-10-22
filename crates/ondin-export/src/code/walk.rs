//! The DOM core: layers to elements and declarations, each in the first of the
//! three tiers that draws it exactly — CSS, inline SVG, PNG.
//!
//! **Every element establishes its layer's local space**: its top-left is the
//! layer's box origin, so a child's numbers are the model's numbers. Every element
//! is positioned (`relative` in a flow, `absolute` out of one) so siblings paint in
//! tree order, as the model's z-order does.

use super::Report;
use super::css::{self, px};
use super::dom::{Child, Classes, Element};
use crate::svg::fmt;
use ondin_core::container::{
    self, AlignContent, AlignItems, Dimension, Display, FlexDirection, FlexWrap, GridAutoFlow,
    JustifyContent, LengthPct,
};
use ondin_core::kurbo::{Affine, Rect, Size};
use ondin_core::{
    Brush, Document, EffectKind, Fill, Node, NodeId, NodeKind, Resolved, Stroke, StrokeAlign,
    StrokeSides, TextSizing, TextStyle,
};

/// The page the walk builds, and what it saw on the way.
pub(crate) struct Page {
    pub root: Element,
    pub report: Report,
    /// Pictures the page links to, by path under the export's folder.
    pub assets: Vec<(String, Vec<u8>)>,
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
}

pub(crate) fn page(doc: &Document, res: &Resolved, origins: &[NodeId]) -> Page {
    let view = crate::extent(res, origins).unwrap_or(Rect::new(0.0, 0.0, 100.0, 100.0));
    let mut walk = Walk {
        doc,
        res,
        classes: Classes::default(),
        report: Report::default(),
        assets: Vec::new(),
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
            root.children.push(Child::Element(el));
        }
    }
    Page {
        root,
        report: walk.report,
        assets: walk.assets,
    }
}

struct Walk<'a> {
    doc: &'a Document,
    res: &'a Resolved,
    classes: Classes,
    report: Report,
    assets: Vec<(String, Vec<u8>)>,
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
    fn node(&mut self, id: NodeId, place: Place) -> Option<Element> {
        let doc = self.doc;
        let node = doc.get(id)?;
        if !doc.shown(id) || node.mask() {
            return None;
        }
        match self.css_blocker(node) {
            None => Some(self.css_element(node, place)),
            Some(why) => self.picture(node, place, &why),
        }
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
            for f in &fills {
                if css::background_layer(f.brush.drawn(), size).is_none() {
                    return Some(match f.brush.drawn() {
                        Brush::Image(_) => "an image fill".into(),
                        _ => "a gradient CSS cannot draw".into(),
                    });
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

        let opacity = doc.drawn_opacity(id);
        if opacity < 1.0 {
            el.set("opacity", fmt(f64::from(opacity.max(0.0))));
        }
        self.effects(&mut el, node);

        match &kind {
            NodeKind::Rect { corner_radii, size } => {
                let half = size.width.min(size.height) * 0.5;
                // Clamped per corner as the outline is, so CSS's own proportional
                // scaling never has an overlap to resolve.
                let r = [
                    corner_radii.top_left,
                    corner_radii.top_right,
                    corner_radii.bottom_right,
                    corner_radii.bottom_left,
                ]
                .map(|r| r.clamp(0.0, half.max(0.0)));
                if r.iter().any(|r| *r > 0.0) {
                    let all: Vec<String> = r.iter().map(|r| px(*r)).collect();
                    el.set(
                        "border-radius",
                        if r.iter().all(|x| *x == r[0]) {
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
            NodeKind::Text { .. } => self.text(&mut el, node),
            _ => {}
        }

        if let Some(display) = doc.resolved_display(id)
            && container::is_container(&kind)
        {
            self.display(&mut el, &display);
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
                el.children.push(Child::Element(e));
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
                        el.set("min-width", px(b.width()));
                    }
                    if item.min_height == Dimension::Auto {
                        el.set("min-height", px(b.height()));
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
        }
    }

    /// Width and height as specified: a shape's or a frame's own size, a group's
    /// `LayoutItem` sizes (`auto` hugging, as the model reads it), a text box's
    /// sizing mode. An axis `stretched` between two insets is left to them.
    fn sizes(
        &self,
        el: &mut Element,
        node: &Node,
        kind: &NodeKind,
        bx: Option<Rect>,
        stretched: (bool, bool),
    ) {
        let item = node.item();
        let (w, h): (Option<String>, Option<String>) = match kind {
            NodeKind::Group => {
                if node.display().is_none() {
                    (Some("0".into()), Some("0".into()))
                } else {
                    (Some(hug(item.width)), Some(hug(item.height)))
                }
            }
            NodeKind::Artboard { size } if node.display().is_some() => (
                Some(own_or(item.width, size.width)),
                Some(own_or(item.height, size.height)),
            ),
            NodeKind::Text { sizing, .. } => match sizing {
                TextSizing::Auto => (None, None),
                TextSizing::AutoHeight(w) => (Some(px(*w)), None),
                TextSizing::Fixed(s) => (Some(px(s.width)), Some(px(s.height))),
            },
            _ => {
                let s = bx.map(|b| b.size()).unwrap_or_default();
                (Some(px(s.width)), Some(px(s.height)))
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
        let layers: Vec<String> = paint
            .fills
            .iter()
            .filter(|f| f.visible)
            .rev()
            .filter_map(|f| css::background_layer(f.brush.drawn(), size))
            .collect();
        if !layers.is_empty() {
            el.set("background", layers.join(", "));
        }
        let kind = self.res.used_kind_of(node);
        let Some(s) = paint.strokes.iter().find(|s| s.visible && s.width > 0.0) else {
            return;
        };
        let Brush::Solid(c) = s.brush.drawn() else {
            return;
        };
        let c = css::color(*c, 1.0);
        let sides = ondin_core::geometry::effective_sides(kind, s);
        if sides.is_all() {
            let w = s.width;
            el.set("outline", format!("{} solid {c}", px(w)));
            let offset = match s.align {
                StrokeAlign::Inside => -w,
                StrokeAlign::Center => -w * 0.5,
                StrokeAlign::Outside => 0.0,
            };
            if offset != 0.0 {
                el.set("outline-offset", px(offset));
            }
        } else {
            el.set("box-sizing", "border-box");
            let [t, r, b, l] = side_widths(sides, s.width);
            for (w, prop) in [
                (t, "border-top"),
                (r, "border-right"),
                (b, "border-bottom"),
                (l, "border-left"),
            ] {
                if w > 0.0 {
                    el.set(prop, format!("{} solid {c}", px(w)));
                }
            }
        }
    }

    /// The effect stack as `filter`: blurs first, the one drop shadow after, as
    /// the model casts shadows from the blurred layer.
    fn effects(&mut self, el: &mut Element, node: &Node) {
        let effects = self.doc.resolved_effects(node.id(), node.effects());
        let mut parts = Vec::new();
        for e in effects.iter().filter(|e| e.visible) {
            if let EffectKind::LayerBlur { radius } = e.kind {
                parts.push(format!(
                    "blur({})",
                    px(ondin_core::effect::deviation(radius))
                ));
            }
        }
        for e in effects.iter().filter(|e| e.visible) {
            if let EffectKind::DropShadow(s) = &e.kind {
                parts.push(format!(
                    "drop-shadow({} {} {} {})",
                    px(s.offset.x),
                    px(s.offset.y),
                    px(ondin_core::effect::deviation(s.blur)),
                    css::color(s.color, 1.0)
                ));
            }
        }
        if !parts.is_empty() {
            el.set("filter", parts.join(" "));
        }
    }

    /// A container's own layout declarations.
    fn display(&mut self, el: &mut Element, display: &Display) {
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
        if row_gap != 0.0 {
            el.set("row-gap", px(row_gap));
        }
        if column_gap != 0.0 {
            el.set("column-gap", px(column_gap));
        }
        if pad.iter().any(|p| *p != 0.0) {
            el.set("padding", pad.map(px).join(" "));
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
            .find(|f| f.visible)
            .map(|f| f.brush.drawn().clone());
        match ink {
            Some(Brush::Solid(c)) => el.set("color", css::color(c, 1.0)),
            // Text with no ink is black, as on the canvas.
            _ => el.set("color", "#000000"),
        }
        for (p, v) in face(style, None) {
            el.set(p, v);
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
                    p.children.push(Child::Element(span));
                }
            }
            // An empty paragraph still takes a line.
            if p.children.is_empty() {
                p.children.push(Child::Markup("<br>".into()));
            }
            el.children.push(Child::Element(p));
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
            Place::Free { local, .. } => {
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
                holder.children.push(Child::Element(pic));
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
