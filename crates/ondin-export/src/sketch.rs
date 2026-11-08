//! Sketch export: the whole document written as a `.sketch` file, which
//! `ondin_core::sketch_in` reads back.
//!
//! The file is a zip of JSON — `document.json`, `meta.json`, `user.json` and one
//! `pages/<id>.json` per page — with pictures under `images/`, to the shape
//! `@sketch-hq/sketch-file-format` describes, whose schema closes every class.
//!
//! Sketch places a layer by its frame in its parent's space, turned and flipped
//! about the frame's centre. A path is written in its parent's space with no
//! turn; a group is unturned too, its own transform folded into its layers.
//!
//! Sketch's structure is stricter than Ondin's in two places, so the writer
//! rebuilds both: an artboard sits only at the top of a page, so a frame anywhere
//! else is a group whose first layer carries its paint (and clips, when the frame
//! does); and a symbol's master sits only at the top of a page, so every main is
//! written on a Symbols page and an instance of it stands where the main was. A
//! copy is a symbol instance carrying its text overrides; one that differs from
//! its main in anything else is written as plain layers.

use crate::stable_id::uuid as stable_uuid;
use ondin_core::kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape, Size, Vec2};
use ondin_core::peniko::{Color, Extend, GradientKind};
use ondin_core::{
    BlendMode, BoolOp, Brush, Document, EffectKind, FillRule, ImageFit, ImageFormat, ImageId,
    Length, MaskMode, Node, NodeId, NodeKind, Operation, Resolved, Stroke, StrokeAlign, TextAlign,
    TextCase, TextSizing, TextStyle, VarId, VarKind, VerticalAlign,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// A written `.sketch` file and what it draws differently.
pub struct SketchExport {
    /// The zip.
    pub bytes: Vec<u8>,
    /// Features Sketch draws differently or not at all. Deduplicated, in order met.
    pub approximated: Vec<String>,
}

/// The file format written: Sketch 99's, the oldest the current format reads.
const VERSION: u32 = 146;
const COMPATIBILITY: u32 = 99;

/// Write `doc` as a `.sketch` file named `name`.
pub fn export(doc: &Document, res: &Resolved, name: &str) -> std::io::Result<SketchExport> {
    let mut w = Writer {
        doc,
        res,
        entries: Vec::new(),
        images: BTreeMap::new(),
        approximated: Vec::new(),
        swatches: HashMap::new(),
        text_styles: HashMap::new(),
        symbols: BTreeMap::new(),
        plain: Vec::new(),
    };
    let (swatches, text_styles) = w.library();
    if !doc.guides().is_empty() {
        w.approximate("Guides, left out");
    }
    w.find_symbols();
    let mut pages = Vec::new();
    let mut directory = serde_json::Map::new();
    for page in doc.page_ids() {
        let pid = uuid(&["page", &page.to_wire()]);
        let layers = w.children(page, Affine::IDENTITY, Container::Page);
        let page_name = doc.get(page).map_or("Page", |n| n.name()).to_owned();
        pages.push((pid, page_name, layers));
    }
    if !w.symbols.is_empty() {
        let masters = w.masters();
        pages.push((uuid(&["symbols page", name]), "Symbols".to_owned(), masters));
    }
    let mut refs = Vec::new();
    for (pid, page_name, layers) in pages {
        let mut boards = serde_json::Map::new();
        for l in &layers {
            if matches!(s(l, "_class"), "artboard" | "symbolMaster") {
                boards.insert(
                    s(l, "do_objectID").to_owned(),
                    json!({ "name": s(l, "name") }),
                );
            }
        }
        directory.insert(
            pid.clone(),
            json!({ "name": page_name, "artboards": boards }),
        );
        let mut page = base(&pid, &page_name, "page", Rect::ZERO);
        page["hasClickThrough"] = json!(true);
        page["horizontalRulerData"] = json!({ "_class": "rulerData", "base": 0, "guides": [] });
        page["verticalRulerData"] = json!({ "_class": "rulerData", "base": 0, "guides": [] });
        page["style"] = style(&format!("{pid}/style"), Vec::new(), Vec::new());
        page["layers"] = Value::Array(layers);
        w.put(format!("pages/{pid}.json"), &page);
        refs.push(json!({
            "_class": "MSJSONFileReference",
            "_ref_class": "MSImmutablePage",
            "_ref": format!("pages/{pid}"),
        }));
    }
    let document = json!({
        "_class": "document",
        "do_objectID": uuid(&["document", name]),
        "assets": {
            "_class": "assetCollection",
            "do_objectID": uuid(&["assets", name]),
            "colorAssets": [], "gradientAssets": [], "images": [],
            "colors": [], "gradients": [], "exportPresets": [],
        },
        "colorSpace": 1,
        "currentPageIndex": 0,
        "foreignLayerStyles": [],
        "foreignSymbols": [],
        "foreignTextStyles": [],
        "foreignSwatches": [],
        "layerStyles": { "_class": "sharedStyleContainer", "objects": [] },
        "layerTextStyles": { "_class": "sharedTextStyleContainer", "objects": text_styles },
        "sharedSwatches": { "_class": "swatchContainer", "objects": swatches },
        "perDocumentLibraries": [],
        "pages": refs,
    });
    w.put("document.json".to_owned(), &document);
    let app = json!({
        "commit": "ondin",
        "appVersion": "99.0",
        "build": 0,
        "app": "com.bohemiancoding.sketch3",
        "compatibilityVersion": COMPATIBILITY,
        "version": VERSION,
        "variant": "NONAPPSTORE",
    });
    let mut meta = app.clone();
    meta["pagesAndArtboards"] = Value::Object(directory);
    meta["autosaved"] = json!(0);
    meta["created"] = app;
    meta["saveHistory"] = json!([format!("ondin {}", env!("CARGO_PKG_VERSION"))]);
    w.put("meta.json".to_owned(), &meta);
    w.put(
        "user.json".to_owned(),
        &json!({ "document": { "pageListHeight": 110, "pageListCollapsed": 0 } }),
    );
    let bytes = crate::plan::zip(&w.entries)?;
    Ok(SketchExport {
        bytes,
        approximated: w.approximated,
    })
}

/// A UUID as Sketch writes one: upper-case.
fn uuid(parts: &[&str]) -> String {
    stable_uuid(parts).to_ascii_uppercase()
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// What a layer is being written into, which decides what a frame becomes.
#[derive(Clone, Copy, PartialEq)]
enum Container {
    /// A page's top, where a frame can be an artboard.
    Page,
    /// A symbol master, or anything below a page's top.
    Inner,
}

struct Writer<'a> {
    doc: &'a Document,
    res: &'a Resolved,
    entries: Vec<(String, Vec<u8>)>,
    /// Picture → its entry in `images/`, `None` for one with no bytes here.
    images: BTreeMap<String, Option<String>>,
    approximated: Vec<String>,
    /// Colour variables written as swatches: their id and default colour.
    swatches: HashMap<VarId, (String, Color)>,
    /// Text styles written as shared text styles: their id and the type.
    text_styles: HashMap<ondin_core::style::StyleId, (String, TextStyle)>,
    /// Main → its symbol id.
    symbols: BTreeMap<NodeId, String>,
    /// Instance roots written as plain layers.
    plain: Vec<NodeId>,
}

impl Writer<'_> {
    fn put(&mut self, name: String, v: &Value) {
        let bytes = serde_json::to_vec(v).unwrap_or_default();
        self.entries.push((name, bytes));
    }

    /// Sketch draws this differently. Deduplicated by wording.
    fn approximate(&mut self, what: &str) {
        if !self.approximated.iter().any(|s| s == what) {
            self.approximated.push(what.to_owned());
        }
    }

    /// Colour variables as swatches, in the default mode, and text styles as
    /// shared text styles.
    fn library(&mut self) -> (Vec<Value>, Vec<Value>) {
        let doc = self.doc;
        let t = doc.variables();
        let mut swatches = Vec::new();
        for var in t.iter().filter(|v| v.kind == VarKind::Color) {
            // In every collection's default, a kit's through its mapping.
            let value = t.resolve_opt(var.id, None);
            let Some(ondin_core::Value::Color(c)) = value else {
                continue;
            };
            let id = uuid(&["swatch", &format!("{:?}", var.id)]);
            swatches.push(json!({
                "_class": "swatch", "do_objectID": id, "name": var.name, "value": color(c, None),
            }));
            self.swatches.insert(var.id, (id, c));
        }
        if !self.swatches.is_empty() && t.collections().iter().any(|c| c.modes.len() > 1) {
            self.approximate("Colour variables' other modes, left out");
        }
        if t.iter().any(|v| v.kind != VarKind::Color) {
            self.approximate("Number and text variables, written as their values");
        }
        let mut text_styles = Vec::new();
        for st in doc.styles().iter() {
            match &st.value {
                ondin_core::style::StyleValue::Text(ts) => {
                    let id = uuid(&["text style", &format!("{:?}", st.id)]);
                    let typed = ondin_core::style::type_part(ts);
                    let attrs = run_attributes(&typed, Color::BLACK, TextAlign::Start);
                    let mut value = style(&format!("{id}/value"), Vec::new(), Vec::new());
                    value["textStyle"] = json!({
                        "_class": "textStyle", "verticalAlignment": 0, "encodedAttributes": attrs,
                    });
                    text_styles.push(json!({
                        "_class": "sharedStyle", "do_objectID": id, "name": st.name, "value": value,
                    }));
                    self.text_styles.insert(st.id, (id, typed));
                }
                ondin_core::style::StyleValue::Layout(_) => {}
                _ => self.approximate("Fill, border and effect styles, written as their values"),
            }
        }
        (swatches, text_styles)
    }

    /// Every main on a page, given a symbol id.
    fn find_symbols(&mut self) {
        let doc = self.doc;
        for page in doc.page_ids() {
            for id in ondin_core::build::subtree_nodes(doc, &[page]) {
                if doc.get(id).is_some_and(Node::component) && !doc.in_vault(id) {
                    self.symbols.insert(id, uuid(&["symbol", &id.to_wire()]));
                }
            }
        }
    }

    /// The Symbols page's masters, one under the other.
    fn masters(&mut self) -> Vec<Value> {
        let doc = self.doc;
        let mut out = Vec::new();
        let mut y = 0.0;
        let symbols: Vec<(NodeId, String)> =
            self.symbols.iter().map(|(k, v)| (*k, v.clone())).collect();
        for (main, symbol) in symbols {
            let Some(node) = doc.get(main) else { continue };
            let Some((size, inner)) = self.master_space(main) else {
                continue;
            };
            let id = uuid(&["master", &main.to_wire()]);
            let mut m = base(
                &id,
                node.name(),
                "symbolMaster",
                Rect::new(0.0, y, size.width, y + size.height),
            );
            board(&mut m);
            m["includeBackgroundColorInInstance"] = json!(false);
            m["symbolID"] = json!(symbol);
            m["allowsOverrides"] = json!(true);
            m["overrideProperties"] = json!([]);
            let mut layers = Vec::new();
            if let NodeKind::Artboard { .. } = self.res.used_kind_of(node) {
                layers = self.board_paint(&mut m, main, node, inner, size);
                if !node.clip() {
                    self.approximate("Frames that do not clip, clipped as artboards");
                }
            }
            layers.extend(self.children(main, inner, Container::Inner));
            m["layers"] = Value::Array(layers);
            if node.set().is_some() || !node.variant().is_empty() {
                self.approximate("Variants, as separate symbols");
            }
            if node.display().is_some() {
                self.approximate("Auto layout, kept at drawn positions");
            }
            out.push(m);
            y += size.height + 100.0;
        }
        out
    }

    /// A main's size as a master, and the map from the page into the master.
    fn master_space(&self, main: NodeId) -> Option<(Size, Affine)> {
        let (doc, res) = (self.doc, self.res);
        let node = doc.get(main)?;
        let world = res.world_transform(main)?;
        let local = match res.used_kind_of(node) {
            NodeKind::Artboard { size, .. } => Rect::from_origin_size(Point::ZERO, *size),
            _ => ondin_core::local_box(doc, res, main)?,
        };
        let inner = Affine::translate(-local.origin().to_vec2()) * world.inverse();
        Some((local.size(), inner))
    }

    /// `parent`'s children, back to front, as Sketch layers in the container
    /// `space` maps the page into. A mask clips the layers above it, as in Ondin.
    fn children(&mut self, parent: NodeId, space: Affine, at: Container) -> Vec<Value> {
        let doc = self.doc;
        let Some(node) = doc.get(parent) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &child in node.children() {
            out.extend(self.layer(child, space, at));
        }
        out
    }

    /// One node as Sketch layers: usually one, none for a slice, and a group in
    /// a frame's place below the page's top.
    fn layer(&mut self, id: NodeId, space: Affine, at: Container) -> Option<Value> {
        let (doc, res) = (self.doc, self.res);
        let node = doc.get(id)?;
        let world = space * res.world_transform(id).unwrap_or(Affine::IDENTITY);
        let kind = res.used_kind_of(node);
        let lid = uuid(&["node", &id.to_wire()]);
        // A main stands as an instance of its symbol.
        if let Some(symbol) = self.symbols.get(&id).cloned() {
            return self.instance_of(id, node, &symbol, space, &[]);
        }
        if node.link().is_some()
            && ondin_core::component::instance_root(doc, id) == Some(id)
            && let Some(v) = self.copy(id, node, space)
        {
            return Some(v);
        }
        let mut layer = match kind {
            NodeKind::Root => return None,
            NodeKind::Slice { size } => {
                let p = place(world, *size);
                if p.rotation != 0.0 || p.flip_y || p.skewed {
                    self.approximate("Turned slices, unturned");
                }
                let mut sl = base(&lid, node.name(), "slice", p.frame);
                sl["hasBackgroundColor"] = json!(false);
                sl["backgroundColor"] = color(Color::WHITE, None);
                sl["exportOptions"] = self.exports(node);
                self.flags(&mut sl, id, node);
                return Some(sl);
            }
            NodeKind::Artboard { size, .. } => {
                if node.display().is_some() {
                    self.approximate("Auto layout, kept at drawn positions");
                }
                let p = place(world, *size);
                if at == Container::Page && p.plain() {
                    let mut a = base(&lid, node.name(), "artboard", p.frame);
                    board(&mut a);
                    let inner = Affine::translate(-p.frame.origin().to_vec2()) * space;
                    let mut layers = self.board_paint(&mut a, id, node, inner, *size);
                    if !node.clip() {
                        self.approximate("Frames that do not clip, clipped as artboards");
                    }
                    layers.extend(self.children(id, inner, Container::Inner));
                    a["layers"] = Value::Array(layers);
                    a
                } else {
                    if at == Container::Page {
                        self.approximate("Turned or scaled frames, as groups");
                    } else {
                        self.approximate("Frames inside frames or groups, as groups");
                    }
                    let extent = self.extent(id, space)?;
                    let inner = Affine::translate(-extent.origin().to_vec2()) * space;
                    let mut g = base(&lid, node.name(), "group", extent);
                    group(&mut g);
                    let (bg, _) = self.frame_paint(id, node, inner, *size, node.clip());
                    let mut layers = bg;
                    layers.extend(self.children(id, inner, Container::Inner));
                    g["layers"] = Value::Array(layers);
                    // The paint layer carries the paint; the group the rest.
                    g["style"] = self.effects_style(id, node, &lid);
                    self.flags(&mut g, id, node);
                    return Some(g);
                }
            }
            NodeKind::Group => {
                if node.display().is_some() {
                    self.approximate("Auto layout, kept at drawn positions");
                }
                let extent = self.extent(id, space)?;
                let inner = Affine::translate(-extent.origin().to_vec2()) * space;
                let mut g = base(&lid, node.name(), "group", extent);
                group(&mut g);
                let layers = self.children(id, inner, Container::Inner);
                if layers.is_empty() {
                    return None;
                }
                g["layers"] = Value::Array(layers);
                g
            }
            NodeKind::Rect {
                size,
                corner_radii,
                corner_smoothing,
            } => {
                let p = place(world, *size);
                let mut r = base(&lid, node.name(), "rectangle", p.frame);
                p.write(&mut r);
                let k = p.scale.x.min(p.scale.y);
                r["points"] = json!([
                    corner(0.0, 0.0, corner_radii.top_left * k),
                    corner(1.0, 0.0, corner_radii.top_right * k),
                    corner(1.0, 1.0, corner_radii.bottom_right * k),
                    corner(0.0, 1.0, corner_radii.bottom_left * k),
                ]);
                r["fixedRadius"] = json!(0);
                r["hasConvertedToNewRoundCorners"] = json!(true);
                r["needsConvertionToNewRoundCorners"] = json!(false);
                shape(&mut r, *corner_smoothing > 0.0);
                if *corner_smoothing > 0.0 {
                    self.approximate("Corner smoothing, at Sketch's own amount");
                }
                if p.skewed {
                    self.approximate("Skewed layers, unskewed");
                }
                r
            }
            NodeKind::Ellipse { size, arc } if arc.is_whole() => {
                let p = place(world, *size);
                let mut o = base(&lid, node.name(), "oval", p.frame);
                p.write(&mut o);
                o["points"] = oval_points();
                shape(&mut o, false);
                if p.skewed {
                    self.approximate("Skewed layers, unskewed");
                }
                o
            }
            NodeKind::Ellipse { .. }
            | NodeKind::Polygon { .. }
            | NodeKind::Star { .. }
            | NodeKind::Line { .. }
            | NodeKind::Spiral { .. }
            | NodeKind::Arc { .. }
            | NodeKind::Path { .. } => {
                let path = res.local_path(doc, id)?;
                let kind = res.used_kind_of(node);
                let (mut path, spike) = crate::svg::spiral_for_one_path(doc, res, id, kind, path);
                if spike {
                    self.approximate("Spiral strokes, drawn from the centre");
                }
                path.apply_affine(world);
                path_layer(&lid, node.name(), &path, node.fill_rule())?
            }
            NodeKind::Boolean { op } => {
                let parts: Vec<BezPath> = node
                    .children()
                    .iter()
                    .filter_map(|&c| res.operand_path(doc, c))
                    .map(|mut p| {
                        p.apply_affine(world);
                        p
                    })
                    .collect();
                let extent = parts
                    .iter()
                    .map(Shape::bounding_box)
                    .reduce(|a, b| a.union(b))?;
                let mut g = base(&lid, node.name(), "shapeGroup", extent);
                group(&mut g);
                g["windingRule"] = json!(1);
                let sketch_op = match op {
                    BoolOp::Union => 0,
                    BoolOp::Subtract => 1,
                    BoolOp::Intersect => 2,
                    BoolOp::Exclude => 3,
                };
                let to_group = Affine::translate(-extent.origin().to_vec2());
                let mut layers = Vec::new();
                for (i, mut part) in parts.into_iter().enumerate() {
                    part.apply_affine(to_group);
                    let pid = uuid(&["operand", &id.to_wire(), &i.to_string()]);
                    if let Some(mut l) = path_layer(&pid, "Operand", &part, FillRule::NonZero) {
                        l["booleanOperation"] = json!(if i == 0 { -1 } else { sketch_op });
                        layers.push(l);
                    }
                }
                g["layers"] = Value::Array(layers);
                g
            }
            NodeKind::Text { .. } => {
                let local = text_box(res, id, kind);
                let p = place(
                    world * Affine::translate(local.origin().to_vec2()),
                    local.size(),
                );
                let mut t = base(&lid, node.name(), "text", p.frame);
                p.write(&mut t);
                self.text(&mut t, id, node, kind, local);
                t
            }
        };
        let space_kind = match kind {
            NodeKind::Ellipse { arc, .. } if arc.is_whole() => Paint::Box(local_size(kind)),
            NodeKind::Rect { .. } => Paint::Box(local_size(kind)),
            NodeKind::Text { .. } => Paint::None,
            NodeKind::Group | NodeKind::Artboard { .. } => Paint::None,
            _ => Paint::Frame,
        };
        if !matches!(space_kind, Paint::None) {
            let st = self.style_of(id, node, &lid, (&space_kind, world), &layer);
            layer["style"] = st;
        } else if !matches!(kind, NodeKind::Text { .. }) {
            layer["style"] = self.effects_style(id, node, &lid);
        }
        self.flags(&mut layer, id, node);
        Some(layer)
    }

    /// Visibility, lock, mask and constraints, which every layer carries.
    fn flags(&mut self, l: &mut Value, id: NodeId, node: &Node) {
        let doc = self.doc;
        l["isVisible"] = json!(doc.shown(id));
        l["isLocked"] = json!(node.locked());
        if node.mask() && doc.shown(id) {
            l["hasClippingMask"] = json!(true);
            match node.mask_mode() {
                // Sketch draws an outline mask's paint and effects; Ondin's
                // paints nothing.
                MaskMode::Shape => {
                    l["clippingMaskMode"] = json!(0);
                    if let Some(st) = l.get_mut("style") {
                        for key in ["fills", "borders", "shadows", "innerShadows"] {
                            let paints = st.get_mut(key).and_then(Value::as_array_mut);
                            for p in paints.into_iter().flatten() {
                                p["isEnabled"] = json!(false);
                            }
                        }
                        if let Some(blur) = st.get_mut("blur") {
                            blur["isEnabled"] = json!(false);
                        }
                    }
                }
                MaskMode::Alpha => l["clippingMaskMode"] = json!(1),
                MaskMode::Luminance => {
                    l["clippingMaskMode"] = json!(1);
                    self.approximate("Luminance masks, as alpha masks");
                }
            }
        }
        l["resizingConstraint"] = json!(constraint(node.insets()));
        l["exportOptions"] = self.exports(node);
        if !node.grids().is_empty() {
            self.approximate("Layout grids, left out");
        }
    }

    /// A layer's export settings as Sketch's export presets. Sketch names a file
    /// by a suffix or a prefix, not both; the suffix is the one Ondin names the
    /// file with, so an untyped 2× row writes `@2x`.
    fn exports(&mut self, node: &Node) -> Value {
        use ondin_core::{ExportFormat, ExportScale};
        let mut formats = Vec::new();
        for spec in node.exports() {
            let (scale, size, kind) = match spec.scale {
                ExportScale::Times(s) => (f64::from(s), 0, 0),
                ExportScale::Width(w) => (1.0, w, 1),
                ExportScale::Height(h) => (1.0, h, 2),
            };
            let (name, scheme) = match (spec.prefix.is_empty(), spec.suffix.is_empty()) {
                (true, _) => (spec.effective_suffix(), 0),
                (false, true) => (spec.prefix.clone(), 1),
                (false, false) => {
                    self.approximate(
                        "Export names with a prefix and a suffix, named by the suffix",
                    );
                    (spec.suffix.clone(), 0)
                }
            };
            if spec.background != Default::default() {
                self.approximate("Export backgrounds, left out");
            }
            if crate::plan::acting_extras(spec) {
                self.approximate(
                    "Export quality, trimming, square padding and outlined text, left out",
                );
            }
            formats.push(json!({
                "_class": "exportFormat",
                "absoluteSize": size,
                "fileFormat": match spec.format {
                    ExportFormat::Png => "png",
                    ExportFormat::Jpeg => "jpg",
                    ExportFormat::Webp => "webp",
                    ExportFormat::Svg => "svg",
                    ExportFormat::Pdf => "pdf",
                },
                "name": name,
                "namingScheme": scheme,
                "scale": scale,
                "visibleScaleType": kind,
            }));
        }
        json!({
            "_class": "exportOptions",
            "exportFormats": formats,
            "includedLayerIds": [],
            "layerOptions": 0,
            "shouldTrim": false,
        })
    }

    /// The box `id` draws in, in the container `space` maps the page into.
    fn extent(&self, id: NodeId, space: Affine) -> Option<Rect> {
        let (doc, res) = (self.doc, self.res);
        let local = ondin_core::local_box(doc, res, id)?;
        let world = space * res.world_transform(id)?;
        Some(world.transform_rect_bbox(local))
    }

    /// A frame's paint as a layer of its own — the rect that, below a page's
    /// top, also clips the frame when `clip` — and whether it paints anything.
    fn frame_paint(
        &mut self,
        id: NodeId,
        node: &Node,
        inner: Affine,
        size: Size,
        clip: bool,
    ) -> (Vec<Value>, bool) {
        let NodeKind::Artboard {
            corner_radii,
            corner_smoothing,
            ..
        } = self.res.used_kind_of(node)
        else {
            return (Vec::new(), false);
        };
        let paints = ondin_core::paint_box::node_paint(self.doc, self.res, id).is_some_and(|p| {
            p.fills.iter().any(|f| f.visible) || p.strokes.iter().any(|s| s.visible)
        });
        if !paints && !clip {
            return (Vec::new(), false);
        }
        let world = inner * self.res.world_transform(id).unwrap_or(Affine::IDENTITY);
        let p = place(world, size);
        let lid = uuid(&["paint", &id.to_wire()]);
        let mut r = base(&lid, node.name(), "rectangle", p.frame);
        p.write(&mut r);
        let k = p.scale.x.min(p.scale.y);
        r["points"] = json!([
            corner(0.0, 0.0, corner_radii.top_left * k),
            corner(1.0, 0.0, corner_radii.top_right * k),
            corner(1.0, 1.0, corner_radii.bottom_right * k),
            corner(0.0, 1.0, corner_radii.bottom_left * k),
        ]);
        r["fixedRadius"] = json!(0);
        r["hasConvertedToNewRoundCorners"] = json!(true);
        r["needsConvertionToNewRoundCorners"] = json!(false);
        shape(&mut r, *corner_smoothing > 0.0);
        if *corner_smoothing > 0.0 {
            self.approximate("Corner smoothing, at Sketch's own amount");
        }
        let st = self.paint_style(id, node, &lid, (&Paint::Box(size), world), &r);
        r["style"] = st;
        // An outline mask that also paints: Sketch draws its fill under what it
        // clips, which is the frame's own fill under its content.
        if clip {
            r["hasClippingMask"] = json!(true);
            r["clippingMaskMode"] = json!(0);
        }
        (vec![r], paints)
    }

    /// A frame's paint where an artboard or a master holds it: one solid fill as
    /// the board's background colour, anything more as a layer at its bottom.
    fn board_paint(
        &mut self,
        board: &mut Value,
        id: NodeId,
        node: &Node,
        inner: Affine,
        size: Size,
    ) -> Vec<Value> {
        if let Some(c) = self.solid_background(id, node) {
            board["hasBackgroundColor"] = json!(true);
            board["backgroundColor"] = color(c, None);
            if s(board, "_class") == "symbolMaster" {
                board["includeBackgroundColorInInstance"] = json!(true);
            }
            return Vec::new();
        }
        self.frame_paint(id, node, inner, size, false).0
    }

    /// A frame's one visible solid fill, when that is its whole paint.
    fn solid_background(&self, id: NodeId, node: &Node) -> Option<Color> {
        let paint = ondin_core::paint_box::node_paint(self.doc, self.res, id)?;
        let fills: Vec<_> = paint.fills.iter().filter(|f| f.visible).collect();
        let radii = matches!(self.res.used_kind_of(node),
            NodeKind::Artboard { corner_radii, .. } if *corner_radii != Default::default());
        match fills.as_slice() {
            [f] if paint.strokes.iter().all(|s| !s.visible)
                && !radii
                && node.effects().is_empty() =>
            {
                match f.brush.drawn() {
                    Brush::Solid(c) => Some(*c),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A copy as a symbol instance, or `None` to write it as plain layers.
    fn copy(&mut self, root: NodeId, node: &Node, space: Affine) -> Option<Value> {
        let doc = self.doc;
        let main = ondin_core::component::main_of(doc, root)?;
        let symbol = self.symbols.get(&main).cloned();
        let overrides = symbol
            .as_ref()
            .and_then(|_| self.text_overrides(root, main));
        match (symbol, overrides) {
            (Some(symbol), Some(o)) => self.instance_of(root, node, &symbol, space, &o),
            _ => {
                if !self.plain.contains(&root) {
                    self.plain.push(root);
                    self.approximate(
                        "Instances that differ from their main past text, as plain layers",
                    );
                }
                None
            }
        }
    }

    /// The text overrides of the copy at `root` as Sketch names them, or `None`
    /// when it differs from its main in something Sketch cannot override.
    fn text_overrides(&self, root: NodeId, main: NodeId) -> Option<Vec<Value>> {
        let doc = self.doc;
        let mut out = Vec::new();
        for id in ondin_core::build::subtree_nodes(doc, &[root]) {
            let n = doc.get(id)?;
            if n.link().is_none() || n.swap().is_some() {
                return None;
            }
            for o in ondin_core::reset::overrides(doc, id) {
                match &o.reset {
                    // The root's own place, size, name and switches are the
                    // instance's.
                    Operation::SetTransform { .. }
                    | Operation::SetGeometry { .. }
                    | Operation::SetName { .. }
                    | Operation::SetVisible { .. }
                    | Operation::SetInsets { .. }
                    | Operation::SetLayoutItem { .. }
                        if id == root => {}
                    // A string override carries no runs, so the copy's must be
                    // its source's.
                    Operation::SetText { .. } => {
                        let NodeKind::Text {
                            content,
                            spans,
                            para_spans,
                            ..
                        } = n.kind()
                        else {
                            return None;
                        };
                        let source = n.link()?;
                        let NodeKind::Text {
                            spans: s_spans,
                            para_spans: s_paras,
                            ..
                        } = doc.get(source)?.kind()
                        else {
                            return None;
                        };
                        if spans != s_spans || para_spans != s_paras {
                            return None;
                        }
                        let path = self.override_path(source, main)?;
                        out.push(json!({
                            "_class": "overrideValue",
                            "overrideName": format!("{path}_stringValue"),
                            "value": content,
                        }));
                    }
                    _ => return None,
                }
            }
        }
        Some(out)
    }

    /// The override path of `source`, a layer inside `main`'s tree: the
    /// outermost nested copy holding it that `main`'s master writes as an
    /// instance, then its path inside that copy's main; with none, its own id,
    /// since a copy written plain holds its layers under their own ids.
    fn override_path(&self, source: NodeId, main: NodeId) -> Option<String> {
        use ondin_core::component::{instance_root, main_of};
        let doc = self.doc;
        let mut above = Vec::new();
        let mut at = Some(source);
        while let Some(id) = at.filter(|&a| a != main) {
            above.push(id);
            at = doc.get(id)?.parent();
        }
        let nested = above.into_iter().rev().find(|&r| {
            instance_root(doc, r) == Some(r)
                && main_of(doc, r).is_some_and(|m| {
                    self.symbols.contains_key(&m) && self.text_overrides(r, m).is_some()
                })
        });
        let Some(r) = nested else {
            return Some(uuid(&["node", &source.to_wire()]));
        };
        let inner_main = main_of(doc, r)?;
        // `source`'s counterpart in that main: its links run one level per hop.
        let within = |mut x: Option<NodeId>| {
            while let Some(id) = x {
                if id == inner_main {
                    return true;
                }
                x = doc.get(id).and_then(Node::parent);
            }
            false
        };
        let mut link = doc.get(source)?.link()?;
        while !within(Some(link)) {
            link = doc.get(link)?.link()?;
        }
        let rest = self.override_path(link, inner_main)?;
        Some(format!("{}/{rest}", self.instance_id(r)))
    }

    /// The id a node written as an instance carries.
    fn instance_id(&self, id: NodeId) -> String {
        match self.symbols.contains_key(&id) {
            true => uuid(&["instance of main", &id.to_wire()]),
            false => uuid(&["node", &id.to_wire()]),
        }
    }

    /// A symbol instance of `symbol` where `id` stands.
    fn instance_of(
        &mut self,
        id: NodeId,
        node: &Node,
        symbol: &str,
        space: Affine,
        overrides: &[Value],
    ) -> Option<Value> {
        let (doc, res) = (self.doc, self.res);
        let local = match res.used_kind_of(node) {
            NodeKind::Artboard { size, .. } => Rect::from_origin_size(Point::ZERO, *size),
            _ => ondin_core::local_box(doc, res, id)?,
        };
        let world = space * res.world_transform(id)? * Affine::translate(local.origin().to_vec2());
        let p = place(world, local.size());
        let lid = self.instance_id(id);
        let mut i = base(&lid, node.name(), "symbolInstance", p.frame);
        p.write(&mut i);
        i["symbolID"] = json!(symbol);
        i["overrideValues"] = Value::Array(overrides.to_vec());
        i["scale"] = json!(1);
        i["verticalSpacing"] = json!(0);
        i["horizontalSpacing"] = json!(0);
        i["style"] = self.effects_style(id, node, &lid);
        self.flags(&mut i, id, node);
        Some(i)
    }

    /// A layer's style: fills, borders, shadows, blur, opacity and blend.
    /// `world` places the node in the layer's container.
    fn style_of(
        &mut self,
        id: NodeId,
        node: &Node,
        lid: &str,
        place: (&Paint, Affine),
        layer: &Value,
    ) -> Value {
        let mut st = self.effects_style(id, node, lid);
        self.paint_into(&mut st, id, node, place, layer);
        st
    }

    /// A frame's paint layer's style: its fills and borders only, since the
    /// group, artboard or instance it leads carries the frame's effects, opacity
    /// and blend, and Sketch would apply them twice.
    fn paint_style(
        &mut self,
        id: NodeId,
        node: &Node,
        lid: &str,
        place: (&Paint, Affine),
        layer: &Value,
    ) -> Value {
        let mut st = style(&format!("{lid}/style"), Vec::new(), Vec::new());
        self.paint_into(&mut st, id, node, place, layer);
        st
    }

    /// A layer's fills, borders and winding rule, written into `st`.
    fn paint_into(
        &mut self,
        st: &mut Value,
        id: NodeId,
        node: &Node,
        (space, world): (&Paint, Affine),
        layer: &Value,
    ) {
        let frame = frame_rect(layer);
        let to_frac = space.to_fraction(frame, world);
        let Some(paint) = ondin_core::paint_box::node_paint(self.doc, self.res, id) else {
            return;
        };
        let own = node.paint();
        let mut fills = Vec::new();
        for (i, f) in paint.fills.iter().enumerate() {
            let bound = own.fills.get(i).and_then(|o| o.brush.var());
            if let Some(mut v) = self.paint(f.brush.drawn(), bound, to_frac, "fill") {
                v["isEnabled"] = json!(f.visible);
                fills.push(v);
            }
        }
        let mut borders = Vec::new();
        for (i, st) in paint.strokes.iter().enumerate() {
            let bound = own.strokes.get(i).and_then(|o| o.brush.var());
            if let Some(mut v) = self.paint(st.brush.drawn(), bound, to_frac, "border") {
                v["isEnabled"] = json!(st.visible);
                v["thickness"] = json!(st.width);
                v["position"] = json!(match st.align {
                    StrokeAlign::Center => 0,
                    StrokeAlign::Inside => 1,
                    StrokeAlign::Outside => 2,
                });
                borders.push(v);
            }
        }
        st["fills"] = Value::Array(fills);
        st["borders"] = Value::Array(borders);
        if let Some(first) = paint.strokes.first() {
            st["borderOptions"] = self.border_options(first, &paint.strokes);
            st["miterLimit"] = json!(first.miter_limit.round().max(1.0) as i64);
        }
        st["windingRule"] = json!(match node.fill_rule() {
            FillRule::EvenOdd => 1,
            FillRule::NonZero => 0,
        });
    }

    /// The options every border of a layer shares in Sketch, from its first.
    fn border_options(&mut self, first: &Stroke, all: &[ondin_core::Keyed<Stroke>]) -> Value {
        if all
            .iter()
            .any(|s| s.dashes != first.dashes || s.cap != first.cap || s.join != first.join)
        {
            self.approximate("Borders with different dashes, caps or joins, drawn as the first");
        }
        if first.dash_offset != 0.0 && !first.dashes.is_empty() {
            self.approximate("Dash offsets");
        }
        if !first.sides.is_all() {
            self.approximate("Borders on some sides only, drawn on all four");
        }
        json!({
            "_class": "borderOptions",
            "isEnabled": true,
            "dashPattern": first.dashes,
            "lineCapStyle": match first.cap {
                ondin_core::kurbo::Cap::Butt => 0,
                ondin_core::kurbo::Cap::Round => 1,
                ondin_core::kurbo::Cap::Square => 2,
            },
            "lineJoinStyle": match first.join {
                ondin_core::kurbo::Join::Miter => 0,
                ondin_core::kurbo::Join::Round => 1,
                ondin_core::kurbo::Join::Bevel => 2,
            },
        })
    }

    /// A layer's style with no paint: effects, opacity and blend.
    fn effects_style(&mut self, id: NodeId, node: &Node, lid: &str) -> Value {
        let mut st = style(&format!("{lid}/style"), Vec::new(), Vec::new());
        let blend = match node.blend() {
            BlendMode::Normal | BlendMode::PassThrough => 0,
            b => blend_code(b),
        };
        st["contextSettings"] = json!({
            "_class": "graphicsContextSettings", "blendMode": blend, "opacity": node.opacity(),
        });
        let effects = self.doc.resolved_effects(id, node.effects());
        let (mut shadows, mut inner) = (Vec::new(), Vec::new());
        let mut blur = false;
        for e in effects.iter() {
            match &e.kind {
                EffectKind::DropShadow(sh) | EffectKind::InnerShadow(sh) => {
                    if sh.blend != BlendMode::Normal {
                        self.approximate("Shadow blend modes");
                    }
                    let class = match e.kind {
                        EffectKind::InnerShadow(_) => "innerShadow",
                        _ => "shadow",
                    };
                    let v = json!({
                        "_class": class,
                        "isEnabled": e.visible,
                        "blurRadius": sh.blur,
                        "color": color(sh.color, None),
                        "contextSettings": context(1.0),
                        "offsetX": sh.offset.x,
                        "offsetY": sh.offset.y,
                        "spread": sh.spread,
                    });
                    match class {
                        "shadow" => shadows.push(v),
                        _ => inner.push(v),
                    }
                }
                EffectKind::LayerBlur { radius } | EffectKind::BackgroundBlur { radius }
                    if !blur =>
                {
                    blur = true;
                    st["blur"] = json!({
                        "_class": "blur",
                        "isEnabled": e.visible,
                        "center": "{0.5, 0.5}",
                        "motionAngle": 0,
                        "radius": radius,
                        "saturation": 1,
                        "type": if matches!(e.kind, EffectKind::BackgroundBlur { .. }) { 3 } else { 0 },
                    });
                }
                EffectKind::LayerBlur { .. } | EffectKind::BackgroundBlur { .. } => {
                    self.approximate("A second blur, left out");
                }
                EffectKind::Filters(_) => self.approximate("Colour filters, left out"),
            }
        }
        st["shadows"] = Value::Array(shadows);
        st["innerShadows"] = Value::Array(inner);
        st
    }

    /// A brush as a Sketch fill or border, `key` naming which for the report.
    fn paint(
        &mut self,
        brush: &Brush,
        bound: Option<VarId>,
        to_frac: Option<Affine>,
        key: &str,
    ) -> Option<Value> {
        let mut v = json!({
            "_class": if key == "fill" { "fill" } else { "border" },
            "isEnabled": true,
            "color": color(Color::BLACK, None),
            "fillType": 0,
            "contextSettings": context(1.0),
            "gradient": default_gradient(),
        });
        if key == "fill" {
            v["noiseIndex"] = json!(0);
            v["noiseIntensity"] = json!(0);
            v["patternFillType"] = json!(1);
            v["patternTileScale"] = json!(1);
        }
        match brush {
            Brush::Solid(c) => {
                let swatch = bound
                    .and_then(|var| self.swatches.get(&var))
                    .filter(|(_, lib)| lib.to_rgba8() == c.to_rgba8())
                    .map(|(id, _)| id.clone());
                v["color"] = color(*c, swatch.as_deref());
            }
            Brush::Gradient(g) => {
                let to_frac = to_frac?;
                v["fillType"] = json!(1);
                v["gradient"] = self.gradient(g, to_frac)?;
                v["contextSettings"] = context(g.opacity);
            }
            Brush::Image(img) => {
                if key != "fill" {
                    self.approximate("Picture borders, left out");
                    return None;
                }
                let reference = self.picture(&img.image.id)?;
                v["fillType"] = json!(4);
                v["image"] = json!({
                    "_class": "MSJSONFileReference",
                    "_ref_class": "MSImageData",
                    "_ref": reference,
                });
                v["patternFillType"] = json!(match img.image.fit {
                    ImageFit::Fill => 1,
                    ImageFit::Fit => 3,
                    ImageFit::Tile => 0,
                    ImageFit::Crop => {
                        if img.image.crop != ondin_core::whole_crop() {
                            self.approximate("Cropped pictures, shown whole");
                        }
                        2
                    }
                });
                v["patternTileScale"] = json!(img.image.tile_scale);
                v["contextSettings"] = context(img.sampler.alpha);
                if img.image.adjust.pipeline().is_some() {
                    self.approximate("Picture adjustments, left out");
                }
                if !img.image.orient.is_default() {
                    self.approximate("Picture flips and turns, left out");
                }
            }
        }
        Some(v)
    }

    /// A gradient as Sketch's, its points fractions of the layer's frame.
    fn gradient(&mut self, g: &ondin_core::GradientBrush, to_frac: Affine) -> Option<Value> {
        if g.gradient.extend != Extend::Pad {
            self.approximate("Repeating and reflected gradients, drawn padded");
        }
        let stops: Vec<Value> = g
            .gradient
            .stops
            .iter()
            .map(|st| {
                json!({
                    "_class": "gradientStop",
                    "color": color(st.color.to_alpha_color::<ondin_core::peniko::color::Srgb>(), None),
                    "position": st.offset.clamp(0.0, 1.0),
                })
            })
            .collect();
        if stops.is_empty() {
            return None;
        }
        let f = to_frac * g.transform;
        let (kind, from, to, ratio) = match g.gradient.kind {
            GradientKind::Linear(l) => (0, f * l.start, f * l.end, 0.0),
            GradientKind::Radial(r) => {
                if r.start_radius > 0.0 || r.start_center != r.end_center {
                    self.approximate("Two-point radial gradients, drawn from one centre");
                }
                let e = f
                    * Affine::translate(r.end_center.to_vec2())
                    * Affine::scale(f64::from(r.end_radius));
                let (centre, end, ratio) = crate::penpot::ellipse_axes(e)?;
                (1, centre, end, ratio)
            }
            GradientKind::Sweep(sw) => {
                let c = f * sw.center;
                if (c - Point::new(0.5, 0.5)).hypot() > 1e-6 || sw.start_angle != 0.0 {
                    self.approximate("Angular gradients off the centre or turned, drawn from it");
                }
                (2, Point::new(0.5, 0.0), Point::new(0.5, 1.0), 0.0)
            }
        };
        Some(json!({
            "_class": "gradient",
            "gradientType": kind,
            "elipseLength": ratio,
            "from": point(from),
            "to": point(to),
            "stops": stops,
        }))
    }

    /// A picture's entry in `images/`, written once; a linked one is left out.
    fn picture(&mut self, image: &ImageId) -> Option<String> {
        if let Some(known) = self.images.get(&image.0) {
            return known.clone();
        }
        let made = match self
            .doc
            .image(image)
            .and_then(|e| e.bytes().map(|b| (e, b)))
        {
            None => {
                self.approximate("Linked or missing pictures, left out");
                None
            }
            Some((entry, bytes)) => {
                let ext = match entry.format {
                    ImageFormat::Png => "png",
                    ImageFormat::Jpeg => "jpg",
                    ImageFormat::Webp => "webp",
                    ImageFormat::Gif => "gif",
                };
                let name = format!("images/{}.{ext}", uuid(&["image", &image.0]).to_lowercase());
                self.entries.push((name.clone(), bytes.to_vec()));
                Some(name)
            }
        };
        self.images.insert(image.0.clone(), made.clone());
        made
    }

    /// A text layer's string, its runs' attributes, sizing and style.
    fn text(&mut self, t: &mut Value, id: NodeId, node: &Node, kind: &NodeKind, local: Rect) {
        let NodeKind::Text {
            content,
            style: default,
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
        let paint = ondin_core::paint_box::node_paint(self.doc, self.res, id);
        let ink = paint
            .as_ref()
            .and_then(|p| {
                p.fills
                    .iter()
                    .rev()
                    .find(|f| f.visible)
                    .map(|f| match f.brush.drawn() {
                        Brush::Solid(c) => *c,
                        Brush::Gradient(g) => g.gradient.stops.first().map_or(Color::BLACK, |s| {
                            s.color.to_alpha_color::<ondin_core::peniko::color::Srgb>()
                        }),
                        Brush::Image(_) => Color::BLACK,
                    })
            })
            .unwrap_or(Color::BLACK);
        if paint.as_ref().is_some_and(|p| {
            p.fills.iter().filter(|f| f.visible).count() > 1
                || p.fills
                    .iter()
                    .any(|f| f.visible && !matches!(f.brush.drawn(), Brush::Solid(_)))
        }) {
            self.approximate("Text filled with more than one colour, a gradient or a picture, drawn in one colour");
        }
        if paint
            .as_ref()
            .is_some_and(|p| p.strokes.iter().any(|s| s.visible))
        {
            self.approximate("Outlined text, drawn without its outline");
        }
        let runs = spans.runs(default, content.len());
        let paras = para_spans.runs(paragraph, content.len());
        let mut attributes = Vec::new();
        for (range, run) in &runs {
            let align = paras
                .iter()
                .find(|(r, _)| r.start <= range.start && range.start < r.end.max(r.start + 1))
                .map_or(paragraph.align, |(_, p)| p.align);
            if !run.variations.is_empty() || !run.features.is_empty() {
                self.approximate("OpenType features and variable axes, left out");
            }
            if !run.word_spacing.is_zero() {
                self.approximate("Word spacing, left out");
            }
            if !run.baseline_shift.is_zero() {
                self.approximate("Baseline shift, left out");
            }
            if run.case == TextCase::Title {
                self.approximate("Title case, written as typed");
            }
            let location = utf16_len(&content[..range.start]);
            let length = utf16_len(&content[range.clone()]);
            let mut attrs = run_attributes(run, run.color.unwrap_or(ink), align);
            attrs["textStyleVerticalAlignmentKey"] = json!(vertical(block.vertical_align));
            attributes.push(json!({
                "_class": "stringAttribute",
                "location": location,
                "length": length,
                "attributes": attrs,
            }));
        }
        t["attributedString"] = json!({
            "_class": "attributedString",
            "string": content,
            "attributes": attributes,
        });
        t["automaticallyDrawOnUnderlyingPath"] = json!(false);
        t["dontSynchroniseWithSymbol"] = json!(false);
        t["lineSpacingBehaviour"] = json!(2);
        t["textBehaviour"] = json!(match sizing {
            TextSizing::Auto => 0,
            TextSizing::AutoHeight(_) => 1,
            TextSizing::Fixed(_) => 2,
        });
        t["glyphBounds"] = json!(format!(
            "{{{}, {}}}",
            point(Point::ZERO),
            point(Point::new(local.width(), local.height()))
        ));
        let first = runs.first().map_or(default.as_ref(), |(_, r)| r);
        let mut st = self.effects_style(id, node, &uuid(&["node", &id.to_wire()]));
        let mut encoded = run_attributes(first, first.color.unwrap_or(ink), paragraph.align);
        encoded["textStyleVerticalAlignmentKey"] = json!(vertical(block.vertical_align));
        st["textStyle"] = json!({
            "_class": "textStyle",
            "verticalAlignment": vertical(block.vertical_align),
            "encodedAttributes": encoded,
        });
        t["style"] = st;
        if let Some((sid, typed)) = node.styles().text.and_then(|s| self.text_styles.get(&s))
            && *typed == ondin_core::style::type_part(first)
        {
            t["sharedStyleID"] = json!(sid);
        }
    }
}

/// The space a layer's gradients are measured in.
enum Paint {
    /// A box shape: its own local box of this size.
    Box(Size),
    /// A layer written in its container's space: its frame there.
    Frame,
    None,
}

impl Paint {
    /// The paint's own space → fractions of the frame Sketch measures gradients
    /// in. `to_frame` maps a layer written in its container's space from its
    /// local space into that container.
    fn to_fraction(&self, frame: Rect, to_frame: Affine) -> Option<Affine> {
        let onto = |w: f64, h: f64| {
            (w > 1e-9 && h > 1e-9).then(|| Affine::scale_non_uniform(1.0 / w, 1.0 / h))
        };
        match self {
            Paint::Box(size) => onto(size.width, size.height),
            Paint::Frame => onto(frame.width(), frame.height())
                .map(|f| f * Affine::translate(-frame.origin().to_vec2()) * to_frame),
            Paint::None => None,
        }
    }
}

/// Where a box of `size` placed by `m` sits in Sketch's terms.
struct Placed {
    frame: Rect,
    rotation: f64,
    flip_y: bool,
    scale: Vec2,
    skewed: bool,
}

impl Placed {
    /// Whether the box is placed without a turn, mirror, skew or scale: what an
    /// artboard can be.
    fn plain(&self) -> bool {
        self.rotation.abs() < 1e-9
            && !self.flip_y
            && !self.skewed
            && (self.scale.x - 1.0).abs() < 1e-9
            && (self.scale.y - 1.0).abs() < 1e-9
    }

    fn write(&self, l: &mut Value) {
        l["rotation"] = json!(self.rotation);
        l["isFlippedVertical"] = json!(self.flip_y);
    }
}

/// `m`'s linear part split into a turn, a mirror and a scale folded into the
/// frame. Sketch's rotation runs counter-clockwise on screen.
fn place(m: Affine, size: Size) -> Placed {
    let [a, b, c, d, _, _] = m.as_coeffs();
    let sx = Vec2::new(a, b).hypot();
    let (cos, sin) = if sx > 1e-12 {
        (a / sx, b / sx)
    } else {
        (1.0, 0.0)
    };
    let sy = -sin * c + cos * d;
    let shear = cos * c + sin * d;
    let sx = if sx > 1e-12 { sx } else { 1.0 };
    let flip_y = sy < 0.0;
    let sy = if sy.abs() > 1e-12 { sy.abs() } else { 1.0 };
    let (w, h) = (size.width * sx, size.height * sy);
    let centre = m * Point::new(size.width / 2.0, size.height / 2.0);
    let angle = sin.atan2(cos).to_degrees();
    Placed {
        frame: Rect::from_center_size(centre, (w, h)),
        rotation: if angle.abs() < 1e-9 { 0.0 } else { -angle },
        flip_y,
        scale: Vec2::new(sx, sy),
        skewed: shear.abs() > 1e-6 * sy.max(1.0),
    }
}

/// The fields every Sketch layer carries.
fn base(id: &str, name: &str, class: &str, frame: Rect) -> Value {
    json!({
        "_class": class,
        "do_objectID": id,
        "booleanOperation": -1,
        "exportOptions": {
            "_class": "exportOptions",
            "exportFormats": [],
            "includedLayerIds": [],
            "layerOptions": 0,
            "shouldTrim": false,
        },
        "frame": {
            "_class": "rect",
            "constrainProportions": false,
            "height": frame.height(),
            "width": frame.width(),
            "x": frame.x0,
            "y": frame.y0,
        },
        "isFixedToViewport": false,
        "isFlippedHorizontal": false,
        "isFlippedVertical": false,
        "isLocked": false,
        "isTemplate": false,
        "isVisible": true,
        "layerListExpandedType": 0,
        "name": name,
        "nameIsFixed": false,
        "resizingConstraint": 63,
        "resizingType": 0,
        "rotation": 0,
        "shouldBreakMaskChain": false,
    })
}

/// An artboard's or a symbol master's own fields.
fn board(l: &mut Value) {
    l["hasClickThrough"] = json!(false);
    l["horizontalRulerData"] = json!({ "_class": "rulerData", "base": 0, "guides": [] });
    l["verticalRulerData"] = json!({ "_class": "rulerData", "base": 0, "guides": [] });
    l["backgroundColor"] = color(Color::WHITE, None);
    l["hasBackgroundColor"] = json!(false);
    l["includeBackgroundColorInExport"] = json!(true);
    l["isFlowHome"] = json!(false);
    l["resizesContent"] = json!(false);
}

fn group(l: &mut Value) {
    l["hasClickThrough"] = json!(false);
}

/// A shape layer's own fields; `smooth` asks for Sketch's smooth corners.
fn shape(l: &mut Value, smooth: bool) {
    l["edited"] = json!(false);
    l["isClosed"] = json!(true);
    l["pointRadiusBehaviour"] = json!(if smooth { 2 } else { 1 });
}

/// The frame a layer was written with.
fn frame_rect(l: &Value) -> Rect {
    let f = &l["frame"];
    let n = |k: &str| f[k].as_f64().unwrap_or(0.0);
    Rect::from_origin_size((n("x"), n("y")), (n("width"), n("height")))
}

fn local_size(kind: &NodeKind) -> Size {
    kind.box_size().unwrap_or(Size::ZERO)
}

/// A style with nothing in it but what the schema asks for, its id derived
/// from `seed`.
fn style(seed: &str, fills: Vec<Value>, borders: Vec<Value>) -> Value {
    json!({
        "_class": "style",
        "do_objectID": uuid(&["style", seed]),
        "borders": borders,
        "borderOptions": {
            "_class": "borderOptions",
            "isEnabled": true,
            "dashPattern": [],
            "lineCapStyle": 0,
            "lineJoinStyle": 0,
        },
        "fills": fills,
        "startMarkerType": 0,
        "endMarkerType": 0,
        "miterLimit": 10,
        "windingRule": 0,
        "shadows": [],
        "innerShadows": [],
        "contextSettings": context(1.0),
        "colorControls": {
            "_class": "colorControls",
            "isEnabled": false,
            "brightness": 0,
            "contrast": 1,
            "hue": 0,
            "saturation": 1,
        },
    })
}

fn context(opacity: f32) -> Value {
    json!({ "_class": "graphicsContextSettings", "blendMode": 0, "opacity": opacity.clamp(0.0, 1.0) })
}

fn color(c: Color, swatch: Option<&str>) -> Value {
    let [r, g, b, a] = c.components;
    let mut v = json!({
        "_class": "color",
        "alpha": a.clamp(0.0, 1.0),
        "red": r.clamp(0.0, 1.0),
        "green": g.clamp(0.0, 1.0),
        "blue": b.clamp(0.0, 1.0),
    });
    if let Some(id) = swatch {
        v["swatchID"] = json!(id);
    }
    v
}

fn default_gradient() -> Value {
    json!({
        "_class": "gradient",
        "gradientType": 0,
        "elipseLength": 0,
        "from": "{0.5, 0}",
        "to": "{0.5, 1}",
        "stops": [
            { "_class": "gradientStop", "color": color(Color::WHITE, None), "position": 0 },
            { "_class": "gradientStop", "color": color(Color::BLACK, None), "position": 1 },
        ],
    })
}

fn blend_code(b: BlendMode) -> i64 {
    match b {
        BlendMode::Darken => 1,
        BlendMode::Multiply => 2,
        BlendMode::ColorBurn => 3,
        BlendMode::Lighten => 4,
        BlendMode::Screen => 5,
        BlendMode::ColorDodge => 6,
        BlendMode::Overlay => 7,
        BlendMode::SoftLight => 8,
        BlendMode::HardLight => 9,
        BlendMode::Difference => 10,
        BlendMode::Exclusion => 11,
        BlendMode::Hue => 12,
        BlendMode::Saturation => 13,
        BlendMode::Color => 14,
        BlendMode::Luminosity => 15,
        BlendMode::Normal | BlendMode::PassThrough => 0,
    }
}

/// A layer's insets as Sketch's resizing constraint: a bit cleared for each
/// thing pinned — right 1, width 2, left 4, bottom 8, height 16, top 32. A layer
/// with none keeps its place from the top left, at its own size.
fn constraint(insets: &ondin_core::Insets) -> i64 {
    use ondin_core::LengthPct;
    let axis = |a: Option<LengthPct>, b: Option<LengthPct>, centred: bool, bits: [i64; 3]| {
        let [start, size, end] = bits;
        match (a, b) {
            (Some(LengthPct::Percent(_)), _) | (_, Some(LengthPct::Percent(_))) => 0,
            (Some(_), Some(_)) if centred => size,
            (Some(_), Some(_)) => start | end,
            (None, Some(_)) => end | size,
            _ => start | size,
        }
    };
    let m = insets.margin_auto;
    let pinned = axis(insets.left, insets.right, m.left && m.right, [4, 2, 1])
        | axis(insets.top, insets.bottom, m.top && m.bottom, [32, 16, 8]);
    63 & !pinned
}

fn vertical(v: VerticalAlign) -> i64 {
    match v {
        VerticalAlign::Middle => 1,
        VerticalAlign::Bottom => 2,
        _ => 0,
    }
}

/// A run's type as Sketch's string attributes.
fn run_attributes(t: &TextStyle, ink: Color, align: TextAlign) -> Value {
    let mut paragraph = json!({
        "_class": "paragraphStyle",
        "alignment": match align {
            TextAlign::End | TextAlign::Right => 1,
            TextAlign::Center => 2,
            TextAlign::Justify => 3,
            TextAlign::Start | TextAlign::Left => 0,
        },
    });
    let line = match t.line_height {
        Some(Length::Em(m)) => Some(m * t.font_size),
        Some(Length::Px(px)) => Some(px),
        _ => None,
    };
    if let Some(px) = line {
        paragraph["maximumLineHeight"] = json!(px);
        paragraph["minimumLineHeight"] = json!(px);
    }
    let mut v = json!({
        "MSAttributedStringFontAttribute": {
            "_class": "fontDescriptor",
            "attributes": { "name": postscript(&t.font_family, t.weight, t.italic), "size": t.font_size },
        },
        "MSAttributedStringColorAttribute": color(ink, None),
        "kerning": t.letter_spacing.resolve(t.font_size),
        "paragraphStyle": paragraph,
    });
    if t.underline.is_some() {
        v["underlineStyle"] = json!(1);
    }
    if t.strikethrough.is_some() {
        v["strikethroughStyle"] = json!(1);
    }
    match t.case {
        TextCase::Upper => v["MSAttributedStringTextTransformAttribute"] = json!(1),
        TextCase::Lower => v["MSAttributedStringTextTransformAttribute"] = json!(2),
        _ => {}
    }
    v
}

/// A family, weight and slant as the PostScript name Sketch looks a face up by:
/// `Inter-SemiBoldItalic`. The inverse of `sketch_in::font_face`.
pub(crate) fn postscript(family: &str, weight: u16, italic: bool) -> String {
    let face = match weight {
        0..=150 => "Thin",
        151..=250 => "ExtraLight",
        251..=350 => "Light",
        351..=450 => "Regular",
        451..=550 => "Medium",
        551..=650 => "SemiBold",
        651..=750 => "Bold",
        751..=850 => "ExtraBold",
        _ => "Black",
    };
    let face = match (face, italic) {
        ("Regular", true) => "Italic".to_owned(),
        (f, true) => format!("{f}Italic"),
        (f, false) => f.to_owned(),
    };
    format!("{}-{face}", family.replace(' ', ""))
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// A point as Sketch writes one: `{x, y}`.
fn point(p: Point) -> String {
    let n = |v: f64| {
        let v = if v.is_finite() {
            (v * 1e6).round() / 1e6
        } else {
            0.0
        };
        let v = if v == 0.0 { 0.0 } else { v };
        format!("{v}")
    };
    format!("{{{}, {}}}", n(p.x), n(p.y))
}

/// A corner of a rectangle's unit box, rounded by `radius`.
fn corner(x: f64, y: f64, radius: f64) -> Value {
    curve_point(Point::new(x, y), None, None, radius)
}

fn curve_point(at: Point, to: Option<Point>, from: Option<Point>, radius: f64) -> Value {
    json!({
        "_class": "curvePoint",
        "cornerRadius": radius.max(0.0),
        "cornerStyle": 0,
        "curveFrom": point(from.unwrap_or(at)),
        "curveTo": point(to.unwrap_or(at)),
        "hasCurveFrom": from.is_some(),
        "hasCurveTo": to.is_some(),
        "curveMode": if to.is_some() || from.is_some() { 4 } else { 1 },
        "point": point(at),
    })
}

/// The four points of Sketch's own oval.
fn oval_points() -> Value {
    let k = 0.5 * 0.552_284_749_8;
    let p = |x: f64, y: f64, to: (f64, f64), from: (f64, f64)| {
        let mut v = curve_point(
            Point::new(x, y),
            Some(Point::new(to.0, to.1)),
            Some(Point::new(from.0, from.1)),
            0.0,
        );
        v["curveMode"] = json!(2);
        v
    };
    json!([
        p(0.5, 1.0, (0.5 - k, 1.0), (0.5 + k, 1.0)),
        p(1.0, 0.5, (1.0, 0.5 + k), (1.0, 0.5 - k)),
        p(0.5, 0.0, (0.5 + k, 0.0), (0.5 - k, 0.0)),
        p(0.0, 0.5, (0.0, 0.5 - k), (0.0, 0.5 + k)),
    ])
}

/// A path in its container's space as a shape path, or a shape group of its
/// parts when it has several: Sketch's path holds one.
fn path_layer(id: &str, name: &str, path: &BezPath, rule: FillRule) -> Option<Value> {
    let extent = path.bounding_box();
    let parts = subpaths(path);
    if parts.len() < 2 {
        return single_path(id, name, path);
    }
    let mut g = base(id, name, "shapeGroup", extent);
    group(&mut g);
    g["windingRule"] = json!(match rule {
        FillRule::EvenOdd => 1,
        FillRule::NonZero => 0,
    });
    let to_group = Affine::translate(-extent.origin().to_vec2());
    let first = parts.first().map_or(0.0, |p| p.area());
    let mut layers = Vec::new();
    for (i, mut part) in parts.into_iter().enumerate() {
        // Under even-odd every part cuts or adds by difference. Under non-zero a
        // part winding the first's way adds to it and one winding against it
        // cuts, as nested holes draw.
        let op = match rule {
            FillRule::NonZero if part.area() * first > 0.0 => 0,
            _ => 3,
        };
        part.apply_affine(to_group);
        let mut l = single_path(&uuid(&[id, "part", &i.to_string()]), "Part", &part)?;
        l["booleanOperation"] = json!(if i == 0 { -1 } else { op });
        layers.push(l);
    }
    g["layers"] = Value::Array(layers);
    Some(g)
}

/// A path's subpaths, each its own path.
fn subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) || out.is_empty() {
            out.push(BezPath::new());
        }
        if let Some(last) = out.last_mut() {
            last.push(*el);
        }
    }
    out.retain(|p| p.elements().len() > 1);
    out
}

/// One subpath as a shape path: its points fractions of its frame, each point's
/// handles its curve in and out.
fn single_path(id: &str, name: &str, path: &BezPath) -> Option<Value> {
    let extent = path.bounding_box();
    let (w, h) = (extent.width(), extent.height());
    let frac = |p: Point| {
        Point::new(
            if w > 1e-9 { (p.x - extent.x0) / w } else { 0.0 },
            if h > 1e-9 { (p.y - extent.y0) / h } else { 0.0 },
        )
    };
    // Anchors with their incoming and outgoing handles, as Sketch keeps them.
    let mut anchors: Vec<(Point, Option<Point>, Option<Point>)> = Vec::new();
    let mut closed = false;
    let mut current = Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                anchors.push((p, None, None));
                current = p;
            }
            PathEl::LineTo(p) => {
                anchors.push((p, None, None));
                current = p;
            }
            PathEl::QuadTo(c, p) => {
                let c1 = current + (c - current) * (2.0 / 3.0);
                let c2 = p + (c - p) * (2.0 / 3.0);
                if let Some(last) = anchors.last_mut() {
                    last.2 = Some(c1);
                }
                anchors.push((p, Some(c2), None));
                current = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                if let Some(last) = anchors.last_mut() {
                    last.2 = Some(c1);
                }
                anchors.push((p, Some(c2), None));
                current = p;
            }
            PathEl::ClosePath => closed = true,
        }
    }
    // A closed path that ends on its start shares that anchor.
    if closed
        && anchors.len() > 1
        && let (Some(first), Some(last)) = (anchors.first().copied(), anchors.last().copied())
        && (first.0 - last.0).hypot() < 1e-9
    {
        anchors.pop();
        if let Some(f) = anchors.first_mut() {
            f.1 = last.1;
        }
    }
    if anchors.is_empty() {
        return None;
    }
    let points: Vec<Value> = anchors
        .iter()
        .map(|(p, to, from)| curve_point(frac(*p), to.map(frac), from.map(frac), 0.0))
        .collect();
    let mut l = base(id, name, "shapePath", extent);
    l["points"] = Value::Array(points);
    shape(&mut l, false);
    l["isClosed"] = json!(closed);
    Some(l)
}

/// A text's box in its own space: the fixed size, or the laid-out lines.
fn text_box(res: &Resolved, id: NodeId, kind: &NodeKind) -> Rect {
    let laid = res.text_layout(id).map(|l| l.bounds());
    match kind {
        NodeKind::Text {
            sizing: TextSizing::Fixed(size),
            ..
        } => Rect::from_origin_size(Point::ZERO, *size),
        NodeKind::Text {
            sizing: TextSizing::AutoHeight(w),
            ..
        } => Rect::new(0.0, 0.0, *w, laid.map_or(0.0, |l| l.y1.max(0.0))),
        _ => laid.unwrap_or(Rect::ZERO),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Circle;

    /// The operations a several-part path's later parts join by.
    fn ops(path: &BezPath, rule: FillRule) -> Vec<i64> {
        let g = path_layer("ID", "Path", path, rule).expect("a layer");
        g["layers"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|l| l["booleanOperation"].as_i64().unwrap())
            .collect()
    }

    /// Under non-zero, a part winding the first's way adds and one winding
    /// against it cuts; under even-odd every part cuts or adds by difference.
    /// flip: `path_layer` joining every part by difference → fails at the
    /// overlapping circles.
    #[test]
    fn a_several_part_path_joins_its_parts_by_its_fill_rule() {
        let mut same = Circle::new((0.0, 0.0), 10.0).to_path(0.1);
        same.extend(Circle::new((8.0, 0.0), 10.0).to_path(0.1));
        let mut ring = Circle::new((0.0, 0.0), 10.0).to_path(0.1);
        ring.extend(Circle::new((0.0, 0.0), 5.0).to_path(0.1).reverse_subpaths());
        assert_eq!(ops(&same, FillRule::NonZero), [0]);
        assert_eq!(ops(&ring, FillRule::NonZero), [3]);
        assert_eq!(ops(&same, FillRule::EvenOdd), [3]);
    }

    /// Every weight in hundreds, upright and italic, comes back from the name it
    /// is written as, spaced families included.
    #[test]
    fn a_postscript_name_reads_back_as_its_face() {
        for family in ["Inter", "Times New Roman"] {
            for weight in (100..=900).step_by(100) {
                for italic in [false, true] {
                    let name = postscript(family, weight, italic);
                    assert_eq!(
                        ondin_core::sketch_in::font_face(&name),
                        (family.to_owned(), weight, italic),
                        "{name}"
                    );
                }
            }
        }
    }
}
