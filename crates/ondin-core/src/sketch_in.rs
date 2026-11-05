//! Sketch import: a `.sketch` file read into a new document — its pages, layers,
//! symbols, shared styles and colour swatches as Ondin's own.
//!
//! A `.sketch` file is a zip of JSON (`document.json`, `pages/<id>.json`) and
//! pictures (`images/<hash>.png`). The zip is the caller's: this module reads
//! entries through [`Archive`] and pictures through [`ImageSink`], so core stays
//! free of an archive reader and a decoder. Lunacy saves the same format.
//!
//! The document is built in two passes, each through [`Document::apply`]. The
//! first writes every page, layer, style and variable, with an empty group in the
//! place of each symbol instance. The second replaces those groups with instances
//! of their masters, the masters holding no placeholder going first, so a symbol
//! nested in another is whole before the outer one is copied; then it writes the
//! instances' text overrides.
//!
//! What has no counterpart is counted in [`Read::report`], one row per kind, worded
//! as `svg_in` reports are.

use crate::container::{AutoMargins, Insets, LengthPct};
use crate::document::Document;
use crate::effect::{Effect, EffectKind, Shadow};
use crate::export::{ExportFormat, ExportScale, ExportSpec};
use crate::id::{IdSource, NodeId};
use crate::image::{GradientBrush, ImageFit, ImageId, image_brush};
use crate::item::keyed_by_position;
use crate::kurbo::{Affine, BezPath, Point, RoundedRectRadii, Size, Vec2};
use crate::node::{BlendMode, BoolOp, EllipseArc, Fill, FillRule, NodeKind, Stroke, StrokeAlign};
use crate::op::{OpError, Operation, Transaction};
use crate::peniko::{self, Color};
use crate::style::{Style, StyleId, StyleSlot, StyleValue};
use crate::svg_in::ImageSink;
use crate::typography::{
    CharAttr, CharSpans, Decoration, Length, ParaSpans, ParagraphStyle, TextAlign, TextStyle,
};
use crate::variable::{BrushSource, Cell, Mode, VarId, VarKind, Variable};
use crate::{BlockStyle, TextSizing, VerticalAlign};
use rustc_hash::FxHashMap;
use serde_json::Value;
use std::collections::BTreeMap;

pub use crate::foreign::{Archive, Part, Unmapped};
use crate::foreign::{Names, Tally, arr, b_of, num, sniff, solid, str_of, utf16_to_bytes};

/// A Sketch file read into a document.
pub struct Read {
    pub doc: Document,
    /// What did not come in as it was, one row per kind, in a stable order.
    pub report: Vec<Unmapped>,
    /// Every font family the text asks for, with how many runs ask for it, so the
    /// caller can say which are not installed.
    pub families: BTreeMap<String, usize>,
}

/// Why a file could not be read at all.
#[derive(Debug, thiserror::Error)]
pub enum SketchError {
    #[error("not a Sketch file: it has no document.json")]
    NoDocument,
    #[error("not a Sketch file: {0}")]
    Json(String),
    #[error("the file has no pages")]
    NoPages,
    #[error("the document did not build: {0}")]
    Build(#[from] OpError),
}

/// Read the Sketch file behind `archive` into a new document, minting from `ids`
/// and handing pictures to `images`.
pub fn read(
    archive: &mut dyn Archive,
    ids: &mut IdSource,
    images: &mut dyn ImageSink,
) -> Result<Read, SketchError> {
    let document = json(archive, "document.json")?.ok_or(SketchError::NoDocument)?;
    let refs: Vec<String> = arr(&document, "pages")
        .iter()
        .filter_map(|p| p.get("_ref").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    let mut pages = Vec::new();
    let mut lost = 0;
    for r in &refs {
        match json(archive, &format!("{r}.json"))? {
            Some(page) => pages.push(page),
            None => lost += 1,
        }
    }
    if pages.is_empty() {
        return Err(SketchError::NoPages);
    }
    let first = ids.mint();
    let mut b = Builder {
        doc: Document::new(first),
        ids,
        archive,
        images,
        ops: Vec::new(),
        layers: FxHashMap::default(),
        masters: FxHashMap::default(),
        pending: Vec::new(),
        pins: FxHashMap::default(),
        swatches: FxHashMap::default(),
        layer_styles: FxHashMap::default(),
        text_styles: FxHashMap::default(),
        pictures: FxHashMap::default(),
        tally: Tally::default(),
        names: Names::default(),
        families: BTreeMap::new(),
        archived: crate::keyed_archive::Budget::default(),
    };
    if lost > 0 {
        b.count("Pages missing from the file", lost, "", "Left out");
    }
    b.swatches(&document);
    b.shared_styles(&document);
    for (i, page) in pages.iter().enumerate() {
        let id = if i == 0 { first } else { b.ids.mint() };
        b.page(page, id, i);
    }
    // Library symbols are full copies of their masters; they land as local ones.
    let foreign: Vec<Value> = arr(&document, "foreignSymbols")
        .iter()
        .filter_map(|f| f.get("symbolMaster").cloned())
        .collect();
    if !foreign.is_empty() {
        let id = b.ids.mint();
        let page = serde_json::json!({ "name": "Library symbols", "layers": foreign });
        b.page(&page, id, pages.len());
        b.count(
            "Library symbols",
            foreign.len(),
            "",
            "Brought in as local components",
        );
    }
    let ops = std::mem::take(&mut b.ops);
    b.commit(ops)?;
    b.place_instances()?;
    let Builder {
        doc,
        tally,
        families,
        ..
    } = b;
    Ok(Read {
        doc,
        report: tally.into_report(),
        families,
    })
}

/// How many nodes placed symbols may add to the document. Each instance
/// copies its master whole, nested instances included, so masters holding two
/// instances of the one before double per level: a few KB of JSON would ask for
/// billions of nodes. Placing costs more than linearly in the document's size:
/// a quarter million nodes of doubling symbols take about a minute in a release
/// build and a million take minutes, so the budget is the quarter million. A
/// test build's is small enough to reach.
const MAX_NODES: usize = if cfg!(test) { 4_000 } else { 250_000 };

/// The entry at `name` parsed, `None` when there is none.
fn json(archive: &mut dyn Archive, name: &str) -> Result<Option<Value>, SketchError> {
    archive
        .entry(name)
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|e| SketchError::Json(e.to_string())))
        .transpose()
}

/// A symbol instance waiting for its master to be whole.
struct Pending {
    /// The empty group standing in its place.
    placeholder: NodeId,
    symbol: String,
    /// The master it sits inside, which cannot be copied until it is placed.
    inside: Option<String>,
    /// The instance's own Sketch id, for an override that reaches through it.
    sketch_id: String,
    layer: Value,
}

/// A shared layer style split into the slots Ondin styles one at a time.
#[derive(Default)]
struct LayerStyle {
    fills: Option<(StyleId, Vec<Fill>)>,
    strokes: Option<(StyleId, Vec<Stroke>)>,
    effects: Option<(StyleId, Vec<Effect>)>,
}

struct Builder<'a> {
    doc: Document,
    ids: &'a mut IdSource,
    archive: &'a mut dyn Archive,
    images: &'a mut dyn ImageSink,
    ops: Vec<Operation>,
    /// Sketch object id → the node made for it, for overrides.
    layers: FxHashMap<String, NodeId>,
    /// Symbol id → its master's node.
    masters: FxHashMap<String, NodeId>,
    pending: Vec<Pending>,
    /// An instance's insets inside its master, by its Sketch id, for its copy.
    pins: FxHashMap<String, Insets>,
    swatches: FxHashMap<String, VarId>,
    layer_styles: FxHashMap<String, LayerStyle>,
    text_styles: FxHashMap<String, (StyleId, TextStyle)>,
    /// Picture reference → its id, `None` for one that would not read.
    pictures: FxHashMap<String, Option<ImageId>>,
    tally: Tally,
    /// Style names by slot; `None` holds the colour variables'.
    names: Names<Option<StyleSlot>>,
    families: BTreeMap<String, usize>,
    /// What reading the file's archived text may still cost.
    archived: crate::keyed_archive::Budget,
}

impl Builder<'_> {
    fn count(&mut self, what: &'static str, n: usize, unit: &'static str, how: &'static str) {
        self.tally.count(what, n, unit, how);
    }

    fn push(&mut self, op: Operation) {
        self.ops.push(op);
    }

    /// Apply `ops` with what the editor's commit owes them.
    fn commit(&mut self, ops: Vec<Operation>) -> Result<(), OpError> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut tx = Transaction(ops);
        crate::propagate::owed(&self.doc, &mut tx, self.ids);
        self.doc.apply(&tx).map(|_| ())
    }

    /// Colour swatches become colour variables, in one mode.
    fn swatches(&mut self, document: &Value) {
        let swatches = document
            .get("sharedSwatches")
            .map(|s| arr(s, "objects"))
            .unwrap_or_default();
        if swatches.is_empty() {
            return;
        }
        let mode = self.ids.mint_mode();
        self.push(Operation::AddMode {
            collection: crate::variable::CollectionId::FIRST,
            mode: Mode {
                id: mode,
                name: "Default".into(),
            },
            index: 0,
            cells: vec![],
        });
        for s in swatches {
            let Some(value) = s.get("value") else {
                continue;
            };
            let var = self.ids.mint_var();
            self.swatches
                .insert(str_of(s, "do_objectID").to_owned(), var);
            let name = self
                .names
                .unique(None, str_of(s, "name"), "Colour", &mut self.tally);
            self.push(Operation::AddVariable {
                var: Variable {
                    id: var,
                    name,
                    kind: VarKind::Color,
                    cells: [(mode, Cell::Color(color(value)))].into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            });
        }
    }

    /// Shared layer and text styles, a library's local copies included.
    fn shared_styles(&mut self, document: &Value) {
        let local = |k: &str| {
            document
                .get(k)
                .map(|c| arr(c, "objects").to_vec())
                .unwrap_or_default()
        };
        let foreign = |k: &str| -> Vec<Value> {
            arr(document, k)
                .iter()
                .filter_map(|f| f.get("localSharedStyle").cloned())
                .collect()
        };
        let mut layer = local("layerStyles");
        layer.extend(foreign("foreignLayerStyles"));
        let mut text = local("layerTextStyles");
        text.extend(foreign("foreignTextStyles"));
        for s in &layer {
            let (Some(value), name) = (s.get("value"), str_of(s, "name").to_owned()) else {
                continue;
            };
            let size = Size::new(1.0, 1.0);
            let mut out = LayerStyle::default();
            let fills = self.fills(value, size);
            if !fills.is_empty() {
                let id = self.ids.mint_style();
                self.add_style(
                    id,
                    &name,
                    StyleValue::Fills(keyed_by_position(fills.clone())),
                );
                out.fills = Some((id, fills));
            }
            let strokes = self.strokes(value, size);
            if !strokes.is_empty() {
                let id = self.ids.mint_style();
                self.add_style(
                    id,
                    &name,
                    StyleValue::Strokes(keyed_by_position(strokes.clone())),
                );
                out.strokes = Some((id, strokes));
            }
            let effects = self.effects(value);
            if !effects.is_empty() {
                let id = self.ids.mint_style();
                self.add_style(
                    id,
                    &name,
                    StyleValue::Effects(keyed_by_position(effects.clone())),
                );
                out.effects = Some((id, effects));
            }
            self.layer_styles
                .insert(str_of(s, "do_objectID").to_owned(), out);
        }
        for s in &text {
            let Some(attrs) = s
                .get("value")
                .and_then(|v| v.get("textStyle"))
                .and_then(|t| t.get("encodedAttributes"))
            else {
                continue;
            };
            let attrs = crate::keyed_archive::encoded_attributes(attrs, &mut self.archived);
            let ts = crate::style::type_part(&self.text_style(&attrs));
            let id = self.ids.mint_style();
            self.add_style(
                id,
                str_of(s, "name"),
                StyleValue::Text(Box::new(ts.clone())),
            );
            self.text_styles
                .insert(str_of(s, "do_objectID").to_owned(), (id, ts));
        }
    }

    fn add_style(&mut self, id: StyleId, name: &str, value: StyleValue) {
        let name = self
            .names
            .unique(Some(value.slot()), name, "Style", &mut self.tally);
        self.push(Operation::AddStyle {
            style: Style {
                id,
                name,
                value,
                origin: None,
                forked: None,
                description: String::new(),
            },
        });
    }

    /// One page: the first is the document's own, the rest added.
    fn page(&mut self, page: &Value, id: NodeId, index: usize) {
        let name = str_of(page, "name");
        if index == 0 {
            self.push(Operation::SetName {
                id,
                name: name.to_owned(),
            });
        } else {
            self.push(Operation::AddPage {
                id,
                index,
                name: name.to_owned(),
                ground: None,
                draft: false,
                modes: Vec::new(),
            });
        }
        self.children(page, id, None);
    }

    fn children(&mut self, parent_layer: &Value, parent: NodeId, master: Option<&str>) {
        // Whether a mask below is clipping this run of siblings: Sketch sets the
        // chain break on layers no mask reaches, and only a reached one matters.
        let mut masking = false;
        // The next free index: a layer left out takes none.
        let mut at = 0;
        // A master's children are pinned as Sketch pins them, so a resized
        // instance moves and stretches them as Sketch does.
        let master_box = (str_of(parent_layer, "_class") == "symbolMaster")
            .then(|| Frame::of(parent_layer).size());
        for layer in arr(parent_layer, "layers") {
            if masking && b(layer, "shouldBreakMaskChain") {
                self.count(
                    "Mask chains broken early",
                    1,
                    "layers",
                    "Masked with the layers below them",
                );
            }
            masking |= b(layer, "hasClippingMask");
            if self.layer(layer, parent, at, master) {
                at += 1;
                if let Some(size) = master_box {
                    self.pin(str_of(layer, "do_objectID"), layer, size);
                }
                // Sketch draws an outline mask's own paint under what it clips;
                // Ondin's mask paints nothing, so a copy just above it carries
                // the paint, clipped to the same outline. An alpha mask's paint
                // is the mask itself.
                if b(layer, "hasClippingMask") && paints(layer) && !alpha_mask(layer) {
                    let mut ink = layer.clone();
                    ink["hasClippingMask"] = false.into();
                    ink["do_objectID"] = format!("{}/paint", str_of(layer, "do_objectID")).into();
                    if let Some(o) = ink.as_object_mut() {
                        o.remove("exportOptions");
                        o.remove("flow");
                    }
                    if self.layer(&ink, parent, at, master) {
                        at += 1;
                        if let Some(size) = master_box {
                            self.pin(str_of(&ink, "do_objectID"), &ink, size);
                        }
                    }
                }
            }
        }
    }

    /// Pin the node made for the master child `layer` (Sketch id `sid`) inside
    /// its master of `size`, as its resizing constraint says. A group takes no
    /// insets; an instance's wait for its copy.
    fn pin(&mut self, sid: &str, layer: &Value, size: Size) {
        let class = str_of(layer, "_class");
        if class == "group" {
            return;
        }
        let Some(insets) = insets_of(layer, size) else {
            return;
        };
        if class == "symbolInstance" {
            self.pins.insert(sid.to_owned(), insets);
        } else if let Some(&id) = self.layers.get(sid) {
            self.push(Operation::SetInsets { id, insets });
        }
    }

    /// Count what a layer carries that has no counterpart: prototype links and
    /// smart layout.
    fn notes(&mut self, layer: &Value) {
        if layer.get("flow").is_some() {
            self.count("Prototype links", 1, "", "Left out");
        }
        if str_in(layer, &["groupLayout", "_class"]) == "MSImmutableInferredGroupLayout" {
            self.count("Smart layout", 1, "groups", "Kept at their drawn positions");
        }
    }

    /// One layer and what it holds, at `index` under `parent`. Answers whether a
    /// node was made.
    fn layer(&mut self, layer: &Value, parent: NodeId, index: usize, master: Option<&str>) -> bool {
        let class = str_of(layer, "_class");
        let frame = Frame::of(layer);
        self.notes(layer);
        let id = self.ids.mint();
        let made = match class {
            "artboard" | "symbolMaster" => {
                self.create(
                    id,
                    parent,
                    index,
                    frame.artboard(),
                    frame.placement(),
                    layer,
                );
                let fill = if class == "symbolMaster" {
                    // A master's background shows in its instances only when
                    // it says so; Ondin's main and copies look alike, so the
                    // instances decide.
                    (b(layer, "hasBackgroundColor") && b(layer, "includeBackgroundColorInInstance"))
                        .then(|| layer.get("backgroundColor").map(color))
                        .flatten()
                } else if b(layer, "hasBackgroundColor") {
                    layer.get("backgroundColor").map(color)
                } else {
                    // Sketch draws an artboard without a background white.
                    Some(Color::WHITE)
                };
                if let Some(c) = fill {
                    self.push(Operation::SetFills {
                        id,
                        fills: keyed_by_position([solid(c)]),
                    });
                }
                self.push(Operation::SetClip { id, clip: true });
                let symbol = (class == "symbolMaster").then(|| str_of(layer, "symbolID"));
                if let Some(symbol) = symbol {
                    self.push(Operation::SetComponent {
                        id,
                        component: true,
                    });
                    self.masters.insert(symbol.to_owned(), id);
                }
                self.children(layer, id, symbol.or(master));
                true
            }
            "group" => {
                self.create(id, parent, index, NodeKind::Group, frame.placement(), layer);
                self.style(id, layer, frame.size(), false);
                self.children(layer, id, master);
                true
            }
            "shapeGroup" => self.shape_group(id, parent, index, layer, master),
            "rectangle" | "oval" | "triangle" | "star" | "polygon" | "shapePath" => {
                let kind = shape_kind(class, layer, frame.size());
                self.create(id, parent, index, kind, frame.placement(), layer);
                self.style(id, layer, frame.size(), true);
                true
            }
            "text" => {
                self.text(id, parent, index, layer, &frame);
                true
            }
            "bitmap" => {
                self.create(id, parent, index, frame.rect(), frame.placement(), layer);
                let reference = str_in(layer, &["image", "_ref"]).to_owned();
                match self.picture(&reference) {
                    Some(image) => self.push(Operation::SetFills {
                        id,
                        fills: keyed_by_position([Fill {
                            brush: image_brush(image).into(),
                            visible: true,
                        }]),
                    }),
                    None => self.count("Pictures", 1, "", "Could not be read"),
                }
                self.style(id, layer, frame.size(), false);
                true
            }
            "symbolInstance" => {
                self.create(id, parent, index, NodeKind::Group, frame.placement(), layer);
                self.pending.push(Pending {
                    placeholder: id,
                    symbol: str_of(layer, "symbolID").to_owned(),
                    inside: master.map(str::to_owned),
                    sketch_id: str_of(layer, "do_objectID").to_owned(),
                    layer: layer.clone(),
                });
                true
            }
            "slice" => {
                self.create(
                    id,
                    parent,
                    index,
                    NodeKind::Slice { size: frame.size() },
                    frame.placement(),
                    layer,
                );
                true
            }
            "MSImmutableHotspotLayer" | "hotspot" => {
                self.count("Prototype links", 1, "", "Left out");
                false
            }
            _ => {
                self.count("Layers of an unknown kind", 1, "", "Left out");
                false
            }
        };
        if made {
            self.layers
                .insert(str_of(layer, "do_objectID").to_owned(), id);
        }
        made
    }

    /// A shape group: its one shape carrying the group's style, or a boolean of
    /// its shapes. Answers whether a node was made.
    fn shape_group(
        &mut self,
        id: NodeId,
        parent: NodeId,
        index: usize,
        layer: &Value,
        master: Option<&str>,
    ) -> bool {
        let frame = Frame::of(layer);
        let shapes = arr(layer, "layers");
        let is_part = |s: &Value| is_shape(s) || str_of(s, "_class") == "shapeGroup";
        if !shapes.iter().all(is_part) {
            // A boolean holds shapes only, so this is a group: each shape in its
            // place wearing the group's paints, the rest as themselves.
            self.create(id, parent, index, NodeKind::Group, frame.placement(), layer);
            self.style(id, layer, frame.size(), false);
            let mut at = 0;
            for s in shapes {
                if is_shape(s) {
                    let child = self.ids.mint();
                    let f = Frame::of(s);
                    self.notes(s);
                    let kind = shape_kind(str_of(s, "_class"), s, f.size());
                    self.create(child, id, at, kind, f.placement(), s);
                    self.paint(child, layer, f.size());
                } else if !self.layer(s, id, at, master) {
                    continue;
                }
                at += 1;
            }
            self.count(
                "Shape groups holding other layers",
                1,
                "groups",
                "Each shape drawn on its own",
            );
            return true;
        }
        if let [only] = shapes
            && is_shape(only)
        {
            let inner = Frame::of(only);
            let kind = shape_kind(str_of(only, "_class"), only, inner.size());
            self.create(
                id,
                parent,
                index,
                kind,
                frame.placement() * inner.placement(),
                layer,
            );
            self.style(id, layer, inner.size(), true);
            return true;
        }
        let ops: Vec<i64> = shapes
            .iter()
            .skip(1)
            .map(|s| {
                s.get("booleanOperation")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            })
            .collect();
        let op = match ops.first().copied().unwrap_or(0) {
            1 => BoolOp::Subtract,
            2 => BoolOp::Intersect,
            3 => BoolOp::Exclude,
            _ => BoolOp::Union,
        };
        if ops.windows(2).any(|w| w[0] != w[1]) {
            self.count(
                "Mixed boolean operations",
                1,
                "shapes",
                "Combined with the first one's operation",
            );
        }
        self.create(
            id,
            parent,
            index,
            NodeKind::Boolean { op },
            frame.placement(),
            layer,
        );
        self.style(id, layer, frame.size(), true);
        for (i, s) in shapes.iter().enumerate() {
            if str_of(s, "_class") == "shapeGroup" {
                let child = self.ids.mint();
                self.shape_group(child, id, i, s, master);
            } else if is_shape(s) {
                self.notes(s);
                let child = self.ids.mint();
                let f = Frame::of(s);
                let kind = shape_kind(str_of(s, "_class"), s, f.size());
                self.create(child, id, i, kind, f.placement(), s);
            }
        }
        true
    }

    /// A node with a layer's name and its common flags.
    fn create(
        &mut self,
        id: NodeId,
        parent: NodeId,
        index: usize,
        kind: NodeKind,
        transform: Affine,
        layer: &Value,
    ) {
        self.push(Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(transform),
            name: Some(str_of(layer, "name").to_owned()),
        });
        if layer.get("isVisible").and_then(Value::as_bool) == Some(false) {
            self.push(Operation::SetVisible { id, visible: false });
        }
        if b(layer, "isLocked") {
            self.push(Operation::SetLocked { id, locked: true });
        }
        if b(layer, "hasClippingMask") {
            self.push(Operation::SetMask { id, mask: true });
            if alpha_mask(layer) {
                self.push(Operation::SetMaskMode {
                    id,
                    mode: crate::node::MaskMode::Alpha,
                });
            }
        }
        self.exports(id, layer);
    }

    /// A layer's export presets as its export settings. Sketch's `name` is put
    /// after the layer's name or, by its naming scheme, before it; a suffix goes
    /// through `ExportSpec::take_foreign_suffix`, so the marker Ondin derives is
    /// left for it to add and a raster named without one is counted. A format
    /// Ondin cannot write is counted and left out.
    fn exports(&mut self, id: NodeId, layer: &Value) {
        let mut exports = Vec::new();
        for f in arr_in(layer, &["exportOptions", "exportFormats"]) {
            let format = match str_of(f, "fileFormat").to_ascii_lowercase().as_str() {
                "png" => ExportFormat::Png,
                "jpg" | "jpeg" => ExportFormat::Jpeg,
                "webp" => ExportFormat::Webp,
                "svg" => ExportFormat::Svg,
                "pdf" => ExportFormat::Pdf,
                _ => {
                    self.count("Export formats Ondin lacks", 1, "", "Left out");
                    continue;
                }
            };
            let size = num(f, "absoluteSize").round();
            let scale = match f.get("visibleScaleType").and_then(Value::as_i64) {
                Some(1) if size >= 1.0 => ExportScale::Width(size as u32),
                Some(2) if size >= 1.0 => ExportScale::Height(size as u32),
                _ => match num(f, "scale") as f32 {
                    s if s.is_finite() && s > 0.0 => ExportScale::Times(s),
                    _ => ExportScale::Times(1.0),
                },
            };
            let mut spec = ExportSpec::new(format, scale);
            let name = str_of(f, "name");
            let prefixed = f.get("namingScheme").and_then(Value::as_i64) == Some(1);
            let suffix = match prefixed && !name.is_empty() {
                true => {
                    spec.prefix = name.to_owned();
                    ""
                }
                false => name,
            };
            if spec.take_foreign_suffix(suffix) {
                self.count(
                    "Export names without their scale",
                    1,
                    "",
                    "Given its marker",
                );
            }
            exports.push(spec);
        }
        if !exports.is_empty() {
            self.push(Operation::SetExports {
                id,
                exports: keyed_by_position(exports),
            });
        }
    }

    /// A layer's style onto `id`: opacity, blend, and — for a shape — its fills,
    /// borders and winding; effects for every kind.
    fn style(&mut self, id: NodeId, layer: &Value, size: Size, paints: bool) {
        let Some(style) = layer.get("style") else {
            return;
        };
        let context = style.get("contextSettings");
        let opacity = context.map_or(1.0, |c| num(c, "opacity"));
        if opacity < 1.0 {
            self.push(Operation::SetOpacity {
                id,
                opacity: opacity as f32,
            });
        }
        let blend = context.map_or(0, |c| num(c, "blendMode") as i64);
        if blend != 0 {
            self.push(Operation::SetBlend {
                id,
                blend: blend_mode(blend),
            });
        }
        if paints {
            self.paint(id, layer, size);
        }
        let effect_style = self
            .layer_styles
            .get(str_of(layer, "sharedStyleID"))
            .and_then(|s| s.effects.clone());
        let effects = self.effects(style);
        if !effects.is_empty() {
            self.push(Operation::SetEffects {
                id,
                effects: keyed_by_position(effects.clone()),
            });
        }
        if let Some((s, value)) = effect_style
            && value == effects
        {
            self.push(Operation::SetEffectsStyle { id, style: Some(s) });
        }
    }

    /// `layer`'s fills, borders and winding onto `id`, bound to its shared style
    /// where they match it.
    fn paint(&mut self, id: NodeId, layer: &Value, size: Size) {
        let Some(style) = layer.get("style") else {
            return;
        };
        let shared = self.layer_styles.get(str_of(layer, "sharedStyleID"));
        let (fill_style, stroke_style) = shared
            .map(|s| (s.fills.clone(), s.strokes.clone()))
            .unwrap_or_default();
        let fills = self.fills(style, size);
        if !fills.is_empty() {
            self.push(Operation::SetFills {
                id,
                fills: keyed_by_position(fills.clone()),
            });
        }
        if let Some((s, value)) = fill_style
            && value == fills
        {
            self.push(Operation::SetFillStyle { id, style: Some(s) });
        }
        let strokes = self.strokes(style, size);
        if !strokes.is_empty() {
            self.push(Operation::SetStrokes {
                id,
                strokes: keyed_by_position(strokes.clone()),
            });
        }
        if let Some((s, value)) = stroke_style
            && value == strokes
        {
            self.push(Operation::SetStrokeStyle { id, style: Some(s) });
        }
        if num(style, "windingRule") as i64 == 1 && !fills.is_empty() {
            self.push(Operation::SetFillRule {
                id,
                rule: FillRule::EvenOdd,
            });
        }
    }

    fn fills(&mut self, style: &Value, size: Size) -> Vec<Fill> {
        arr(style, "fills")
            .iter()
            .filter_map(|f| {
                let brush = self.brush(f, size)?;
                Some(Fill {
                    brush,
                    visible: b(f, "isEnabled"),
                })
            })
            .collect()
    }

    fn strokes(&mut self, style: &Value, size: Size) -> Vec<Stroke> {
        let options = style.get("borderOptions");
        let dashes: Vec<f64> = options
            .map(|o| arr(o, "dashPattern"))
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        let cap = match options.map_or(0, |o| num(o, "lineCapStyle") as i64) {
            1 => crate::kurbo::Cap::Round,
            2 => crate::kurbo::Cap::Square,
            _ => crate::kurbo::Cap::Butt,
        };
        let join = match options.map_or(0, |o| num(o, "lineJoinStyle") as i64) {
            1 => crate::kurbo::Join::Round,
            2 => crate::kurbo::Join::Bevel,
            _ => crate::kurbo::Join::Miter,
        };
        let miter = style
            .get("miterLimit")
            .and_then(Value::as_f64)
            .unwrap_or(10.0);
        arr(style, "borders")
            .iter()
            .filter_map(|s| {
                let brush = self.brush(s, size)?;
                Some(Stroke {
                    brush,
                    width: num(s, "thickness"),
                    width_var: None,
                    join,
                    cap,
                    dashes: dashes.clone(),
                    dash_offset: 0.0,
                    miter_limit: miter,
                    dash_fit: false,
                    align: match num(s, "position") as i64 {
                        1 => StrokeAlign::Inside,
                        2 => StrokeAlign::Outside,
                        _ => StrokeAlign::Center,
                    },
                    sides: crate::StrokeSides::All,
                    visible: b(s, "isEnabled"),
                })
            })
            .collect()
    }

    fn effects(&mut self, style: &Value) -> Vec<Effect> {
        let mut out = Vec::new();
        let shadow = |s: &Value| Shadow {
            offset: Vec2::new(num(s, "offsetX"), num(s, "offsetY")),
            blur: num(s, "blurRadius"),
            spread: num(s, "spread"),
            color: s.get("color").map_or(Color::BLACK, color),
            blend: BlendMode::Normal,
        };
        for s in arr(style, "shadows") {
            out.push(Effect {
                visible: b(s, "isEnabled"),
                ..Effect::new(EffectKind::DropShadow(shadow(s)))
            });
        }
        for s in arr(style, "innerShadows") {
            out.push(Effect {
                visible: b(s, "isEnabled"),
                ..Effect::new(EffectKind::InnerShadow(shadow(s)))
            });
        }
        if let Some(blur) = style.get("blur").filter(|b| b_of(b, "isEnabled")) {
            let radius = num(blur, "radius");
            match num(blur, "type") as i64 {
                0 => out.push(Effect::new(EffectKind::LayerBlur { radius })),
                3 => out.push(Effect::new(EffectKind::BackgroundBlur { radius })),
                _ => {
                    self.count("Motion and zoom blur", 1, "layers", "Read as a layer blur");
                    out.push(Effect::new(EffectKind::LayerBlur { radius }));
                }
            }
        }
        out
    }

    /// A fill's or border's paint: a colour (bound to its swatch when it names
    /// one), a gradient, or a picture.
    fn brush(&mut self, f: &Value, size: Size) -> Option<BrushSource> {
        let alpha = f.get("contextSettings").map_or(1.0, |c| num(c, "opacity")) as f32;
        match num(f, "fillType") as i64 {
            0 => {
                let c = f.get("color")?;
                if let Some(var) = c
                    .get("swatchID")
                    .and_then(Value::as_str)
                    .and_then(|s| self.swatches.get(s))
                {
                    return Some(BrushSource::Bound { var: *var, alpha });
                }
                Some(peniko::Brush::Solid(color(c).multiply_alpha(alpha)).into())
            }
            1 => Some(gradient(f.get("gradient")?, size, alpha).into()),
            4 | 5 => {
                let reference = str_in(f, &["image", "_ref"]).to_owned();
                let Some(id) = self.picture(&reference) else {
                    self.count("Pictures", 1, "", "Could not be read");
                    return None;
                };
                let mut brush = image_brush(id);
                if let peniko::Brush::Image(img) = &mut brush {
                    img.image.fit = match num(f, "patternFillType") as i64 {
                        0 => ImageFit::Tile,
                        3 => ImageFit::Fit,
                        _ => ImageFit::Fill,
                    };
                }
                Some(brush.into())
            }
            _ => None,
        }
    }

    /// The picture at `reference`, read once.
    fn picture(&mut self, reference: &str) -> Option<ImageId> {
        if reference.is_empty() {
            return None;
        }
        if let Some(known) = self.pictures.get(reference) {
            return known.clone();
        }
        let id = (|| {
            let bytes = self
                .archive
                .entry(reference)
                .or_else(|| self.archive.entry(&format!("{reference}.png")))?;
            let format = sniff(&bytes)?;
            let (id, entry) = self.images.place(bytes, format)?;
            if !self.doc.has_image(&id)
                && !self
                    .ops
                    .iter()
                    .any(|op| matches!(op, Operation::AddImage { id: added, .. } if *added == id))
            {
                self.ops.push(Operation::AddImage {
                    id: id.clone(),
                    entry,
                });
            }
            Some(id)
        })();
        self.pictures.insert(reference.to_owned(), id.clone());
        id
    }

    /// A text layer: its runs as the node's default and spans, its box as Sketch
    /// sizes it, its paragraphs aligned.
    fn text(&mut self, id: NodeId, parent: NodeId, index: usize, layer: &Value, frame: &Frame) {
        // Sketch before 48 archived its text; read, it has the later shape.
        let archived = layer
            .get("attributedString")
            .and_then(|a| a.get("archivedAttributedString"))
            .map(|a| {
                crate::keyed_archive::attributed_string(str_of(a, "_archive"), &mut self.archived)
            });
        if matches!(archived, Some(None)) {
            self.count(
                "Texts in Sketch's older format",
                1,
                "",
                "Could not be read, left empty",
            );
        }
        let archived = archived.flatten();
        let attributed = archived.as_ref().or(layer.get("attributedString"));
        let content = attributed.map_or("", |a| str_of(a, "string")).to_owned();
        let runs = attributed.map(|a| arr(a, "attributes")).unwrap_or_default();
        let fallback = str_in_value(layer, &["style", "textStyle", "encodedAttributes"])
            .map(|e| crate::keyed_archive::encoded_attributes(e, &mut self.archived));
        let first = runs
            .first()
            .and_then(|r| r.get("attributes"))
            .cloned()
            .or(fallback)
            .unwrap_or(Value::Null);
        let default = self.text_style(&first);
        let ink = run_color(&first).unwrap_or(Color::BLACK);
        let bytes = utf16_to_bytes(&content);
        let mut spans = CharSpans::default();
        for run in runs {
            let Some(attrs) = run.get("attributes") else {
                continue;
            };
            let start = num(run, "location") as usize;
            let len = num(run, "length") as usize;
            let range = bytes(start)..bytes(start + len);
            let style = self.text_style(attrs);
            *self.families.entry(style.font_family.clone()).or_default() += 1;
            for attr in [
                CharAttr::Family(style.font_family.clone()),
                CharAttr::Size(style.font_size),
                CharAttr::Weight(style.weight),
                CharAttr::Italic(style.italic),
                CharAttr::LetterSpacing(style.letter_spacing),
                CharAttr::LineHeight(style.line_height),
                CharAttr::Underline(style.underline),
                CharAttr::Strikethrough(style.strikethrough),
            ] {
                spans.set(range.clone(), attr, &default);
            }
            let c = run_color(attrs).filter(|c| *c != ink);
            spans.set(range, CharAttr::Color(c), &default);
        }
        let paragraph = ParagraphStyle {
            align: match num_in(&first, &["paragraphStyle", "alignment"]) as i64 {
                1 => TextAlign::End,
                2 => TextAlign::Center,
                3 => TextAlign::Justify,
                _ => TextAlign::Start,
            },
            ..ParagraphStyle::default()
        };
        let size = frame.size();
        let sizing = match num(layer, "textBehaviour") as i64 {
            1 => TextSizing::AutoHeight(size.width),
            2 => TextSizing::Fixed(size),
            _ => TextSizing::Auto,
        };
        let block = BlockStyle {
            vertical_align: match num_in(layer, &["style", "textStyle", "verticalAlignment"]) as i64
            {
                1 => VerticalAlign::Middle,
                2 => VerticalAlign::Bottom,
                _ => VerticalAlign::Top,
            },
            ..BlockStyle::default()
        };
        self.create(
            id,
            parent,
            index,
            NodeKind::Text {
                content,
                style: Box::new(default.clone()),
                spans,
                para_spans: ParaSpans::default(),
                paragraph,
                block,
                sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            frame.placement(),
            layer,
        );
        self.push(Operation::SetFills {
            id,
            fills: keyed_by_position([solid(ink)]),
        });
        self.style(id, layer, size, false);
        if let Some((style, typed)) = self.text_styles.get(str_of(layer, "sharedStyleID"))
            && *typed == crate::style::type_part(&default)
        {
            self.push(Operation::SetTypeStyle {
                id,
                style: Some(*style),
            });
        }
    }

    /// The character style a run's attributes describe.
    fn text_style(&mut self, attrs: &Value) -> TextStyle {
        let font = attrs
            .get("MSAttributedStringFontAttribute")
            .and_then(|f| f.get("attributes"));
        let (family, weight, italic) = font_face(font.map_or("", |f| str_of(f, "name")));
        let paragraph = attrs.get("paragraphStyle");
        let line = paragraph
            .and_then(|p| p.get("maximumLineHeight"))
            .and_then(Value::as_f64)
            .filter(|h| *h > 0.0);
        let decoration = |k: &str| (num(attrs, k) as i64 != 0).then(Decoration::default);
        crate::foreign::canonical_style(TextStyle {
            font_family: family,
            font_size: font.map_or(12.0, |f| num(f, "size")),
            weight,
            italic,
            letter_spacing: Length::Px(num(attrs, "kerning")),
            line_height: line.map(Length::Px),
            underline: decoration("underlineStyle"),
            strikethrough: decoration("strikethroughStyle"),
            ..TextStyle::default()
        })
    }

    /// Replace each placeholder with an instance of its master, a master holding
    /// no placeholder first, then write its overrides.
    fn place_instances(&mut self) -> Result<(), SketchError> {
        // A round places every instance whose master holds no placeholder, in
        // three transactions whatever their number: each commit costs in
        // proportion to the whole document, so one per instance made a kit of
        // thousands take minutes.
        let literal = self.doc.len();
        loop {
            let waiting: rustc_hash::FxHashSet<String> = self
                .pending
                .iter()
                .filter_map(|p| p.inside.clone())
                .collect();
            let (ready, rest): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut self.pending)
                .into_iter()
                .partition(|p| !waiting.contains(&p.symbol));
            self.pending = rest;
            if ready.is_empty() {
                break;
            }
            let ready = self.within_budget(ready, literal);
            self.place(ready)?;
        }
        let left = self.pending.len();
        if left > 0 {
            self.count(
                "Symbols that contain themselves",
                left,
                "instances",
                "Left as empty groups",
            );
        }
        Ok(())
    }

    /// The instances of `ready` whose copies keep the nodes placing adds, past
    /// the `literal` ones the file itself holds, within [`MAX_NODES`]; the rest
    /// are counted and stay empty groups. The file's own layers are bounded by
    /// its size, so only the copies, which can double, are budgeted.
    fn within_budget(&mut self, ready: Vec<Pending>, literal: usize) -> Vec<Pending> {
        let mut total = self.doc.len().saturating_sub(literal);
        let mut sizes: FxHashMap<String, usize> = FxHashMap::default();
        let mut kept = Vec::with_capacity(ready.len());
        let mut over = 0;
        for p in ready {
            let size = match (sizes.get(&p.symbol), self.masters.get(&p.symbol)) {
                (Some(&n), _) => n,
                (None, Some(&main)) => {
                    let n = crate::build::subtree_nodes(&self.doc, &[main]).len();
                    sizes.insert(p.symbol.clone(), n);
                    n
                }
                // Counted as missing when it is placed.
                (None, None) => 0,
            };
            if total.saturating_add(size) > MAX_NODES {
                over += 1;
            } else {
                total += size;
                kept.push(p);
            }
        }
        if over > 0 {
            self.count(
                "Symbols too large to place",
                over,
                "instances",
                "Left as empty groups",
            );
        }
        kept
    }

    /// Replace each of `ready`'s placeholders with a copy of its master, none of
    /// which holds a placeholder: the copies in one transaction, their styles in
    /// a second, their overrides in a third.
    fn place(&mut self, ready: Vec<Pending>) -> Result<(), SketchError> {
        let mut ops = Vec::new();
        // Copies already inserted per holder, at their indices before any
        // insert, so a later one in the same holder lands past them; the
        // placeholders go only at the end, so those indices hold.
        let mut added: FxHashMap<NodeId, Vec<usize>> = FxHashMap::default();
        let mut placed = Vec::new();
        for p in ready {
            if let Some(root) = self.copy_of(&p, &mut added, &mut ops) {
                placed.push((root, p));
            }
        }
        ops.extend(
            placed
                .iter()
                .map(|(_, p)| Operation::DeleteNode { id: p.placeholder }),
        );
        self.commit(ops)?;
        for (root, p) in &placed {
            let frame = Frame::of(&p.layer);
            self.layers.insert(p.sketch_id.clone(), *root);
            self.style(*root, &p.layer, frame.size(), false);
            self.exports(*root, &p.layer);
        }
        let ops = std::mem::take(&mut self.ops);
        self.commit(ops)?;
        let mut ops = Vec::new();
        for (root, p) in &placed {
            self.overrides(*root, &p.layer, &mut ops);
        }
        self.commit(ops)?;
        Ok(())
    }

    /// The operations copying `p`'s master into its placeholder's place, onto
    /// `ops`, and the copy's root; `None`, counted, when it cannot be copied.
    fn copy_of(
        &mut self,
        p: &Pending,
        added: &mut FxHashMap<NodeId, Vec<usize>>,
        ops: &mut Vec<Operation>,
    ) -> Option<NodeId> {
        let Some(&main) = self.masters.get(&p.symbol) else {
            self.count("Missing symbols", 1, "instances", "Left as empty groups");
            return None;
        };
        let unplaced = |b: &mut Self| {
            b.count(
                "Symbols that could not be placed",
                1,
                "instances",
                "Left as empty groups",
            );
        };
        let (Some(holder), Some(template)) = (
            self.doc.get(p.placeholder).and_then(|n| n.parent()),
            self.doc.capture_subtree(main),
        ) else {
            unplaced(self);
            return None;
        };
        let Some(base) = self
            .doc
            .get(holder)
            .and_then(|n| n.children().iter().position(|c| *c == p.placeholder))
        else {
            unplaced(self);
            return None;
        };
        let done = added.entry(holder).or_default();
        let index = base + done.iter().filter(|i| **i <= base).count();
        let (tx, made) = crate::build::insert_subtrees(
            &self.doc,
            self.ids,
            &[crate::build::Placement {
                nodes: template,
                parent: holder,
                index: Some(index),
            }],
            Vec2::ZERO,
        );
        let Some(&root) = made.first() else {
            unplaced(self);
            return None;
        };
        added.entry(holder).or_default().push(base);
        let frame = Frame::of(&p.layer);
        ops.extend(tx.0);
        ops.push(Operation::SetTransform {
            id: root,
            transform: frame.placement(),
        });
        ops.push(Operation::SetName {
            id: root,
            name: str_of(&p.layer, "name").to_owned(),
        });
        if let Some(&insets) = self.pins.get(&p.sketch_id) {
            ops.push(Operation::SetInsets { id: root, insets });
        }
        let master_size = match self.doc.get(main).map(|n| n.kind()) {
            Some(NodeKind::Artboard { size, .. }) => *size,
            _ => frame.size(),
        };
        if (master_size.to_vec2() - frame.size().to_vec2()).hypot() > 0.01 {
            ops.push(Operation::SetGeometry {
                id: root,
                geometry: crate::GeometryPatch::Size(frame.size()),
            });
            self.count(
                "Resized symbols",
                1,
                "instances",
                "Their layers follow their pins, except inside groups",
            );
        }
        // The placeholder's own flags, written again on the copy.
        if let Some(n) = self.doc.get(p.placeholder) {
            if !n.visible() {
                ops.push(Operation::SetVisible {
                    id: root,
                    visible: false,
                });
            }
            if n.locked() {
                ops.push(Operation::SetLocked {
                    id: root,
                    locked: true,
                });
            }
            if n.mask() {
                self.count("Masking symbols", 1, "instances", "Left unmasked");
            }
        }
        Some(root)
    }

    /// An instance's overrides onto `ops`: text is written onto its copy; the
    /// rest is counted.
    fn overrides(&mut self, root: NodeId, layer: &Value, ops: &mut Vec<Operation>) {
        for o in arr(layer, "overrideValues") {
            let name = str_of(o, "overrideName");
            let Some((path, property)) = name.rsplit_once('_') else {
                continue;
            };
            match property {
                "stringValue" => {
                    let Some(value) = o.get("value").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(target) = self.copy_at(root, path) else {
                        continue;
                    };
                    ops.push(Operation::SetText {
                        id: target,
                        content: value.to_owned(),
                        spans: CharSpans::default(),
                        para_spans: ParaSpans::default(),
                    });
                }
                "symbolID" => self.count("Symbol swaps", 1, "", "Left out"),
                "image" => self.count("Picture overrides", 1, "", "Left out"),
                "layerStyle" | "textStyle" => self.count("Style overrides", 1, "", "Left out"),
                _ => self.count("Other overrides", 1, "", "Left out"),
            }
        }
    }

    /// The node inside the instance at `root` that is the copy of the Sketch layer
    /// path `a/b/…`, each step a layer of the symbol the step before it names.
    /// Each step is looked for strictly below the last, so a path repeating
    /// one step cannot rebuild the same subtree step after step.
    fn copy_at(&self, root: NodeId, path: &str) -> Option<NodeId> {
        let mut at = root;
        for step in path.split('/') {
            let source = *self.layers.get(step)?;
            at = crate::build::subtree_nodes(&self.doc, &[at])
                .into_iter()
                .skip(1)
                .find(|&n| self.copies(n, source))?;
        }
        Some(at)
    }

    /// Whether `n`'s chain of links passes through `source`.
    fn copies(&self, n: NodeId, source: NodeId) -> bool {
        let mut link = self.doc.get(n).and_then(|n| n.link());
        while let Some(l) = link {
            if l == source {
                return true;
            }
            link = self.doc.get(l).and_then(|n| n.link());
        }
        false
    }
}

/// A layer's box and orientation, as Sketch stores them.
struct Frame {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    rotation: f64,
    flip_x: bool,
    flip_y: bool,
}

impl Frame {
    fn of(layer: &Value) -> Self {
        let f = layer.get("frame");
        let g = |k: &str| f.map_or(0.0, |f| num(f, k));
        Self {
            x: g("x"),
            y: g("y"),
            w: g("width").max(0.0),
            h: g("height").max(0.0),
            rotation: num(layer, "rotation"),
            flip_x: b(layer, "isFlippedHorizontal"),
            flip_y: b(layer, "isFlippedVertical"),
        }
    }

    fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }

    /// Where the layer sits in its parent: at its frame's corner, turned and
    /// flipped about the frame's centre. Sketch's rotation runs counter-clockwise
    /// on screen, which in y-down space is a negative angle.
    fn placement(&self) -> Affine {
        let c = Vec2::new(self.w / 2.0, self.h / 2.0);
        let flip = Affine::scale_non_uniform(
            if self.flip_x { -1.0 } else { 1.0 },
            if self.flip_y { -1.0 } else { 1.0 },
        );
        Affine::translate((self.x, self.y))
            * Affine::translate(c)
            * Affine::rotate(-self.rotation.to_radians())
            * flip
            * Affine::translate(-c)
    }

    fn artboard(&self) -> NodeKind {
        NodeKind::Artboard {
            size: self.size(),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        }
    }

    fn rect(&self) -> NodeKind {
        NodeKind::Rect {
            size: self.size(),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        }
    }
}

fn is_shape(layer: &Value) -> bool {
    matches!(
        str_of(layer, "_class"),
        "rectangle" | "oval" | "triangle" | "star" | "polygon" | "shapePath"
    )
}

/// The node kind for a shape layer: a rectangle whose points are its four
/// corners as a rect, an oval as an ellipse, anything else as the path its points
/// draw.
fn shape_kind(class: &str, layer: &Value, size: Size) -> NodeKind {
    // Older files (Sketch 47's, for one) keep the points in a nested `path`.
    let outline = layer.get("path").filter(|_| layer.get("points").is_none());
    let points = arr(outline.unwrap_or(layer), "points");
    let closed = b(outline.unwrap_or(layer), "isClosed");
    match class {
        "oval" => NodeKind::Ellipse {
            size,
            arc: EllipseArc::default(),
        },
        "rectangle" if is_box(points) => {
            let r = |i: usize| {
                points
                    .get(i)
                    .map_or(0.0, |p| num(p, "cornerRadius"))
                    .max(0.0)
            };
            let fixed = num(layer, "fixedRadius");
            let radii = if points.iter().all(|p| num(p, "cornerRadius") == 0.0) && fixed > 0.0 {
                RoundedRectRadii::from_single_radius(fixed)
            } else {
                RoundedRectRadii::new(r(0), r(1), r(2), r(3))
            };
            NodeKind::Rect {
                size,
                corner_radii: radii,
                corner_smoothing: 0.0,
            }
        }
        _ => {
            let path = path_of(points, closed, size);
            let corner_radii = points.iter().map(|p| num(p, "cornerRadius")).collect();
            NodeKind::Path {
                path,
                corner_radii: trimmed(corner_radii),
                corner_smoothing: 0.0,
            }
        }
    }
}

/// A radius list with its trailing zeros dropped, as a path that rounds nothing
/// carries none.
fn trimmed(mut radii: Vec<f64>) -> Vec<f64> {
    while radii.last() == Some(&0.0) {
        radii.pop();
    }
    radii
}

/// Whether `points` are the unit box's four corners, straight, from the top left.
fn is_box(points: &[Value]) -> bool {
    let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    points.len() == 4
        && points.iter().zip(corners).all(|(p, (x, y))| {
            let at = pt(str_of(p, "point"));
            let straight = |k: &str| (pt(str_of(p, k)) - at).hypot() < 1e-6;
            (at.x - x).abs() < 1e-6
                && (at.y - y).abs() < 1e-6
                && (straight("curveFrom") || !b(p, "hasCurveFrom"))
                && (straight("curveTo") || !b(p, "hasCurveTo"))
        })
}

/// The outline Sketch's curve points draw in a box of `size`: each segment from a
/// point's `curveFrom` to the next point's `curveTo`, a line where both sit on
/// their points.
fn path_of(points: &[Value], closed: bool, size: Size) -> BezPath {
    let at = |v: &Value, k: &str| {
        let p = pt(str_of(v, k));
        Point::new(p.x * size.width, p.y * size.height)
    };
    let mut path = BezPath::new();
    let Some(first) = points.first() else {
        return path;
    };
    path.move_to(at(first, "point"));
    let n = points.len();
    let segments = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segments {
        let (a, b) = (&points[i], &points[(i + 1) % n]);
        let (p0, p1) = (at(a, "point"), at(b, "point"));
        let c0 = if b_of(a, "hasCurveFrom") {
            at(a, "curveFrom")
        } else {
            p0
        };
        let c1 = if b_of(b, "hasCurveTo") {
            at(b, "curveTo")
        } else {
            p1
        };
        if (c0 - p0).hypot() < 1e-9 && (c1 - p1).hypot() < 1e-9 {
            path.line_to(p1);
        } else {
            path.curve_to(c0, c1, p1);
        }
    }
    if closed {
        path.close_path();
    }
    path
}

/// A gradient in a box of `size`: Sketch's points are fractions of the box.
fn gradient(g: &Value, size: Size, alpha: f32) -> crate::Brush {
    let from = pt(str_of(g, "from"));
    let to = pt(str_of(g, "to"));
    let stops: Vec<peniko::ColorStop> = arr(g, "stops")
        .iter()
        .map(|s| peniko::ColorStop {
            offset: num(s, "position").clamp(0.0, 1.0) as f32,
            color: peniko::color::DynamicColor::from_alpha_color(
                s.get("color").map_or(Color::BLACK, color),
            ),
        })
        .collect();
    let box_ = Affine::scale_non_uniform(size.width.max(1e-9), size.height.max(1e-9));
    let (gradient, transform) = match num(g, "gradientType") as i64 {
        1 => (
            peniko::Gradient::new_radial((from.x, from.y), (to - from).hypot() as f32),
            box_,
        ),
        2 => (
            peniko::Gradient::new_sweep((0.5, 0.5), 0.0, std::f32::consts::TAU),
            box_,
        ),
        _ => {
            let (a, b) = (box_ * from, box_ * to);
            (
                peniko::Gradient::new_linear((a.x, a.y), (b.x, b.y)),
                Affine::IDENTITY,
            )
        }
    };
    peniko::Brush::Gradient(GradientBrush {
        gradient: gradient.with_stops(stops.as_slice()),
        transform,
        opacity: alpha,
    })
}

fn color(c: &Value) -> Color {
    let ch = |k: &str| num(c, k).clamp(0.0, 1.0) as f32;
    Color::new([ch("red"), ch("green"), ch("blue"), ch("alpha")])
}

fn run_color(attrs: &Value) -> Option<Color> {
    attrs.get("MSAttributedStringColorAttribute").map(color)
}

fn blend_mode(n: i64) -> BlendMode {
    match n {
        1 => BlendMode::Darken,
        2 => BlendMode::Multiply,
        3 => BlendMode::ColorBurn,
        4 => BlendMode::Lighten,
        5 => BlendMode::Screen,
        6 => BlendMode::ColorDodge,
        7 => BlendMode::Overlay,
        8 => BlendMode::SoftLight,
        9 => BlendMode::HardLight,
        10 => BlendMode::Difference,
        11 => BlendMode::Exclusion,
        12 => BlendMode::Hue,
        13 => BlendMode::Saturation,
        14 => BlendMode::Color,
        15 => BlendMode::Luminosity,
        _ => BlendMode::Normal,
    }
}

/// A PostScript font name (`Inter-SemiBoldItalic`, `TimesNewRomanPSMT`) as a
/// family, a weight and whether it is italic.
pub fn font_face(postscript: &str) -> (String, u16, bool) {
    let (family, face) = postscript
        .split_once('-')
        .unwrap_or((postscript, "Regular"));
    let family = family.trim_start_matches('.');
    let family = ["PSMT", "MT", "PS"]
        .iter()
        .find_map(|s| family.strip_suffix(s))
        .unwrap_or(family);
    let lower = face.to_ascii_lowercase();
    let italic = lower.contains("italic") || lower.contains("oblique");
    let weight = [
        ("hairline", 100),
        ("thin", 100),
        ("extralight", 200),
        ("ultralight", 200),
        ("semibold", 600),
        ("demibold", 600),
        ("extrabold", 800),
        ("ultrabold", 800),
        ("light", 300),
        ("medium", 500),
        ("bold", 700),
        ("heavy", 800),
        ("black", 900),
    ]
    .iter()
    .find(|(k, _)| lower.contains(k))
    .map_or(400, |(_, w)| *w);
    (spaced(family), weight, italic)
}

/// `TimesNewRoman` as `Times New Roman`, `SFProDisplay` as `SF Pro Display`: a
/// space before a capital that follows a lower-case letter, or that starts a word
/// after a run of capitals.
fn spaced(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || (prev.is_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(c);
    }
    out
}

/// `"{0.5, 1}"` as a point.
fn pt(s: &str) -> Point {
    let inner = s.trim().trim_start_matches('{').trim_end_matches('}');
    let mut it = inner
        .split(',')
        .map(|n| n.trim().parse::<f64>().unwrap_or(0.0));
    Point::new(it.next().unwrap_or(0.0), it.next().unwrap_or(0.0))
}

fn num_in(v: &Value, path: &[&str]) -> f64 {
    str_in_value(v, path).and_then(Value::as_f64).unwrap_or(0.0)
}

fn b(v: &Value, k: &str) -> bool {
    b_of(v, k)
}

/// A layer's Sketch resizing constraint as insets inside a box of `outer`.
/// Sketch clears a bit for each thing pinned: right 1, width 2, left 4,
/// bottom 8, height 16, top 32. A pinned edge is a px inset; a free size between
/// free edges scales, as percentages; a pinned size between free edges keeps its
/// offset from the centre, centred by auto margins between insets that put it
/// where it is.
fn insets_of(layer: &Value, outer: Size) -> Option<Insets> {
    let c = layer.get("resizingConstraint").and_then(Value::as_i64)?;
    if !(1..=63).contains(&c) {
        return None;
    }
    let pinned = |bit: i64| c & bit == 0;
    let f = Frame::of(layer);
    let axis = |start: bool, size: bool, end: bool, at: f64, len: f64, extent: f64| {
        let after = extent - at - len;
        match (start, end) {
            (true, true) => (Some(LengthPct::Px(at)), Some(LengthPct::Px(after)), false),
            (true, false) => (Some(LengthPct::Px(at)), None, false),
            (false, true) => (None, Some(LengthPct::Px(after)), false),
            _ if !size && extent > 0.0 => (
                Some(LengthPct::Percent(at / extent * 100.0)),
                Some(LengthPct::Percent(after / extent * 100.0)),
                false,
            ),
            _ => {
                let d = extent - 2.0 * at - len;
                let (a, b) = if d >= 0.0 { (0.0, d) } else { (-d, 0.0) };
                (Some(LengthPct::Px(a)), Some(LengthPct::Px(b)), true)
            }
        }
    };
    let (left, right, h) = axis(pinned(4), pinned(2), pinned(1), f.x, f.w, outer.width);
    let (top, bottom, v) = axis(pinned(32), pinned(16), pinned(8), f.y, f.h, outer.height);
    Some(Insets {
        top,
        right,
        bottom,
        left,
        margin_auto: AutoMargins {
            top: v,
            bottom: v,
            left: h,
            right: h,
        },
    })
}

/// Whether a layer has a fill or border turned on.
fn paints(layer: &Value) -> bool {
    ["fills", "borders"].iter().any(|k| {
        arr_in(layer, &["style", k])
            .iter()
            .any(|p| b_of(p, "isEnabled"))
    })
}

/// Whether a mask layer masks by its paint's alpha (`clippingMaskMode` 1)
/// rather than by its outline.
fn alpha_mask(layer: &Value) -> bool {
    num(layer, "clippingMaskMode") as i64 == 1
}

fn str_in<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    str_in_value(v, path).and_then(Value::as_str).unwrap_or("")
}

fn str_in_value<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, k| v.get(k))
}

fn arr_in<'a>(v: &'a Value, path: &[&str]) -> &'a [Value] {
    str_in_value(v, path)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Files(BTreeMap<String, Vec<u8>>);

    impl Archive for Files {
        fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
            self.0.get(name).cloned()
        }
    }

    struct NoPictures;

    impl ImageSink for NoPictures {
        fn place(
            &mut self,
            _: Vec<u8>,
            _: crate::image::ImageFormat,
        ) -> Option<(ImageId, crate::image::ImageEntry)> {
            None
        }
    }

    /// Masters each holding two instances of the one before double per level;
    /// past `MAX_NODES` the rest stay empty groups and are reported, and the
    /// read returns. flip: return `ready` unchanged from `within_budget` → the
    /// read runs on towards 2²⁴ copies.
    #[test]
    fn symbols_that_double_stop_at_the_node_budget() {
        let n = 24;
        let frame = json!({ "_class": "rect", "x": 0, "y": 0, "width": 10, "height": 10 });
        let mut layers = vec![json!({
            "_class": "symbolMaster", "do_objectID": "M0", "name": "S0", "symbolID": "S0",
            "frame": frame,
            "layers": [{ "_class": "oval", "do_objectID": "O", "name": "Dot", "frame": frame,
                         "isClosed": true, "points": [] }],
        })];
        for k in 1..=n {
            let inst = |i: usize| {
                json!({ "_class": "symbolInstance", "do_objectID": format!("I{k}-{i}"),
                        "name": "i", "symbolID": format!("S{}", k - 1), "frame": frame })
            };
            layers.push(json!({
                "_class": "symbolMaster", "do_objectID": format!("M{k}"),
                "name": format!("S{k}"), "symbolID": format!("S{k}"), "frame": frame,
                "layers": [inst(0), inst(1)],
            }));
        }
        layers.push(
            json!({ "_class": "symbolInstance", "do_objectID": "TOP", "name": "Top",
                            "symbolID": format!("S{n}"), "frame": frame }),
        );
        let page =
            json!({ "_class": "page", "do_objectID": "P", "name": "Page", "layers": layers });
        let document = json!({ "_class": "document", "pages": [{ "_ref": "pages/P" }] });
        let mut files = Files(BTreeMap::from([
            (
                "document.json".to_owned(),
                document.to_string().into_bytes(),
            ),
            ("pages/P.json".to_owned(), page.to_string().into_bytes()),
        ]));
        let started = std::time::Instant::now();
        let r = read(&mut files, &mut IdSource::new(3), &mut NoPictures).expect("the file reads");
        let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
        assert!(rows.contains(&"Symbols too large to place"), "{rows:?}");
        // The copies within the budget, the file's own few dozen layers on top.
        assert!(r.doc.len() <= MAX_NODES + 4 * (n + 1), "{}", r.doc.len());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "{:?}",
            started.elapsed()
        );
    }

    /// A file whose own layers already pass `MAX_NODES` still places its
    /// instances: the budget is for what the copies add. flip: start
    /// `within_budget`'s total at the whole document's length → fails at the
    /// report row.
    #[test]
    fn a_large_file_still_places_its_symbols() {
        let frame = json!({ "_class": "rect", "x": 0, "y": 0, "width": 10, "height": 10 });
        let mut layers: Vec<Value> = (0..MAX_NODES)
            .map(|i| {
                json!({ "_class": "rectangle", "do_objectID": format!("R{i}"), "name": "r",
                        "frame": frame, "isClosed": true, "points": [] })
            })
            .collect();
        layers.push(json!({
            "_class": "symbolMaster", "do_objectID": "M", "name": "S", "symbolID": "S",
            "frame": frame,
            "layers": [{ "_class": "oval", "do_objectID": "O", "name": "Dot", "frame": frame,
                         "isClosed": true, "points": [] }],
        }));
        layers.push(
            json!({ "_class": "symbolInstance", "do_objectID": "I", "name": "Placed",
                            "symbolID": "S", "frame": frame }),
        );
        let page =
            json!({ "_class": "page", "do_objectID": "P", "name": "Page", "layers": layers });
        let document = json!({ "_class": "document", "pages": [{ "_ref": "pages/P" }] });
        let mut files = Files(BTreeMap::from([
            (
                "document.json".to_owned(),
                document.to_string().into_bytes(),
            ),
            ("pages/P.json".to_owned(), page.to_string().into_bytes()),
        ]));
        let r = read(&mut files, &mut IdSource::new(3), &mut NoPictures).expect("the file reads");
        assert!(
            r.doc.len() > MAX_NODES,
            "the file's own layers pass the budget"
        );
        let rows: Vec<&str> = r.report.iter().map(|u| u.what).collect();
        assert!(!rows.contains(&"Symbols too large to place"), "{rows:?}");
        let nodes = r.doc.node_map();
        let placed = nodes
            .values()
            .find(|n| n.name() == "Placed")
            .expect("the instance");
        assert_eq!(placed.children().len(), 1, "a copy of the master's dot");
    }
}
