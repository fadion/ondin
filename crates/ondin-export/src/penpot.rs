//! Penpot export: the whole document written as a `.penpot` file, Penpot 2's
//! third export format, which `ondin_core::penpot_in` reads back.
//!
//! The file is a zip of JSON: `manifest.json`, then `files/<file>.json`, one
//! `files/<file>/pages/<page>.json` per page and one
//! `files/<file>/pages/<page>/<shape>.json` per shape, with library colours,
//! typographies and components beside them and pictures as
//! `files/<file>/media/<media>.json` naming a stored object under `objects/`.
//!
//! Penpot places every shape on the page: a box shape is its unturned box
//! (`x`, `y`, `width`, `height`) turned by `transform` about the box's centre; a
//! path, a boolean and a group are their outline or their children's extent, on
//! the page and unturned. Each node is written from its world placement, its
//! scale folded into the box so that `transform` is a turn, a skew or a mirror.
//!
//! Penpot's components are Ondin's: a main is a shape on a page, and each layer
//! of a copy names the layer it copies (`shapeRef`). What a copy overrides is
//! listed in `touched`, so Penpot's sync with the main leaves it alone.
//!
//! Ids are UUIDs derived from the node ids, so the same document writes the same
//! file; Penpot gives everything fresh ids when it imports.

use ondin_core::container::{AlignContent, AlignItems, Dimension, Display, JustifyContent};
use ondin_core::kurbo::{Affine, Point, Rect, Shape, Size, Vec2};
use ondin_core::peniko::{Color, Extend, GradientKind};
use ondin_core::{
    BlendMode, BoolOp, Brush, Document, EffectKind, ImageFit, ImageFormat, ImageId, Length,
    MaskMode, Node, NodeId, NodeKind, Operation, Resolved, Stroke, StrokeAlign, TextAlign,
    TextCase, TextSizing, TextStyle, VarId, VarKind, VerticalAlign,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// A written `.penpot` file and what it draws differently.
pub struct PenpotExport {
    /// The zip.
    pub bytes: Vec<u8>,
    /// Features Penpot draws differently or not at all. Deduplicated, in order met.
    pub approximated: Vec<String>,
}

/// The id Penpot gives every page's root frame.
const ROOT: &str = "00000000-0000-0000-0000-000000000000";

/// The data version written: Penpot 2.18's. A reader applies only the later
/// repairs, since the file lists no migrations of its own.
const VERSION: u32 = 67;

/// What the file needs of a reader: linked copies are Penpot's components v2.
const FEATURES: [&str; 2] = ["components/v2", "fdata/shape-data-type"];

/// Write `doc` as a `.penpot` file named `name`.
pub fn export(doc: &Document, res: &Resolved, name: &str) -> std::io::Result<PenpotExport> {
    let file = uuid(&["file", name]);
    let mut w = Writer {
        doc,
        res,
        file: file.clone(),
        entries: Vec::new(),
        media: BTreeMap::new(),
        approximated: Vec::new(),
        colours: HashMap::new(),
        typographies: HashMap::new(),
        components: BTreeMap::new(),
        plain: Vec::new(),
    };
    w.library();
    let mut pages = Vec::new();
    for (index, page) in doc.page_ids().enumerate() {
        let pid = uuid(&["page", &page.to_wire()]);
        let mut objects = Vec::new();
        let top = w.children(page, ROOT, ROOT, &pid, &mut objects);
        let mut root = box_shape(
            ROOT,
            "Root Frame",
            "frame",
            ROOT,
            ROOT,
            &pid,
            &frame_of(Size::new(0.01, 0.01), Affine::IDENTITY),
        );
        root["fills"] = json!([{ "fillColor": "#FFFFFF", "fillOpacity": 1 }]);
        root["strokes"] = json!([]);
        root["shapes"] = ids(&top);
        objects.push((ROOT.to_owned(), root));
        let page_name = doc.get(page).map_or("Page", |n| n.name());
        w.put(
            format!("files/{file}/pages/{pid}.json"),
            &json!({ "id": pid, "name": page_name, "index": index }),
        );
        for (id, shape) in objects {
            w.put(format!("files/{file}/pages/{pid}/{id}.json"), &shape);
        }
        pages.push(pid);
    }
    let components = std::mem::take(&mut w.components);
    for (cid, (main, page)) in components {
        let Some(page) = page else { continue };
        let (path, leaf) = split_name(doc.get(main).map_or("Component", |n| n.name()));
        w.put(
            format!("files/{file}/components/{cid}.json"),
            &json!({
                "id": cid,
                "name": leaf,
                "path": path,
                "mainInstanceId": uuid(&["node", &main.to_wire()]),
                "mainInstancePage": page,
            }),
        );
    }
    w.put(
        format!("files/{file}.json"),
        &json!({
            "id": file,
            "name": name,
            "revn": 1,
            "vern": 0,
            "isShared": false,
            "version": VERSION,
            "features": FEATURES,
            "options": { "componentsV2": true, "baseFontSize": "16px" },
        }),
    );
    w.put(
        "manifest.json".to_owned(),
        &json!({
            "type": "penpot/export-files",
            "version": 1,
            "generatedBy": concat!("ondin/", env!("CARGO_PKG_VERSION")),
            "referer": "ondin",
            "files": [{ "id": file, "name": name, "features": FEATURES }],
            "relations": [],
        }),
    );
    // The manifest first, as Penpot writes it; the rest in the order made.
    w.entries.rotate_right(1);
    let bytes = crate::plan::zip(&w.entries)?;
    Ok(PenpotExport {
        bytes,
        approximated: w.approximated,
    })
}

/// A picture written once: its media id and what a fill names it by.
#[derive(Clone)]
struct Media {
    id: String,
    width: u32,
    height: u32,
    mtype: &'static str,
}

struct Writer<'a> {
    doc: &'a Document,
    res: &'a Resolved,
    file: String,
    entries: Vec<(String, Vec<u8>)>,
    media: BTreeMap<String, Option<Media>>,
    approximated: Vec<String>,
    /// Colour variables written as library colours: their id and the colour.
    colours: HashMap<VarId, (String, Color)>,
    /// Text styles written as typographies: their id and the type they hold.
    typographies: HashMap<ondin_core::style::StyleId, (String, TextStyle)>,
    /// Component id → its main and the page it was written on.
    components: BTreeMap<String, (NodeId, Option<String>)>,
    /// Instance roots written as plain layers, so their layers name no main.
    plain: Vec<NodeId>,
}

/// A shape written: its id and its extent on the page, which a group's own
/// extent is the union of.
struct Written {
    id: String,
    extent: Option<Rect>,
}

impl Writer<'_> {
    fn put(&mut self, name: String, v: &Value) {
        let bytes = serde_json::to_vec_pretty(v).unwrap_or_default();
        self.entries.push((name, bytes));
    }

    /// Penpot draws this differently. Deduplicated by wording.
    fn approximate(&mut self, what: &str) {
        if !self.approximated.iter().any(|s| s == what) {
            self.approximated.push(what.to_owned());
        }
    }

    /// Colour variables as library colours, in the default mode, and text styles
    /// as typographies.
    fn library(&mut self) {
        let (doc, file) = (self.doc, self.file.clone());
        let t = doc.variables();
        let mut written = Vec::new();
        for var in t.iter().filter(|v| v.kind == VarKind::Color) {
            let value = match var.kit() {
                None => t.default_mode().and_then(|m| t.resolve(var.id, m)),
                Some(_) => t.every_value(var.id).into_iter().next(),
            };
            let Some(ondin_core::Value::Color(c)) = value else {
                continue;
            };
            let id = uuid(&["colour", &format!("{:?}", var.id)]);
            let (path, leaf) = split_name(&var.name);
            let (hex, alpha) = hex_alpha(c);
            let mut v = json!({ "id": id, "name": leaf, "color": hex, "opacity": alpha });
            if !path.is_empty() {
                v["path"] = json!(path);
            }
            written.push((format!("files/{file}/colors/{id}.json"), v));
            self.colours.insert(var.id, (id, c));
        }
        if !self.colours.is_empty() && t.modes().len() > 1 {
            self.approximate("Colour variables' other modes, left out");
        }
        if t.iter().any(|v| v.kind != VarKind::Color) {
            self.approximate("Number and text variables, written as their values");
        }
        for style in doc.styles().iter() {
            let ondin_core::style::StyleValue::Text(ts) = &style.value else {
                continue;
            };
            let id = uuid(&["typography", &format!("{:?}", style.id)]);
            let (path, leaf) = split_name(&style.name);
            let mut v = type_attrs(ts);
            v["id"] = json!(id);
            v["name"] = json!(leaf);
            v["path"] = json!(path);
            written.push((format!("files/{file}/typographies/{id}.json"), v));
            self.typographies
                .insert(style.id, (id, ondin_core::style::type_part(ts)));
        }
        for (name, v) in written {
            self.put(name, &v);
        }
    }

    /// `parent`'s children, back to front, each run a mask clips wrapped in a
    /// masked group whose first shape is the mask, as Penpot draws one.
    fn children(
        &mut self,
        parent: NodeId,
        parent_id: &str,
        frame: &str,
        page: &str,
        out: &mut Vec<(String, Value)>,
    ) -> Vec<Written> {
        let doc = self.doc;
        let Some(node) = doc.get(parent) else {
            return Vec::new();
        };
        let mut placed = Vec::new();
        // The open masked group: its id, its mask's name, and what it holds so far.
        let mut run: Option<(String, String, Vec<Written>)> = None;
        for &child in node.children() {
            let Some(c) = doc.get(child) else { continue };
            // A hidden mask masks nothing: it is written as a hidden layer and
            // the run carries on unwrapped, as on the canvas.
            let masks = c.mask() && doc.shown(child) && self.res.mask_path(doc, child).is_some();
            if masks {
                if let Some(open) = run.take() {
                    placed.push(self.masked_group(open, parent_id, frame, page, out));
                }
                if c.mask_mode() != MaskMode::Shape {
                    self.approximate("Alpha and luminance masks, clipped to the mask's shape");
                }
                let gid = uuid(&["mask", &child.to_wire()]);
                let held: Vec<Written> = self
                    .shape(child, &gid, frame, page, out)
                    .into_iter()
                    .collect();
                run = Some((gid, c.name().to_owned(), held));
                continue;
            }
            match &mut run {
                Some((gid, _, held)) => {
                    let gid = gid.clone();
                    held.extend(self.shape(child, &gid, frame, page, out));
                }
                None => placed.extend(self.shape(child, parent_id, frame, page, out)),
            }
        }
        if let Some(open) = run.take() {
            placed.push(self.masked_group(open, parent_id, frame, page, out));
        }
        placed
    }

    /// The group a mask's run is written in: the mask first, then what it clips.
    fn masked_group(
        &mut self,
        (gid, name, held): (String, String, Vec<Written>),
        parent_id: &str,
        frame: &str,
        page: &str,
        out: &mut Vec<(String, Value)>,
    ) -> Written {
        let extent = union(held.iter().filter_map(|w| w.extent));
        let mut g = page_shape(&gid, &name, "group", parent_id, frame, page, extent);
        g["maskedGroup"] = json!(true);
        g["shapes"] = ids(&held);
        out.push((gid.clone(), g));
        Written { id: gid, extent }
    }

    /// One node and what it holds; `None` for one Penpot has no shape for.
    fn shape(
        &mut self,
        id: NodeId,
        parent_id: &str,
        frame: &str,
        page: &str,
        out: &mut Vec<(String, Value)>,
    ) -> Option<Written> {
        let (doc, res) = (self.doc, self.res);
        let node = doc.get(id)?;
        let sid = uuid(&["node", &id.to_wire()]);
        let world = res.world_transform(id).unwrap_or(Affine::IDENTITY);
        let name = node.name();
        let kind = res.used_kind_of(node);
        // A group with a layout has a box of its own, which only a board has in
        // Penpot.
        let laid_group = matches!(kind, NodeKind::Group) && node.display().is_some();
        let (mut shape, extent, paint_space) = match kind {
            NodeKind::Root => return None,
            NodeKind::Slice { .. } => {
                self.approximate("Slices, left out");
                return None;
            }
            NodeKind::Artboard {
                size,
                corner_radii,
                corner_smoothing,
            } => {
                let f = frame_of(*size, world);
                let mut s = box_shape(&sid, name, "frame", parent_id, frame, page, &f);
                radii(&mut s, *corner_radii, &f);
                if *corner_smoothing > 0.0 {
                    self.approximate("Corner smoothing, drawn as round corners");
                }
                s["showContent"] = json!(!node.clip());
                let kids = self.children(id, &sid, &sid, page, out);
                s["shapes"] = ids(&kids);
                (s, Some(f.extent), PaintSpace::Box(local_box(*size)))
            }
            NodeKind::Group if laid_group => {
                self.approximate("Groups with a layout, as boards");
                let size = res.used_frame(id).unwrap_or_default();
                let f = frame_of(size, world);
                let mut s = box_shape(&sid, name, "frame", parent_id, frame, page, &f);
                s["showContent"] = json!(true);
                s["fills"] = json!([]);
                let kids = self.children(id, &sid, &sid, page, out);
                s["shapes"] = ids(&kids);
                (s, Some(f.extent), PaintSpace::None)
            }
            NodeKind::Rect {
                size,
                corner_radii,
                corner_smoothing,
            } => {
                let f = frame_of(*size, world);
                let mut s = box_shape(&sid, name, "rect", parent_id, frame, page, &f);
                radii(&mut s, *corner_radii, &f);
                if *corner_smoothing > 0.0 {
                    self.approximate("Corner smoothing, drawn as round corners");
                }
                (s, Some(f.extent), PaintSpace::Box(local_box(*size)))
            }
            NodeKind::Ellipse { size, arc } if arc.is_whole() => {
                let f = frame_of(*size, world);
                let s = box_shape(&sid, name, "circle", parent_id, frame, page, &f);
                (s, Some(f.extent), PaintSpace::Box(local_box(*size)))
            }
            NodeKind::Ellipse { .. }
            | NodeKind::Polygon { .. }
            | NodeKind::Star { .. }
            | NodeKind::Line { .. }
            | NodeKind::Path { .. } => {
                let mut path = res.local_path(doc, id)?;
                path.apply_affine(world);
                let extent = path.bounding_box();
                let mut s = page_shape(&sid, name, "path", parent_id, frame, page, Some(extent));
                s["content"] = json!(path.to_svg());
                (s, Some(extent), PaintSpace::Page(world, extent))
            }
            NodeKind::Boolean { op } => {
                let kids = self.children(id, &sid, frame, page, out);
                let mut path = res.boolean_path(id).cloned().unwrap_or_default();
                path.apply_affine(world);
                let extent = match path.elements().is_empty() {
                    true => union(kids.iter().filter_map(|k| k.extent)),
                    false => Some(path.bounding_box()),
                };
                let mut s = page_shape(&sid, name, "bool", parent_id, frame, page, extent);
                s["boolType"] = json!(match op {
                    BoolOp::Union => "union",
                    BoolOp::Subtract => "difference",
                    BoolOp::Intersect => "intersection",
                    BoolOp::Exclude => "exclude",
                });
                s["content"] = json!(path.to_svg());
                s["shapes"] = ids(&kids);
                let space = PaintSpace::Page(world, extent.unwrap_or(Rect::ZERO));
                (s, extent, space)
            }
            NodeKind::Group => {
                let kids = self.children(id, &sid, frame, page, out);
                if kids.is_empty() {
                    return None;
                }
                let extent = union(kids.iter().filter_map(|k| k.extent));
                let mut s = page_shape(&sid, name, "group", parent_id, frame, page, extent);
                s["shapes"] = ids(&kids);
                (s, extent, PaintSpace::None)
            }
            NodeKind::Text { .. } => {
                let local = text_box(res, id, kind);
                let f = frame_of(
                    local.size(),
                    world * Affine::translate(local.origin().to_vec2()),
                );
                let mut s = box_shape(&sid, name, "text", parent_id, frame, page, &f);
                self.text(&mut s, id, node, kind, (local, &f));
                (s, Some(f.extent), PaintSpace::Box(local))
            }
        };
        // A text's paints live in its runs; a laid group has none.
        if !matches!(kind, NodeKind::Text { .. } | NodeKind::Group) {
            self.paints(&mut shape, id, node, &paint_space);
        }
        self.common(&mut shape, id, node);
        self.layout(&mut shape, id, node);
        self.item(&mut shape, id, node);
        self.component(&mut shape, id, node, page);
        out.push((sid.clone(), shape));
        Some(Written { id: sid, extent })
    }

    /// What every shape carries: visibility, lock, opacity, blend and effects.
    fn common(&mut self, s: &mut Value, id: NodeId, node: &Node) {
        let doc = self.doc;
        if !doc.shown(id) {
            s["hidden"] = json!(true);
        }
        if node.locked() {
            s["blocked"] = json!(true);
        }
        if node.proportions_locked() {
            s["proportionLock"] = json!(true);
        }
        if node.opacity() < 1.0 {
            s["opacity"] = json!(node.opacity());
        }
        match node.blend() {
            BlendMode::Normal | BlendMode::PassThrough => {}
            b => s["blendMode"] = json!(b.css()),
        }
        let effects = doc.resolved_effects(id, node.effects());
        let mut shadows = Vec::new();
        let (mut blur, mut backdrop) = (false, false);
        for (i, e) in effects.iter().enumerate() {
            let eid = uuid(&["effect", &id.to_wire(), &i.to_string()]);
            match &e.kind {
                EffectKind::DropShadow(sh) | EffectKind::InnerShadow(sh) => {
                    if sh.blend != BlendMode::Normal {
                        self.approximate("Shadow blend modes");
                    }
                    let (hex, alpha) = hex_alpha(sh.color);
                    shadows.push(json!({
                        "id": eid,
                        "style": match e.kind {
                            EffectKind::InnerShadow(_) => "inner-shadow",
                            _ => "drop-shadow",
                        },
                        "offsetX": sh.offset.x,
                        "offsetY": sh.offset.y,
                        "blur": sh.blur,
                        "spread": sh.spread,
                        "hidden": !e.visible,
                        "color": { "color": hex, "opacity": alpha },
                    }));
                }
                // Penpot's layer blur value is the deviation; Ondin's is a radius.
                EffectKind::LayerBlur { radius } if !blur => {
                    blur = true;
                    s["blur"] = json!({
                        "id": eid,
                        "type": "layer-blur",
                        "value": radius * ondin_core::BLUR_DEVIATION,
                        "hidden": !e.visible,
                    });
                }
                EffectKind::BackgroundBlur { radius } if !backdrop => {
                    backdrop = true;
                    s["backgroundBlur"] = json!({
                        "id": eid,
                        "type": "background-blur",
                        "value": radius,
                        "hidden": !e.visible,
                    });
                }
                EffectKind::LayerBlur { .. } | EffectKind::BackgroundBlur { .. } => {
                    self.approximate("A second blur of the same kind, left out");
                }
                EffectKind::Filters(_) => self.approximate("Colour filters, left out"),
            }
        }
        if !shadows.is_empty() {
            s["shadow"] = Value::Array(shadows);
        }
    }

    /// A shape's fills and strokes, topmost first as Penpot lists them; a paint
    /// bound to a colour variable names its library colour.
    fn paints(&mut self, s: &mut Value, id: NodeId, node: &Node, space: &PaintSpace) {
        let Some(paint) = ondin_core::paint_box::node_paint(self.doc, self.res, id) else {
            return;
        };
        let own = node.paint();
        let mut fills = Vec::new();
        for (i, f) in paint.fills.iter().enumerate().rev() {
            if !f.visible {
                continue;
            }
            let bound = own.fills.get(i).and_then(|o| o.brush.var());
            if let Some(v) = self.colour(f.brush.drawn(), bound, "fill", space) {
                fills.push(v);
            }
        }
        let mut strokes = Vec::new();
        for (i, st) in paint.strokes.iter().enumerate().rev() {
            let bound = own.strokes.get(i).and_then(|o| o.brush.var());
            if let Some(v) = self.stroke(st, bound, space) {
                strokes.push(v);
            }
        }
        s["fills"] = Value::Array(fills);
        s["strokes"] = Value::Array(strokes);
    }

    fn stroke(&mut self, st: &Stroke, bound: Option<VarId>, space: &PaintSpace) -> Option<Value> {
        let mut v = self.colour(st.brush.drawn(), bound, "stroke", space)?;
        v["strokeWidth"] = json!(st.width);
        v["strokeAlignment"] = json!(match st.align {
            StrokeAlign::Inside => "inner",
            StrokeAlign::Outside => "outer",
            StrokeAlign::Center => "center",
        });
        if !st.visible {
            v["hidden"] = json!(true);
        }
        match st.dashes.as_slice() {
            [] | [_] => v["strokeStyle"] = json!("solid"),
            // Penpot's dotted pattern is its own: round dots at width + 5.
            [on, gap] if *on == 0.0 => {
                v["strokeStyle"] = json!("dotted");
                if (gap - (st.width + 5.0)).abs() > 0.5 {
                    self.approximate("Dot spacing, drawn at Penpot's own");
                }
            }
            [on, off, rest @ ..] => {
                v["strokeStyle"] = json!("dashed");
                v["strokeDash"] = json!(on);
                v["strokeGap"] = json!(off);
                if !rest.is_empty() {
                    self.approximate("Dash patterns of more than one dash, drawn with the first");
                }
            }
        }
        if st.dash_offset != 0.0 && !st.dashes.is_empty() {
            self.approximate("Dash offsets");
        }
        let cap = match st.cap {
            ondin_core::kurbo::Cap::Round => Some("round"),
            ondin_core::kurbo::Cap::Square => Some("square"),
            ondin_core::kurbo::Cap::Butt => None,
        };
        if let Some(cap) = cap {
            v["strokeCapStart"] = json!(cap);
            v["strokeCapEnd"] = json!(cap);
        }
        if st.join != ondin_core::kurbo::Join::Miter {
            self.approximate("Round and bevel stroke joins, drawn mitred");
        }
        if !st.sides.is_all() {
            self.approximate("Strokes on some sides only, drawn on all four");
        }
        Some(v)
    }

    /// A brush as a Penpot fill or stroke colour, `key` being `fill` or `stroke`.
    /// A solid drawn in its variable's library colour names it.
    fn colour(
        &mut self,
        brush: &Brush,
        bound: Option<VarId>,
        key: &str,
        space: &PaintSpace,
    ) -> Option<Value> {
        match brush {
            Brush::Solid(c) => {
                let (hex, alpha) = hex_alpha(*c);
                let mut v = json!({ format!("{key}Color"): hex, format!("{key}Opacity"): alpha });
                // Bound only where the node draws the library's value: a node in
                // another mode draws another colour.
                if let Some((cid, lib)) = bound.and_then(|var| self.colours.get(&var))
                    && hex_alpha(*lib).0 == hex
                {
                    v[format!("{key}ColorRefId")] = json!(cid);
                    v[format!("{key}ColorRefFile")] = json!(self.file);
                }
                Some(v)
            }
            Brush::Gradient(g) => {
                let to_frac = space.to_fraction()?;
                let (gradient, opacity) = self.gradient(g, to_frac)?;
                Some(json!({
                    format!("{key}ColorGradient"): gradient,
                    format!("{key}Opacity"): opacity,
                }))
            }
            Brush::Image(img) => {
                let media = self.picture(&img.image.id)?;
                let keep = match img.image.fit {
                    ImageFit::Fill => true,
                    ImageFit::Crop if img.image.crop == ondin_core::whole_crop() => false,
                    ImageFit::Crop => {
                        self.approximate("Cropped pictures, shown whole");
                        false
                    }
                    ImageFit::Fit => {
                        self.approximate("Pictures fitted inside their frame, drawn covering it");
                        true
                    }
                    ImageFit::Tile => {
                        self.approximate("Tiled pictures, drawn covering their frame");
                        true
                    }
                };
                if !img.image.adjust.is_default() {
                    self.approximate("Picture adjustments, left out");
                }
                if !img.image.orient.is_default() {
                    self.approximate("Picture flips and turns, left out");
                }
                Some(json!({
                    format!("{key}Image"): {
                        "id": media.id,
                        "width": media.width,
                        "height": media.height,
                        "mtype": media.mtype,
                        "keepAspectRatio": keep,
                    },
                    format!("{key}Opacity"): img.sampler.alpha.clamp(0.0, 1.0),
                }))
            }
        }
    }

    /// A gradient in Penpot's terms: its points as fractions of the shape's box
    /// (`to_frac` maps the shape's local space there), and its opacity.
    fn gradient(&mut self, g: &ondin_core::GradientBrush, to_frac: Affine) -> Option<(Value, f32)> {
        if g.gradient.extend != Extend::Pad {
            self.approximate("Repeating and reflected gradients, drawn padded");
        }
        let stops: Vec<Value> = g
            .gradient
            .stops
            .iter()
            .map(|s| {
                let c = s.color.to_alpha_color::<ondin_core::peniko::color::Srgb>();
                let (hex, alpha) = hex_alpha(c);
                json!({ "color": hex, "opacity": alpha, "offset": s.offset.clamp(0.0, 1.0) })
            })
            .collect();
        if stops.is_empty() {
            return None;
        }
        let f = to_frac * g.transform;
        let gradient = match g.gradient.kind {
            GradientKind::Linear(l) => {
                let (s, e) = (f * l.start, f * l.end);
                json!({
                    "type": "linear",
                    "startX": s.x, "startY": s.y, "endX": e.x, "endY": e.y,
                    "width": 1, "stops": stops,
                })
            }
            GradientKind::Radial(r) => {
                if r.start_radius > 0.0 || r.start_center != r.end_center {
                    self.approximate("Two-point radial gradients, drawn from one centre");
                }
                let e = f
                    * Affine::translate(r.end_center.to_vec2())
                    * Affine::scale(f64::from(r.end_radius));
                let (centre, end, width) = ellipse_axes(e)?;
                json!({
                    "type": "radial",
                    "startX": centre.x, "startY": centre.y, "endX": end.x, "endY": end.y,
                    "width": width, "stops": stops,
                })
            }
            GradientKind::Sweep(s) => {
                self.approximate("Angular gradients, drawn linear");
                let c = f * s.center;
                let e = f * (s.center + Vec2::new(1.0, 0.0));
                json!({
                    "type": "linear",
                    "startX": c.x, "startY": c.y, "endX": e.x, "endY": e.y,
                    "width": 1, "stops": stops,
                })
            }
        };
        Some((gradient, g.opacity.clamp(0.0, 1.0)))
    }

    /// A picture's media entry, written once. A linked picture has no bytes here
    /// and is left out.
    fn picture(&mut self, image: &ImageId) -> Option<Media> {
        if let Some(known) = self.media.get(&image.0) {
            return known.clone();
        }
        let entry = self.doc.image(image);
        let made = match entry.and_then(|e| e.bytes().map(|b| (e, b))) {
            None => {
                self.approximate("Linked or missing pictures, left out");
                None
            }
            Some((entry, bytes)) => {
                let (mtype, ext) = match entry.format {
                    ImageFormat::Png => ("image/png", "png"),
                    ImageFormat::Jpeg => ("image/jpeg", "jpg"),
                    ImageFormat::Webp => ("image/webp", "webp"),
                    ImageFormat::Gif => ("image/gif", "gif"),
                };
                let media = uuid(&["media", &image.0]);
                let object = uuid(&["object", &image.0]);
                let file = self.file.clone();
                self.put(
                    format!("objects/{object}.json"),
                    &json!({
                        "id": object,
                        "size": bytes.len(),
                        "contentType": mtype,
                        "bucket": "file-media-object",
                    }),
                );
                self.entries
                    .push((format!("objects/{object}.{ext}"), bytes.to_vec()));
                self.put(
                    format!("files/{file}/media/{media}.json"),
                    &json!({
                        "id": media,
                        "name": image.0,
                        "width": entry.width,
                        "height": entry.height,
                        "mtype": mtype,
                        "mediaId": object,
                    }),
                );
                Some(Media {
                    id: media,
                    width: entry.width,
                    height: entry.height,
                    mtype,
                })
            }
        };
        self.media.insert(image.0.clone(), made.clone());
        made
    }

    /// A text's content: its lines as paragraphs, each run a leaf with its own
    /// type and fills. The node's fills draw every run without a colour of its own.
    ///
    /// Its `positionData` places each line's runs where Ondin set them, `local`
    /// being the text's box in its own space and `f` that box on the page: Penpot
    /// draws a text from it (stretching each run to its width), and without it a
    /// viewer shows nothing until the workspace sets the text again.
    fn text(
        &mut self,
        s: &mut Value,
        id: NodeId,
        node: &Node,
        kind: &NodeKind,
        (local, f): (Rect, &Frame),
    ) {
        let NodeKind::Text {
            content,
            style,
            spans,
            para_spans,
            paragraph,
            block,
            sizing,
            on_path,
            ..
        } = kind
        else {
            return;
        };
        if on_path.is_some() {
            self.approximate("Text on a path, set straight");
        }
        let space = PaintSpace::Box(local);
        let mut node_fills = Vec::new();
        if let Some(paint) = ondin_core::paint_box::node_paint(self.doc, self.res, id) {
            let own = node.paint();
            for (i, f) in paint.fills.iter().enumerate().rev() {
                let bound = own.fills.get(i).and_then(|o| o.brush.var());
                if f.visible
                    && let Some(v) = self.colour(f.brush.drawn(), bound, "fill", &space)
                {
                    node_fills.push(v);
                }
            }
            let strokes: Vec<Value> = paint
                .strokes
                .iter()
                .enumerate()
                .rev()
                .filter_map(|(i, st)| {
                    let bound = own.strokes.get(i).and_then(|o| o.brush.var());
                    self.stroke(st, bound, &space)
                })
                .collect();
            s["strokes"] = Value::Array(strokes);
        }
        let typography = node
            .styles()
            .text
            .and_then(|sid| self.typographies.get(&sid).cloned());
        let runs = spans.runs(style, content.len());
        let paras = para_spans.runs(paragraph, content.len());
        let mut paragraphs = Vec::new();
        let mut start = 0;
        for line in content.split('\n') {
            let end = start + line.len();
            let align = paras
                .iter()
                .find(|(r, _)| r.start <= start && start <= r.end)
                .map_or(paragraph.align, |(_, p)| p.align);
            let mut leaves = Vec::new();
            for (range, run) in &runs {
                let (a, b) = (range.start.max(start), range.end.min(end));
                if a < b {
                    leaves.push(self.leaf(&content[a..b], run, &node_fills, typography.as_ref()));
                }
            }
            // An empty line still holds one leaf, in the type at its start.
            if leaves.is_empty() {
                let at = runs
                    .iter()
                    .find(|(r, _)| r.start <= start && start <= r.end)
                    .map_or(style.as_ref(), |(_, t)| t);
                leaves.push(self.leaf("", at, &node_fills, typography.as_ref()));
            }
            let mut p = leaves[0].clone();
            if let Some(map) = p.as_object_mut() {
                map.remove("text");
            }
            p["type"] = json!("paragraph");
            p["textAlign"] = json!(match align {
                TextAlign::Center => "center",
                TextAlign::End | TextAlign::Right => "right",
                TextAlign::Justify => "justify",
                TextAlign::Start | TextAlign::Left => "left",
            });
            p["children"] = Value::Array(leaves);
            paragraphs.push(p);
            start = end + 1;
        }
        s["content"] = json!({
            "type": "root",
            "verticalAlign": match block.vertical_align {
                VerticalAlign::Middle => "center",
                VerticalAlign::Bottom => "bottom",
                _ => "top",
            },
            "children": [{ "type": "paragraph-set", "children": paragraphs }],
        });
        s["growType"] = json!(match sizing {
            TextSizing::Auto => "auto-width",
            TextSizing::AutoHeight(_) => "auto-height",
            TextSizing::Fixed(_) => "fixed",
        });
        let Some(parts) = ondin_core::TextRef::of(kind) else {
            return;
        };
        let page = |x: f64, y: f64| {
            (
                f.rect.x0 + (x - local.x0) * f.scale.x,
                f.rect.y0 + (y - local.y0) * f.scale.y,
            )
        };
        let mut placed = Vec::new();
        for frag in ondin_core::text::line_runs(parts) {
            let text = content
                .get(frag.range.clone())
                .unwrap_or("")
                .trim_end_matches(['\n', '\r']);
            // Penpot stretches each run to its width, which smears a lone space.
            if text.trim().is_empty() {
                continue;
            }
            let run = spans.resolve(style, frag.range.start);
            let (x, bottom) = page(frag.x, frag.baseline + frag.descent);
            let mut v = type_attrs(&run);
            let fills = match run.color {
                Some(c) => {
                    let (hex, alpha) = hex_alpha(c);
                    vec![json!({ "fillColor": hex, "fillOpacity": alpha })]
                }
                None => node_fills.clone(),
            };
            v["x"] = json!(x);
            v["y"] = json!(bottom);
            v["width"] = json!(frag.width * f.scale.x);
            v["height"] = json!((frag.ascent + frag.descent) * f.scale.y);
            v["text"] = json!(text);
            v["fontSize"] = json!(format!("{}px", num(run.font_size)));
            v["textDecoration"] = json!(match (&run.underline, &run.strikethrough) {
                (Some(_), _) => "underline",
                (None, Some(_)) => "line-through",
                (None, None) => "none",
            });
            v["fills"] = Value::Array(fills);
            placed.push(v);
        }
        s["positionData"] = Value::Array(placed);
    }

    /// One run of type as a Penpot text leaf, naming its typography while it
    /// still sets in it.
    fn leaf(
        &mut self,
        text: &str,
        t: &TextStyle,
        node_fills: &[Value],
        typography: Option<&(String, TextStyle)>,
    ) -> Value {
        let fills = match t.color {
            Some(c) => {
                let (hex, alpha) = hex_alpha(c);
                vec![json!({ "fillColor": hex, "fillOpacity": alpha })]
            }
            None => node_fills.to_vec(),
        };
        if !t.variations.is_empty() || !t.features.is_empty() {
            self.approximate("OpenType features and variable axes, left out");
        }
        if !t.word_spacing.is_zero() {
            self.approximate("Word spacing, left out");
        }
        if !t.baseline_shift.is_zero() {
            self.approximate("Baseline shift, left out");
        }
        if t.underline.is_some() && t.strikethrough.is_some() {
            self.approximate("Underline with strikethrough, drawn underlined");
        }
        let mut v = type_attrs(t);
        v["text"] = json!(text);
        v["textDecoration"] = json!(match (&t.underline, &t.strikethrough) {
            (Some(_), _) => "underline",
            (None, Some(_)) => "line-through",
            (None, None) => "none",
        });
        v["fills"] = Value::Array(fills);
        if let Some((tid, typed)) = typography
            && *typed == ondin_core::style::type_part(t)
        {
            v["typographyRefId"] = json!(tid);
            v["typographyRefFile"] = json!(self.file);
        }
        v
    }

    /// A container's flex layout as Penpot's. Grid has no counterpart Penpot reads
    /// the same way, so a grid's children keep their drawn places.
    fn layout(&mut self, s: &mut Value, id: NodeId, node: &Node) {
        if node.display().is_none() {
            return;
        }
        let display = self.doc.resolved_display(id);
        let Some(Display::Flex(f)) = display.as_deref() else {
            self.approximate("Grid layout, kept at drawn positions");
            return;
        };
        use ondin_core::container::{FlexDirection, FlexWrap};
        s["layout"] = json!("flex");
        s["layoutFlexDir"] = json!(match f.direction {
            FlexDirection::Row => "row",
            FlexDirection::RowReverse => "row-reverse",
            FlexDirection::Column => "column",
            FlexDirection::ColumnReverse => "column-reverse",
        });
        s["layoutWrapType"] = json!(match f.wrap {
            FlexWrap::NoWrap => "nowrap",
            FlexWrap::Wrap => "wrap",
            FlexWrap::WrapReverse => {
                self.approximate("Reversed wrapping, wrapped forwards");
                "wrap"
            }
        });
        s["layoutJustifyContent"] = json!(match f.justify_content {
            JustifyContent::Start => "start",
            JustifyContent::End => "end",
            JustifyContent::Center => "center",
            JustifyContent::SpaceBetween => "space-between",
            JustifyContent::SpaceAround => "space-around",
            JustifyContent::SpaceEvenly => "space-evenly",
        });
        s["layoutAlignItems"] = json!(self.align(f.align_items));
        s["layoutAlignContent"] = json!(match f.align_content {
            AlignContent::Stretch => "stretch",
            AlignContent::Start => "start",
            AlignContent::End => "end",
            AlignContent::Center => "center",
            AlignContent::SpaceBetween => "space-between",
            AlignContent::SpaceAround => "space-around",
            AlignContent::SpaceEvenly => "space-evenly",
        });
        s["layoutGap"] = json!({ "rowGap": f.row_gap, "columnGap": f.column_gap });
        s["layoutGapType"] = json!("multiple");
        let [top, right, bottom, left] = f.padding;
        s["layoutPadding"] = json!({ "p1": top, "p2": right, "p3": bottom, "p4": left });
        let simple = (top - bottom).abs() < 1e-9 && (right - left).abs() < 1e-9;
        s["layoutPaddingType"] = json!(if simple { "simple" } else { "multiple" });
    }

    fn align(&mut self, a: AlignItems) -> &'static str {
        match a {
            AlignItems::Stretch => "stretch",
            AlignItems::Start => "start",
            AlignItems::End => "end",
            AlignItems::Center => "center",
            AlignItems::Baseline => {
                self.approximate("Baseline alignment, aligned to the start");
                "start"
            }
        }
    }

    /// A layer's place in its container: its sizing in a flex layout, or its
    /// constraints in a frame without one.
    fn item(&mut self, s: &mut Value, id: NodeId, node: &Node) {
        let doc = self.doc;
        let Some(parent) = node.parent().and_then(|p| doc.get(p)) else {
            return;
        };
        let item = doc.resolved_item(id);
        let insets = node.insets();
        let flex = match doc.resolved_display(parent.id()).as_deref() {
            Some(Display::Flex(f)) => Some(*f),
            _ => None,
        };
        // Whether the layer's size on an axis follows its content: a frame hugs
        // only when asked to (`fit-content`), a group with a layout unless it has
        // a typed size, and a text by its sizing mode.
        let kind = self.res.used_kind_of(node);
        let hugs = |d: Dimension, horizontal: bool| match kind {
            NodeKind::Artboard { .. } => d == Dimension::FitContent,
            NodeKind::Group => matches!(d, Dimension::Auto | Dimension::FitContent),
            NodeKind::Text { sizing, .. } => match sizing {
                TextSizing::Auto => true,
                TextSizing::AutoHeight(_) => !horizontal,
                TextSizing::Fixed(_) => false,
            },
            _ => false,
        };
        let (hug_w, hug_h) = (hugs(item.width, true), hugs(item.height, false));
        if node.display().is_some() {
            s["layoutItemHSizing"] = json!(if hug_w { "auto" } else { "fix" });
            s["layoutItemVSizing"] = json!(if hug_h { "auto" } else { "fix" });
        }
        let Some(f) = flex else {
            self.constraints(s, insets);
            return;
        };
        if insets.is_authored() {
            s["layoutItemAbsolute"] = json!(true);
            self.constraints(s, insets);
            return;
        }
        let row = f.direction.is_row();
        let (main_hugs, (cross, cross_hugs)) = if row {
            (hug_w, (item.height, hug_h))
        } else {
            (hug_h, (item.width, hug_w))
        };
        let stretched = match item.align_self {
            Some(a) => a == AlignItems::Stretch,
            None => f.align_items == AlignItems::Stretch,
        };
        let sizing = |fills: bool, hugs: bool| match (fills, hugs) {
            (true, _) => "fill",
            (false, true) => "auto",
            (false, false) => "fix",
        };
        let main_sizing = sizing(item.grow > 0.0, main_hugs);
        // A stretch sets the cross size only where none is typed, as CSS's does.
        let cross_typed = matches!(cross, Dimension::Px(_) | Dimension::Percent(_));
        let cross_sizing = sizing(stretched && !cross_typed, cross_hugs);
        let (h, v) = if row {
            (main_sizing, cross_sizing)
        } else {
            (cross_sizing, main_sizing)
        };
        s["layoutItemHSizing"] = json!(h);
        s["layoutItemVSizing"] = json!(v);
        if matches!(item.width, Dimension::Percent(_))
            || matches!(item.height, Dimension::Percent(_))
        {
            self.approximate("Sizes in percent, fixed at their drawn size");
        }
        for (key, d) in [
            ("layoutItemMinW", item.min_width),
            ("layoutItemMaxW", item.max_width),
            ("layoutItemMinH", item.min_height),
            ("layoutItemMaxH", item.max_height),
        ] {
            if let Dimension::Px(px) = d {
                s[key] = json!(px);
            }
        }
        if let Some(a) = item.align_self {
            s["layoutItemAlignSelf"] = json!(self.align(a));
        }
    }

    /// A layer's insets as Penpot's constraints: which edges it keeps its
    /// distance to when its frame is resized.
    fn constraints(&mut self, s: &mut Value, insets: &ondin_core::Insets) {
        use ondin_core::LengthPct;
        let axis =
            |a: Option<LengthPct>, b: Option<LengthPct>, centred: bool, ends: [&'static str; 3]| {
                match (a, b) {
                    (Some(LengthPct::Percent(_)), _) | (_, Some(LengthPct::Percent(_))) => "scale",
                    (Some(_), Some(_)) if centred => "center",
                    (Some(_), Some(_)) => ends[2],
                    (None, Some(_)) => ends[1],
                    _ => ends[0],
                }
            };
        let m = insets.margin_auto;
        s["constraintsH"] = json!(axis(
            insets.left,
            insets.right,
            m.left && m.right,
            ["left", "right", "leftright"],
        ));
        s["constraintsV"] = json!(axis(
            insets.top,
            insets.bottom,
            m.top && m.bottom,
            ["top", "bottom", "topbottom"],
        ));
    }

    /// A main's and a copy's component attributes. A copy Penpot could not hold
    /// as one — with layers of its own, a swap, or a main left out of the file —
    /// is written as plain layers.
    fn component(&mut self, s: &mut Value, id: NodeId, node: &Node, page: &str) {
        let doc = self.doc;
        if node.component() && !doc.in_vault(id) {
            let cid = uuid(&["component", &id.to_wire()]);
            s["componentId"] = json!(cid);
            s["componentFile"] = json!(self.file);
            s["componentRoot"] = json!(true);
            s["mainInstance"] = json!(true);
            if node.set().is_some() || !node.variant().is_empty() {
                self.approximate("Variants, as separate components");
            }
            self.components.insert(cid, (id, Some(page.to_owned())));
        }
        let Some(link) = node.link() else { return };
        let Some(root) = ondin_core::component::instance_root(doc, id) else {
            return;
        };
        if self.plain.contains(&root) || !self.whole_copy(root) {
            if !self.plain.contains(&root) {
                self.plain.push(root);
                self.approximate(
                    "Instances with layers of their own, swapped, or of a kit's main, as plain layers",
                );
            }
            return;
        }
        if root == id
            && let Some(main) = ondin_core::component::main_of(doc, id)
        {
            s["componentId"] = json!(uuid(&["component", &main.to_wire()]));
            s["componentFile"] = json!(self.file);
            s["componentRoot"] = json!(true);
        }
        s["shapeRef"] = json!(uuid(&["node", &link.to_wire()]));
        let touched = touched(&ondin_core::reset::overrides(doc, id));
        if !touched.is_empty() {
            s["touched"] = json!(touched);
        }
    }

    /// Whether the instance at `root` copies its main layer for layer: every
    /// layer linked, none swapped, and the main on a page of this file.
    fn whole_copy(&self, root: NodeId) -> bool {
        let doc = self.doc;
        let Some(main) = ondin_core::component::main_of(doc, root) else {
            return false;
        };
        if doc.in_vault(main) {
            return false;
        }
        let mut stack = vec![root];
        while let Some(at) = stack.pop() {
            let Some(n) = doc.get(at) else { return false };
            if n.link().is_none() || n.swap().is_some() {
                return false;
            }
            stack.extend(n.children().iter().copied());
        }
        true
    }
}

/// The groups of a copy's overrides as Penpot names them in `touched`.
fn touched(overrides: &[ondin_core::reset::Override]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for o in overrides {
        let groups: &[&'static str] = match &o.reset {
            Operation::SetName { .. } => &["name-group"],
            Operation::SetFills { .. } | Operation::SetFillStyle { .. } => &["fill-group"],
            Operation::SetStrokes { .. } | Operation::SetStrokeStyle { .. } => &["stroke-group"],
            Operation::SetEffects { .. } | Operation::SetEffectsStyle { .. } => {
                &["shadow-group", "blur-group"]
            }
            Operation::SetOpacity { .. } | Operation::SetBlend { .. } => &["layer-effects-group"],
            Operation::SetVisible { .. } => &["visibility-group"],
            Operation::SetText { .. }
            | Operation::SetTextStyle { .. }
            | Operation::SetTypeStyle { .. }
            | Operation::SetParagraphStyle { .. }
            | Operation::SetBlockStyle { .. } => {
                &["content-group", "text-font-group", "text-display-group"]
            }
            Operation::SetGeometry { .. } | Operation::SetTransform { .. } => {
                &["geometry-group", "radius-group"]
            }
            Operation::SetClip { .. } => &["show-content"],
            Operation::SetMask { .. } | Operation::SetMaskMode { .. } => &["mask-group"],
            Operation::SetInsets { .. } => &["constraints-group"],
            Operation::SetDisplay { .. } | Operation::SetLayoutStyle { .. } => &[
                "layout-container",
                "layout-flex-dir",
                "layout-gap",
                "layout-padding",
                "layout-justify-content",
                "layout-align-items",
                "layout-align-content",
                "layout-wrap-type",
            ],
            Operation::SetLayoutItem { .. } => &[
                "layout-item-h-sizing",
                "layout-item-v-sizing",
                "layout-item-min-w",
                "layout-item-max-w",
                "layout-item-min-h",
                "layout-item-max-h",
                "layout-item-align-self",
            ],
            _ => &[],
        };
        for g in groups {
            if !out.contains(g) {
                out.push(g);
            }
        }
    }
    out
}

/// A run's type as Penpot's text attributes, which a typography holds too.
fn type_attrs(t: &TextStyle) -> Value {
    let size = t.font_size;
    let line_height = match t.line_height {
        Some(Length::Em(m)) => m,
        Some(Length::Px(px)) if size > 0.0 => px / size,
        // The font's own height, which Penpot has no word for.
        _ => 1.2,
    };
    json!({
        "fontId": font_id(&t.font_family),
        "fontFamily": t.font_family,
        "fontVariantId": variant_id(t.weight, t.italic),
        "fontSize": num(size),
        "fontWeight": t.weight.to_string(),
        "fontStyle": if t.italic { "italic" } else { "normal" },
        "lineHeight": num(line_height),
        "letterSpacing": num(t.letter_spacing.resolve(size)),
        "textTransform": match t.case {
            TextCase::Upper => "uppercase",
            TextCase::Lower => "lowercase",
            TextCase::Title => "capitalize",
            TextCase::Original => "none",
        },
    })
}

/// The space a shape's gradients are measured in: its local box, or (for a shape
/// written on the page) its world placement and its extent there.
enum PaintSpace {
    Box(Rect),
    Page(Affine, Rect),
    None,
}

impl PaintSpace {
    /// Local space → fractions of the box Penpot measures gradients in.
    fn to_fraction(&self) -> Option<Affine> {
        let onto = |r: &Rect| {
            let (w, h) = (r.width(), r.height());
            (w > 1e-9 && h > 1e-9).then(|| {
                Affine::scale_non_uniform(1.0 / w, 1.0 / h)
                    * Affine::translate(-r.origin().to_vec2())
            })
        };
        match self {
            PaintSpace::Box(r) => onto(r),
            PaintSpace::Page(world, r) => onto(r).map(|f| f * *world),
            PaintSpace::None => None,
        }
    }
}

/// A box shape's placement: its box on the page, the turn about its centre, and
/// the factor its local lengths were scaled by into that box.
struct Frame {
    /// The unturned box, on the page.
    rect: Rect,
    /// The turn, skew or mirror about the box's centre: no translation, no scale.
    turn: Affine,
    /// Local lengths → box lengths, per axis.
    scale: Vec2,
    /// The four corners' bounding box, on the page.
    extent: Rect,
    /// The four corners, clockwise from the box's top left.
    points: [Point; 4],
}

/// Where a box of `size` placed by `world` sits in Penpot's terms. The world's
/// linear part is split into a scale, folded into the box, and what is left.
fn frame_of(size: Size, world: Affine) -> Frame {
    let [a, b, c, d, _, _] = world.as_coeffs();
    let sx = Vec2::new(a, b).hypot();
    let (cos, sin) = if sx > 1e-12 {
        (a / sx, b / sx)
    } else {
        (1.0, 0.0)
    };
    // The second column's part across the first: negative when mirrored.
    let sy = -sin * c + cos * d;
    let sx = if sx > 1e-12 { sx } else { 1.0 };
    let sy = if sy.abs() > 1e-12 { sy.abs() } else { 1.0 };
    let turn = Affine::new([a / sx, b / sx, c / sy, d / sy, 0.0, 0.0]);
    let (w, h) = (size.width * sx, size.height * sy);
    let centre = world * Point::new(size.width / 2.0, size.height / 2.0);
    let rect = Rect::from_center_size(centre, (w, h));
    let about = Affine::translate(centre.to_vec2()) * turn * Affine::translate(-centre.to_vec2());
    let points = [
        about * Point::new(rect.x0, rect.y0),
        about * Point::new(rect.x1, rect.y0),
        about * Point::new(rect.x1, rect.y1),
        about * Point::new(rect.x0, rect.y1),
    ];
    let extent = points
        .iter()
        .fold(Rect::from_points(points[0], points[0]), |r, p| {
            r.union_pt(*p)
        });
    Frame {
        rect,
        turn,
        scale: Vec2::new(sx, sy),
        extent,
        points,
    }
}

/// A box shape: geometry, turn and the ids that place it in the tree.
fn box_shape(
    id: &str,
    name: &str,
    kind: &str,
    parent: &str,
    frame: &str,
    page: &str,
    f: &Frame,
) -> Value {
    let r = f.rect;
    let [a, b, ..] = f.turn.as_coeffs();
    let mirrored = f.turn.determinant() < 0.0;
    let rotation = match mirrored {
        true => 0.0,
        false => b.atan2(a).to_degrees().rem_euclid(360.0),
    };
    json!({
        "id": id,
        "name": name,
        "type": kind,
        "x": r.x0,
        "y": r.y0,
        "width": r.width(),
        "height": r.height(),
        "rotation": rotation,
        "selrect": selrect(r),
        "points": f.points.iter().map(|p| json!({ "x": p.x, "y": p.y })).collect::<Vec<_>>(),
        "transform": matrix(f.turn),
        "transformInverse": matrix(f.turn.inverse()),
        "parentId": parent,
        "frameId": frame,
        "pageId": page,
        "flipX": if mirrored { json!(true) } else { Value::Null },
        "flipY": Value::Null,
        "proportion": if r.height() > 0.0 { r.width() / r.height() } else { 1.0 },
        "proportionLock": false,
    })
}

/// A shape placed by its extent on the page, unturned: a path, boolean or group.
fn page_shape(
    id: &str,
    name: &str,
    kind: &str,
    parent: &str,
    frame: &str,
    page: &str,
    extent: Option<Rect>,
) -> Value {
    let r = extent.unwrap_or(Rect::ZERO);
    let mut s = json!({
        "id": id,
        "name": name,
        "type": kind,
        "rotation": 0,
        "selrect": selrect(r),
        "points": [
            { "x": r.x0, "y": r.y0 }, { "x": r.x1, "y": r.y0 },
            { "x": r.x1, "y": r.y1 }, { "x": r.x0, "y": r.y1 },
        ],
        "transform": matrix(Affine::IDENTITY),
        "transformInverse": matrix(Affine::IDENTITY),
        "parentId": parent,
        "frameId": frame,
        "pageId": page,
        "proportion": if r.height() > 0.0 { r.width() / r.height() } else { 1.0 },
        "proportionLock": false,
    });
    // A group's box is its own as a box shape's is; a path's is its outline.
    if kind == "group" {
        s["x"] = json!(r.x0);
        s["y"] = json!(r.y0);
        s["width"] = json!(r.width());
        s["height"] = json!(r.height());
    }
    s
}

fn selrect(r: Rect) -> Value {
    json!({
        "x": r.x0, "y": r.y0, "width": r.width(), "height": r.height(),
        "x1": r.x0, "y1": r.y0, "x2": r.x1, "y2": r.y1,
    })
}

fn matrix(m: Affine) -> Value {
    let [a, b, c, d, e, f] = m.as_coeffs();
    json!({ "a": a, "b": b, "c": c, "d": d, "e": e, "f": f })
}

/// A box's corner radii in Penpot's order, scaled with the box.
fn radii(s: &mut Value, r: ondin_core::kurbo::RoundedRectRadii, f: &Frame) {
    let k = f.scale.x.min(f.scale.y);
    s["r1"] = json!(r.top_left * k);
    s["r2"] = json!(r.top_right * k);
    s["r3"] = json!(r.bottom_right * k);
    s["r4"] = json!(r.bottom_left * k);
}

fn local_box(size: Size) -> Rect {
    Rect::from_origin_size(Point::ZERO, size)
}

/// A text's box in its own space: the fixed size, or the laid-out lines.
fn text_box(res: &Resolved, id: NodeId, kind: &NodeKind) -> Rect {
    let laid = res.text_layout(id).map(|l| l.bounds());
    match kind {
        NodeKind::Text {
            sizing: TextSizing::Fixed(size),
            ..
        } => local_box(*size),
        NodeKind::Text {
            sizing: TextSizing::AutoHeight(w),
            ..
        } => Rect::new(0.0, 0.0, *w, laid.map_or(0.0, |l| l.y1.max(0.0))),
        _ => laid.unwrap_or(Rect::ZERO),
    }
}

fn ids(written: &[Written]) -> Value {
    Value::Array(written.iter().map(|w| json!(w.id)).collect())
}

fn union(mut rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    let first = rects.next()?;
    Some(rects.fold(first, |a, b| a.union(b)))
}

/// A slash-grouped name as Penpot's group path and name.
fn split_name(name: &str) -> (String, String) {
    match name.rsplit_once('/') {
        Some((path, leaf)) => (path.trim().to_owned(), leaf.trim().to_owned()),
        None => (String::new(), name.to_owned()),
    }
}

/// `#rrggbb` and the alpha, apart, as Penpot stores a colour.
fn hex_alpha(c: Color) -> (String, f32) {
    let rgba = c.to_rgba8();
    (
        format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b),
        c.components[3].clamp(0.0, 1.0),
    )
}

/// A number as Penpot writes a text attribute: a string, without a needless
/// fraction.
fn num(v: f64) -> String {
    let v = (v * 1000.0).round() / 1000.0;
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// The font id Penpot loads a family by: its bundled Source Sans Pro, else the
/// Google family of that name.
fn font_id(family: &str) -> String {
    match family {
        "Source Sans Pro" | "sourcesanspro" => "sourcesanspro".to_owned(),
        f => format!("gfont-{}", f.trim().to_lowercase().replace(' ', "-")),
    }
}

/// Penpot's variant id for a weight and slant: `regular`, `italic`, `700`,
/// `700italic`.
fn variant_id(weight: u16, italic: bool) -> String {
    match (weight, italic) {
        (400, false) => "regular".to_owned(),
        (400, true) => "italic".to_owned(),
        (w, false) => w.to_string(),
        (w, true) => format!("{w}italic"),
    }
}

/// The unit circle placed by `e` as Penpot describes an ellipse: its centre, the
/// end of one axis, and the other axis's length as a multiple of that one.
fn ellipse_axes(e: Affine) -> Option<(Point, Point, f64)> {
    let [a, b, c, d, ..] = e.as_coeffs();
    // The axes are the eigenvectors of A·Aᵀ, their lengths the square roots of
    // its eigenvalues.
    let (p, q, r) = (a * a + c * c, a * b + c * d, b * b + d * d);
    let mid = (p + r) / 2.0;
    let half = (((p - r) / 2.0).powi(2) + q * q).sqrt();
    let (big, small) = (mid + half, (mid - half).max(0.0));
    let axis = if q.abs() > 1e-12 {
        Vec2::new(big - r, q)
    } else if p >= r {
        Vec2::new(1.0, 0.0)
    } else {
        Vec2::new(0.0, 1.0)
    };
    let len = axis.hypot();
    let (major, minor) = (big.sqrt(), small.sqrt());
    if len < 1e-12 || major < 1e-12 {
        return None;
    }
    let centre = e * Point::ZERO;
    Some((centre, centre + axis / len * major, minor / major))
}

/// A UUID derived from `parts`, stable across runs: two FNV-1a passes, marked
/// as version 8.
fn uuid(parts: &[&str]) -> String {
    let hash = |seed: u64| {
        let mut h = seed;
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                h ^= 0x1f;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            for byte in part.bytes() {
                h ^= u64::from(byte);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        h
    };
    let (hi, lo) = (hash(0xcbf2_9ce4_8422_2325), hash(0x8422_2325_cbf2_9ce4));
    let hi = (hi & !0xf000) | 0x8000;
    let lo = (lo & !0xc000_0000_0000_0000) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        hi >> 32,
        (hi >> 16) & 0xffff,
        hi & 0xffff,
        lo >> 48,
        lo & 0xffff_ffff_ffff
    )
}
