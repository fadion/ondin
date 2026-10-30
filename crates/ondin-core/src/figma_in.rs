//! Figma import: a file read through Figma's REST API into a new document — its
//! pages, frames, shapes, text, components and styles as Ondin's own.
//!
//! The app fetches; this reads what it fetched, through [`Archive`]:
//! `file.json` is `GET /v1/files/:key?geometry=paths`, and `images/<ref>` each
//! picture an image fill names. Ondin never reads Figma's `.fig` binary.
//!
//! A node's `relativeTransform` places it in its parent — a group or a boolean
//! included, unlike the Plugin API, which places their children in the frame
//! holding them — so each node's place on the page is its parent's times its own.
//!
//! An instance's nodes carry ids `I<instance>;<source>`, the source being the
//! node of the component it copies; that is Ondin's link. An instance whose every
//! node finds its source is linked whole; any other is kept as plain layers.

use crate::document::Document;
use crate::effect::{Effect, EffectKind, Shadow};
use crate::export::{ExportFormat, ExportScale, ExportSpec};
use crate::foreign::{
    Archive, Names, Tally, Unmapped, arr, b_of, num, sniff, str_of, utf16_to_bytes,
};
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
    CharAttr, CharSpans, Decoration, Length, ParaSpans, ParagraphStyle, TextAlign, TextCase,
    TextStyle,
};
use crate::variable::BrushSource;
use crate::{BlockStyle, TextSizing, VerticalAlign};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::Value;
use std::collections::BTreeMap;

/// A Figma file read into a document.
pub struct Read {
    pub doc: Document,
    /// The file's own name.
    pub name: String,
    /// What did not come in as it was, one row per kind, in a stable order.
    pub report: Vec<Unmapped>,
    /// Every font family the text asks for, with how many runs ask for it.
    pub families: BTreeMap<String, usize>,
}

/// Why a fetched file could not be read at all.
#[derive(Debug, thiserror::Error)]
pub enum FigmaError {
    #[error("Figma sent no file")]
    NoFile,
    #[error("Figma sent something other than a file: {0}")]
    Json(String),
    #[error("the file has no pages")]
    NoPages,
    #[error("the document did not build: {0}")]
    Build(#[from] OpError),
}

/// The file key in a `figma.com` link — `/design/<key>/…`, `/file/<key>/…`, or a
/// branch's `/design/<key>/branch/<branch key>/…`, which is read as the branch.
pub fn file_key(link: &str) -> Option<String> {
    let rest = link.trim();
    let rest = rest
        .strip_prefix("https://")
        .or_else(|| rest.strip_prefix("http://"))
        .unwrap_or(rest);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let path = rest.strip_prefix("figma.com/")?;
    let mut parts = path.split(['/', '?', '#']);
    let kind = parts.next()?;
    if !matches!(kind, "design" | "file" | "proto" | "board") {
        return None;
    }
    let key = parts.next()?;
    let key = match (parts.next(), parts.next()) {
        (Some("branch"), Some(branch)) => branch,
        _ => key,
    };
    let valid = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric());
    valid.then(|| key.to_owned())
}

/// Read a fetched Figma file into a new document, minting from `ids` and handing
/// pictures to `images`.
pub fn read(
    archive: &mut dyn Archive,
    ids: &mut IdSource,
    images: &mut dyn ImageSink,
) -> Result<Read, FigmaError> {
    let bytes = archive.entry("file.json").ok_or(FigmaError::NoFile)?;
    let file: Value =
        serde_json::from_slice(&bytes).map_err(|e| FigmaError::Json(e.to_string()))?;
    let pages = file
        .get("document")
        .map(|d| arr(d, "children").to_vec())
        .unwrap_or_default();
    if pages.is_empty() {
        return Err(FigmaError::NoPages);
    }
    let first = ids.mint();
    let mut b = Builder {
        doc: Document::new(first),
        ids,
        archive,
        images,
        ops: Vec::new(),
        nodes: FxHashMap::default(),
        mains: FxHashSet::default(),
        instances: Vec::new(),
        style_names: file
            .get("styles")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(id, s)| (id.clone(), str_of(s, "name").to_owned()))
                    .collect()
            })
            .unwrap_or_default(),
        styles: FxHashMap::default(),
        pictures: FxHashMap::default(),
        tally: Tally::default(),
        taken: Names::default(),
        families: BTreeMap::new(),
        pending_type: None,
    };
    for (i, page) in pages.iter().enumerate() {
        let id = if i == 0 { first } else { b.ids.mint() };
        let name = str_of(page, "name").to_owned();
        if i == 0 {
            b.push(Operation::SetName { id, name });
        } else {
            b.push(Operation::AddPage {
                id,
                index: i,
                name,
                ground: crate::DEFAULT_CANVAS_BACKGROUND,
                draft: false,
                mode: None,
            });
        }
        if let Some(bg) = page.get("backgroundColor") {
            b.push(Operation::SetPageGround {
                id,
                ground: colour(bg).with_alpha(1.0),
            });
        }
        for (j, child) in arr(page, "children").iter().enumerate() {
            b.node(child, id, j, Affine::IDENTITY, None);
        }
    }
    let ops = std::mem::take(&mut b.ops);
    b.commit(ops)?;
    b.components()?;
    let Builder {
        doc,
        tally,
        families,
        ..
    } = b;
    Ok(Read {
        doc,
        name: str_of(&file, "name").to_owned(),
        report: tally.into_report(),
        families,
    })
}

/// An instance: its root's node and the component it shows, and each of its
/// nodes with the Figma id of the node it copies.
struct Instance {
    root: NodeId,
    component: String,
    copies: Vec<(NodeId, String)>,
    /// A copy inside another instance: its root is linked as that instance's
    /// copy, to the nested instance it copies, never straight to its main.
    nested: bool,
}

/// A style, made from the first node that uses it, with the values a later user
/// must still show to be bound to it.
enum Made {
    Fills(StyleId, Vec<Fill>),
    Strokes(StyleId, Vec<Stroke>),
    Effects(StyleId, Vec<Effect>),
    Text(StyleId, Box<TextStyle>),
}

struct Builder<'a> {
    doc: Document,
    ids: &'a mut IdSource,
    archive: &'a mut dyn Archive,
    images: &'a mut dyn ImageSink,
    ops: Vec<Operation>,
    /// Figma node id → the node made for it.
    nodes: FxHashMap<String, NodeId>,
    mains: FxHashSet<String>,
    instances: Vec<Instance>,
    style_names: FxHashMap<String, String>,
    styles: FxHashMap<String, Made>,
    pictures: FxHashMap<String, Option<ImageId>>,
    tally: Tally,
    /// Style names given out, by slot.
    taken: Names<StyleSlot>,
    families: BTreeMap<String, usize>,
    /// The text style the text node about to be made is bound to, found while
    /// its kind was read and written once the node exists.
    pending_type: Option<StyleId>,
}

impl Builder<'_> {
    fn push(&mut self, op: Operation) {
        self.ops.push(op);
    }

    fn count(&mut self, what: &'static str, n: usize, unit: &'static str, how: &'static str) {
        self.tally.count(what, n, unit, how);
    }

    fn commit(&mut self, ops: Vec<Operation>) -> Result<(), OpError> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut tx = Transaction(ops);
        crate::propagate::owed(&self.doc, &mut tx, self.ids);
        self.doc.apply(&tx).map(|_| ())
    }

    /// One node and what it holds. `parent_world` is the place on the page of the
    /// node it hangs under, and `inst` the innermost instance it sits in, by index.
    fn node(
        &mut self,
        n: &Value,
        parent: NodeId,
        index: usize,
        parent_world: Affine,
        inst: Option<usize>,
    ) {
        let kind = str_of(n, "type");
        self.notes(n);
        let size = size_of(n);
        let world = world_of(n, parent_world);
        let id = self.ids.mint();
        let made = match kind {
            "FRAME" | "COMPONENT" | "COMPONENT_SET" | "INSTANCE" | "SECTION" => {
                NodeKind::Artboard {
                    size,
                    corner_radii: radii(n),
                    corner_smoothing: num(n, "cornerSmoothing"),
                }
            }
            "GROUP" => NodeKind::Group,
            "BOOLEAN_OPERATION" => NodeKind::Boolean {
                op: match str_of(n, "booleanOperation") {
                    "SUBTRACT" => BoolOp::Subtract,
                    "INTERSECT" => BoolOp::Intersect,
                    "EXCLUDE" => BoolOp::Exclude,
                    _ => BoolOp::Union,
                },
            },
            "RECTANGLE" => NodeKind::Rect {
                size,
                corner_radii: radii(n),
                corner_smoothing: num(n, "cornerSmoothing"),
            },
            "ELLIPSE" => NodeKind::Ellipse { size, arc: arc(n) },
            "LINE" => NodeKind::Line {
                end: Point::new(size.width, 0.0),
            },
            "VECTOR" | "STAR" | "REGULAR_POLYGON" => match geometry(n, "fillGeometry")
                // Nothing to fill: an open path, or a closed one without fills,
                // drawn from its network so its stroke stays a stroke.
                .or_else(|| network(n))
            {
                Some(path) => NodeKind::Path {
                    path,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                },
                None => match geometry(n, "strokeGeometry") {
                    // Without a network, its stroke outlined, drawn as a shape.
                    Some(path) => {
                        self.count("Open paths", 1, "", "Drawn as their outlined strokes");
                        return self.outlined(n, path, parent, index, world, parent_world);
                    }
                    None => {
                        self.count("Shapes without geometry", 1, "", "Left out");
                        return;
                    }
                },
            },
            "TEXT" => self.text(n),
            "SLICE" => NodeKind::Slice { size },
            _ => {
                self.count("Layers of a kind Ondin does not have", 1, "", "Left out");
                return;
            }
        };
        self.push(Operation::CreateNode {
            id,
            parent,
            index,
            kind: made,
            transform: Some(parent_world.inverse() * world),
            name: Some(str_of(n, "name").to_owned()),
        });
        let fid = str_of(n, "id").to_owned();
        self.nodes.insert(fid.clone(), id);
        self.flags(id, n, kind);
        self.paint(id, n, kind, size);
        if let Some(style) = self.pending_type.take() {
            self.push(Operation::SetTypeStyle {
                id,
                style: Some(style),
            });
        }
        match kind {
            "COMPONENT" => {
                self.mains.insert(fid.clone());
            }
            "COMPONENT_SET" => self.count(
                "Variant sets",
                1,
                "",
                "Each variant came in as its own component",
            ),
            "INSTANCE" => self.instances.push(Instance {
                root: id,
                component: str_of(n, "componentId").to_owned(),
                copies: Vec::new(),
                nested: inst.is_some() && source_of(&fid).is_some(),
            }),
            _ => {}
        }
        // A copy belongs to the innermost instance it sits in, which is this one
        // for its own children.
        let inst = if kind == "INSTANCE" {
            Some(self.instances.len() - 1)
        } else {
            inst
        };
        for (i, child) in arr(n, "children").iter().enumerate() {
            self.node(child, id, i, world, inst);
            let cid = str_of(child, "id");
            if let (Some(at), Some(&made), Some(source)) =
                (inst, self.nodes.get(cid), source_of(cid))
            {
                self.instances[at].copies.push((made, source));
            }
        }
    }

    /// An open path drawn as its outlined stroke, the stroke's paints as fills.
    fn outlined(
        &mut self,
        n: &Value,
        path: BezPath,
        parent: NodeId,
        index: usize,
        world: Affine,
        parent_world: Affine,
    ) {
        let id = self.ids.mint();
        self.push(Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Path {
                path,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
            },
            transform: Some(parent_world.inverse() * world),
            name: Some(str_of(n, "name").to_owned()),
        });
        self.nodes.insert(str_of(n, "id").to_owned(), id);
        self.flags(id, n, "VECTOR");
        let size = size_of(n);
        let fills = self.paints(arr(n, "strokes"), size);
        if !fills.is_empty() {
            self.push(Operation::SetFills {
                id,
                fills: keyed_by_position(fills),
            });
        }
        let effects = self.effects(n);
        if !effects.is_empty() {
            self.push(Operation::SetEffects {
                id,
                effects: keyed_by_position(effects),
            });
        }
    }

    /// Count what a node carries that has no counterpart.
    fn notes(&mut self, n: &Value) {
        if n.get("transitionNodeID").is_some_and(|t| !t.is_null())
            || !arr(n, "interactions").is_empty()
        {
            self.count("Prototype interactions", 1, "", "Left out");
        }
        if matches!(str_of(n, "layoutMode"), "HORIZONTAL" | "VERTICAL" | "GRID") {
            self.count("Auto layout", 1, "frames", "Kept at their drawn positions");
        }
        // Once per layer, whether the node or one of its paints is bound.
        let bound = |v: &Value| {
            v.get("boundVariables")
                .and_then(Value::as_object)
                .is_some_and(|m| !m.is_empty())
        };
        if bound(n) || arr(n, "fills").iter().chain(arr(n, "strokes")).any(bound) {
            self.count(
                "Variables",
                1,
                "layers",
                "Need Figma's Enterprise plan to be read: the values they show were kept",
            );
        }
    }

    /// Visibility, lock, mask, clipping, opacity, blend and export settings.
    fn flags(&mut self, id: NodeId, n: &Value, kind: &str) {
        if n.get("visible").and_then(Value::as_bool) == Some(false) {
            self.push(Operation::SetVisible { id, visible: false });
        }
        if b_of(n, "locked") {
            self.push(Operation::SetLocked { id, locked: true });
        }
        let frame = matches!(
            kind,
            "FRAME" | "COMPONENT" | "COMPONENT_SET" | "INSTANCE" | "SECTION"
        );
        if b_of(n, "isMask") {
            if frame {
                self.count("Frames used as masks", 1, "", "Left unmasked");
            } else {
                self.push(Operation::SetMask { id, mask: true });
            }
        }
        if frame {
            self.push(Operation::SetClip {
                id,
                clip: kind != "SECTION" && b_of(n, "clipsContent"),
            });
        }
        let opacity = n.get("opacity").and_then(Value::as_f64).unwrap_or(1.0);
        if opacity < 1.0 {
            self.push(Operation::SetOpacity {
                id,
                opacity: opacity as f32,
            });
        }
        if let Some(blend) = blend_mode(str_of(n, "blendMode")) {
            self.push(Operation::SetBlend { id, blend });
        }
        self.exports(id, n);
    }

    /// A node's `exportSettings` as its export settings. A suffix that is only
    /// the scale's own marker (`@2x`) is left for Ondin to add.
    fn exports(&mut self, id: NodeId, n: &Value) {
        let mut exports = Vec::new();
        for e in arr(n, "exportSettings") {
            let format = match str_of(e, "format") {
                "PNG" => ExportFormat::Png,
                "JPG" => ExportFormat::Jpeg,
                "SVG" => ExportFormat::Svg,
                "PDF" => ExportFormat::Pdf,
                _ => {
                    self.count("Export formats Ondin lacks", 1, "", "Left out");
                    continue;
                }
            };
            let c = e.get("constraint").unwrap_or(&Value::Null);
            let value = num(c, "value");
            let scale = match str_of(c, "type") {
                "WIDTH" if value >= 1.0 => ExportScale::Width(value.round() as u32),
                "HEIGHT" if value >= 1.0 => ExportScale::Height(value.round() as u32),
                _ if value.is_finite() && value > 0.0 => ExportScale::Times(value as f32),
                _ => ExportScale::Times(1.0),
            };
            let mut spec = ExportSpec::new(format, scale);
            let suffix = str_of(e, "suffix");
            if suffix != scale.marker() {
                spec.suffix = suffix.to_owned();
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

    /// Fills, strokes and effects, bound to the node's styles where it still
    /// shows them.
    fn paint(&mut self, id: NodeId, n: &Value, kind: &str, size: Size) {
        // A path drawn from its network has no fill in Figma, whatever it lists.
        let vector = matches!(kind, "VECTOR" | "STAR" | "REGULAR_POLYGON");
        if kind != "LINE" && !(vector && arr(n, "fillGeometry").is_empty()) {
            let fills = self.paints(arr(n, "fills"), size);
            if !fills.is_empty() {
                self.push(Operation::SetFills {
                    id,
                    fills: keyed_by_position(fills.clone()),
                });
            }
            if let Some(s) = self.style_for(
                n,
                "fill",
                || StyleValue::Fills(keyed_by_position(fills.clone())),
                |m| matches!(m, Made::Fills(_, v) if *v == fills),
            ) {
                self.push(Operation::SetFillStyle { id, style: Some(s) });
            }
            if str_in(n, "fillGeometry").is_some_and(|g| str_of(g, "windingRule") == "EVENODD") {
                self.push(Operation::SetFillRule {
                    id,
                    rule: FillRule::EvenOdd,
                });
            }
        }
        let strokes = self.strokes(n, size);
        if !strokes.is_empty() {
            self.push(Operation::SetStrokes {
                id,
                strokes: keyed_by_position(strokes.clone()),
            });
        }
        if let Some(s) = self.style_for(
            n,
            "stroke",
            || StyleValue::Strokes(keyed_by_position(strokes.clone())),
            |m| matches!(m, Made::Strokes(_, v) if *v == strokes),
        ) {
            self.push(Operation::SetStrokeStyle { id, style: Some(s) });
        }
        let effects = self.effects(n);
        if !effects.is_empty() {
            self.push(Operation::SetEffects {
                id,
                effects: keyed_by_position(effects.clone()),
            });
        }
        if let Some(s) = self.style_for(
            n,
            "effect",
            || StyleValue::Effects(keyed_by_position(effects.clone())),
            |m| matches!(m, Made::Effects(_, v) if *v == effects),
        ) {
            self.push(Operation::SetEffectsStyle { id, style: Some(s) });
        }
    }

    /// The style the node names for `slot`: made from this node's values the
    /// first time it is met, and answered for a later user only while `matches`.
    fn style_for(
        &mut self,
        n: &Value,
        slot: &str,
        value: impl FnOnce() -> StyleValue,
        matches: impl Fn(&Made) -> bool,
    ) -> Option<StyleId> {
        let styles = n.get("styles")?;
        let sid = styles
            .get(slot)
            .or_else(|| styles.get(format!("{slot}s")))
            .and_then(Value::as_str)?
            .to_owned();
        if let Some(made) = self.styles.get(&sid) {
            return matches(made).then(|| made_id(made));
        }
        let id = self.ids.mint_style();
        let value = value();
        let made = match &value {
            StyleValue::Fills(v) => Made::Fills(id, v.iter().map(|k| k.value.clone()).collect()),
            StyleValue::Strokes(v) => {
                Made::Strokes(id, v.iter().map(|k| k.value.clone()).collect())
            }
            StyleValue::Effects(v) => {
                Made::Effects(id, v.iter().map(|k| k.value.clone()).collect())
            }
            StyleValue::Text(t) => Made::Text(id, t.clone()),
            StyleValue::Layout(_) => return None,
        };
        let named = self.style_names.get(&sid).unwrap_or(&sid);
        let name = self
            .taken
            .unique(value.slot(), named, "Style", &mut self.tally);
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
        self.styles.insert(sid, made);
        Some(id)
    }

    fn paints(&mut self, paints: &[Value], size: Size) -> Vec<Fill> {
        paints
            .iter()
            .filter_map(|p| {
                Some(Fill {
                    brush: self.brush(p, size)?,
                    visible: p.get("visible").and_then(Value::as_bool) != Some(false),
                })
            })
            .collect()
    }

    /// A paint: a colour, a gradient in the box of `size`, or a picture.
    fn brush(&mut self, p: &Value, size: Size) -> Option<BrushSource> {
        let opacity = p.get("opacity").and_then(Value::as_f64).unwrap_or(1.0) as f32;
        let kind = str_of(p, "type");
        match kind {
            "SOLID" => Some(
                peniko::Brush::Solid(
                    p.get("color")
                        .map_or(Color::BLACK, colour)
                        .multiply_alpha(opacity),
                )
                .into(),
            ),
            "GRADIENT_LINEAR" | "GRADIENT_RADIAL" | "GRADIENT_ANGULAR" | "GRADIENT_DIAMOND" => {
                if kind == "GRADIENT_DIAMOND" {
                    self.count("Diamond gradients", 1, "", "Read as radial");
                }
                Some(gradient(p, kind, size, opacity).into())
            }
            "IMAGE" => {
                let Some(image) = self.picture(str_of(p, "imageRef")) else {
                    self.count("Pictures", 1, "", "Could not be fetched");
                    return None;
                };
                let mut brush = image_brush(image);
                if let peniko::Brush::Image(b) = &mut brush {
                    b.image.fit = match str_of(p, "scaleMode") {
                        "FIT" => ImageFit::Fit,
                        "TILE" => ImageFit::Tile,
                        _ => ImageFit::Fill,
                    };
                }
                Some(brush.into())
            }
            _ => {
                self.count("Video and emoji fills", 1, "", "Left out");
                None
            }
        }
    }

    fn picture(&mut self, reference: &str) -> Option<ImageId> {
        if reference.is_empty() {
            return None;
        }
        if let Some(known) = self.pictures.get(reference) {
            return known.clone();
        }
        let id = self
            .archive
            .entry(&format!("images/{reference}"))
            .and_then(|bytes| {
                let format = sniff(&bytes)?;
                let (id, entry) = self.images.place(bytes, format)?;
                if !self.doc.has_image(&id)
                    && !self.ops.iter().any(
                        |op| matches!(op, Operation::AddImage { id: added, .. } if *added == id),
                    )
                {
                    self.ops.push(Operation::AddImage {
                        id: id.clone(),
                        entry,
                    });
                }
                Some(id)
            });
        self.pictures.insert(reference.to_owned(), id.clone());
        id
    }

    fn strokes(&mut self, n: &Value, size: Size) -> Vec<Stroke> {
        let width = n.get("strokeWeight").and_then(Value::as_f64).unwrap_or(1.0);
        if n.get("individualStrokeWeights")
            .is_some_and(Value::is_object)
        {
            self.count(
                "Per-side stroke weights",
                1,
                "",
                "Drawn at the stroke's weight on every side",
            );
        }
        let cap = match str_of(n, "strokeCap") {
            "ROUND" => crate::kurbo::Cap::Round,
            "SQUARE" => crate::kurbo::Cap::Square,
            "" | "NONE" => crate::kurbo::Cap::Butt,
            _ => {
                self.count("Arrow and marker caps", 1, "strokes", "Drawn without them");
                crate::kurbo::Cap::Butt
            }
        };
        let join = match str_of(n, "strokeJoin") {
            "ROUND" => crate::kurbo::Join::Round,
            "BEVEL" => crate::kurbo::Join::Bevel,
            _ => crate::kurbo::Join::Miter,
        };
        let miter = n
            .get("strokeMiterAngle")
            .and_then(Value::as_f64)
            .map_or(4.0, crate::node::miter_ratio_of_degrees);
        let dashes: Vec<f64> = arr(n, "strokeDashes")
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        let align = match str_of(n, "strokeAlign") {
            "INSIDE" => StrokeAlign::Inside,
            "OUTSIDE" => StrokeAlign::Outside,
            _ => StrokeAlign::Center,
        };
        arr(n, "strokes")
            .to_vec()
            .iter()
            .filter_map(|p| {
                Some(Stroke {
                    brush: self.brush(p, size)?,
                    width,
                    width_var: None,
                    join,
                    cap,
                    dashes: dashes.clone(),
                    dash_offset: 0.0,
                    miter_limit: miter,
                    dash_fit: false,
                    align,
                    sides: crate::StrokeSides::All,
                    visible: p.get("visible").and_then(Value::as_bool) != Some(false),
                })
            })
            .collect()
    }

    fn effects(&mut self, n: &Value) -> Vec<Effect> {
        let mut out = Vec::new();
        for e in arr(n, "effects") {
            let visible = e.get("visible").and_then(Value::as_bool) != Some(false);
            let shadow = || Shadow {
                offset: e
                    .get("offset")
                    .map_or(Vec2::ZERO, |o| Vec2::new(num(o, "x"), num(o, "y"))),
                blur: num(e, "radius"),
                spread: num(e, "spread"),
                color: e.get("color").map_or(Color::BLACK, colour),
                blend: BlendMode::Normal,
            };
            let kind = match str_of(e, "type") {
                "DROP_SHADOW" => EffectKind::DropShadow(shadow()),
                "INNER_SHADOW" => EffectKind::InnerShadow(shadow()),
                "LAYER_BLUR" => EffectKind::LayerBlur {
                    radius: num(e, "radius"),
                },
                "BACKGROUND_BLUR" => {
                    self.count("Background blur", 1, "layers", "Left out");
                    continue;
                }
                _ => {
                    self.count("Effects Ondin does not have", 1, "", "Left out");
                    continue;
                }
            };
            out.push(Effect {
                visible,
                ..Effect::new(kind)
            });
        }
        out
    }

    /// A text node: its characters, its style as the default and each run of
    /// overrides as spans.
    fn text(&mut self, n: &Value) -> NodeKind {
        let content = str_of(n, "characters").to_owned();
        let base = n.get("style").cloned().unwrap_or(Value::Null);
        let default = type_style(&base, &TextStyle::default());
        let ink = arr(n, "fills").first().and_then(|f| {
            (str_of(f, "type") == "SOLID").then(|| {
                f.get("color")
                    .map_or(Color::BLACK, colour)
                    .multiply_alpha(f.get("opacity").and_then(Value::as_f64).unwrap_or(1.0) as f32)
            })
        });
        let overrides: Vec<i64> = arr(n, "characterStyleOverrides")
            .iter()
            .filter_map(Value::as_i64)
            .collect();
        let table = n.get("styleOverrideTable");
        let bytes = utf16_to_bytes(&content);
        let mut spans = CharSpans::default();
        let mut runs = vec![(0usize, content.encode_utf16().count(), 0i64)];
        if !overrides.is_empty() {
            runs.clear();
            let mut start = 0;
            for i in 1..=overrides.len() {
                if i == overrides.len() || overrides[i] != overrides[start] {
                    runs.push((start, i, overrides[start]));
                    start = i;
                }
            }
        }
        for (start, end, key) in runs {
            let over = table.and_then(|t| t.get(key.to_string()));
            let style = over.map_or_else(|| default.clone(), |o| type_style(o, &default));
            *self.families.entry(style.font_family.clone()).or_default() += 1;
            let range = bytes(start)..bytes(end);
            for attr in [
                CharAttr::Family(style.font_family.clone()),
                CharAttr::Size(style.font_size),
                CharAttr::Weight(style.weight),
                CharAttr::Italic(style.italic),
                CharAttr::LetterSpacing(style.letter_spacing),
                CharAttr::LineHeight(style.line_height),
                CharAttr::Underline(style.underline),
                CharAttr::Strikethrough(style.strikethrough),
                CharAttr::Case(style.case),
            ] {
                spans.set(range.clone(), attr, &default);
            }
            let run_ink = over
                .and_then(|o| arr(o, "fills").first())
                .and_then(|f| f.get("color"))
                .map(colour)
                .filter(|c| Some(*c) != ink);
            spans.set(range, CharAttr::Color(run_ink), &default);
        }
        let size = size_of(n);
        let typed = crate::style::type_part(&default);
        if let Some(s) = self.style_for(
            n,
            "text",
            || StyleValue::Text(Box::new(typed.clone())),
            |m| matches!(m, Made::Text(_, t) if **t == typed),
        ) {
            self.pending_type = Some(s);
        }
        NodeKind::Text {
            content,
            style: Box::new(default),
            spans,
            para_spans: ParaSpans::default(),
            paragraph: ParagraphStyle {
                align: match str_of(&base, "textAlignHorizontal") {
                    "CENTER" => TextAlign::Center,
                    "RIGHT" => TextAlign::End,
                    "JUSTIFIED" => TextAlign::Justify,
                    _ => TextAlign::Start,
                },
                ..ParagraphStyle::default()
            },
            block: BlockStyle {
                vertical_align: match str_of(&base, "textAlignVertical") {
                    "CENTER" => VerticalAlign::Middle,
                    "BOTTOM" => VerticalAlign::Bottom,
                    _ => VerticalAlign::Top,
                },
                ..BlockStyle::default()
            },
            sizing: match str_of(&base, "textAutoResize") {
                "WIDTH_AND_HEIGHT" => TextSizing::Auto,
                "HEIGHT" => TextSizing::AutoHeight(size.width),
                _ => TextSizing::Fixed(size),
            },
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    /// Components become mains; each instance whose every node finds the node it
    /// copies is linked to it, node by node, in one transaction.
    fn components(&mut self) -> Result<(), FigmaError> {
        let mut ops: Vec<Operation> = self
            .mains
            .iter()
            .filter_map(|m| self.nodes.get(m))
            .map(|&id| Operation::SetComponent {
                id,
                component: true,
            })
            .collect();
        let (mut linked, mut missing, mut unmatched) = (0, 0, 0);
        for inst in &self.instances {
            let main = self
                .nodes
                .get(&inst.component)
                .filter(|_| self.mains.contains(&inst.component));
            let copies: Option<Vec<(NodeId, NodeId)>> = inst
                .copies
                .iter()
                .map(|(id, source)| self.nodes.get(source).map(|&s| (*id, s)))
                .collect();
            match (main, copies) {
                (Some(&main), Some(copies)) => {
                    linked += 1;
                    if !inst.nested {
                        ops.push(Operation::SetLink {
                            id: inst.root,
                            link: Some(main),
                        });
                    }
                    ops.extend(
                        copies
                            .into_iter()
                            .map(|(id, s)| Operation::SetLink { id, link: Some(s) }),
                    );
                }
                (None, _) => missing += 1,
                (Some(_), None) => unmatched += 1,
            }
        }
        if missing > 0 {
            self.count(
                "Instances of components not in the file",
                missing,
                "",
                "Kept as plain layers",
            );
        }
        if unmatched > 0 {
            self.count(
                "Instances whose layers are not their component's",
                unmatched,
                "",
                "Kept as plain layers",
            );
        }
        // Linked as a whole or not at all: a rule the commit refuses is instances
        // kept plain, not a failed import.
        if self.commit(ops.clone()).is_err() {
            let mains: Vec<Operation> = ops
                .into_iter()
                .filter(|op| matches!(op, Operation::SetComponent { .. }))
                .collect();
            self.commit(mains)?;
            self.count(
                "Instances that did not match their component",
                linked,
                "",
                "Kept as plain layers",
            );
        }
        Ok(())
    }
}

fn made_id(m: &Made) -> StyleId {
    match m {
        Made::Fills(id, _) | Made::Strokes(id, _) | Made::Effects(id, _) | Made::Text(id, _) => *id,
    }
}

/// The node an instance's copy `I<instance>;<rest>` copies: `rest`, itself
/// prefixed with `I` when it names a copy inside a nested instance.
fn source_of(id: &str) -> Option<String> {
    let rest = id.strip_prefix('I')?.split_once(';')?.1;
    Some(if rest.contains(';') {
        format!("I{rest}")
    } else {
        rest.to_owned()
    })
}

fn str_in<'a>(n: &'a Value, k: &str) -> Option<&'a Value> {
    arr(n, k).first()
}

/// A node's own size, its bounding box's where it has none.
fn size_of(n: &Value) -> Size {
    if let Some(s) = n.get("size") {
        return Size::new(num(s, "x").max(0.0), num(s, "y").max(0.0));
    }
    n.get("absoluteBoundingBox").map_or(Size::ZERO, |b| {
        Size::new(num(b, "width").max(0.0), num(b, "height").max(0.0))
    })
}

/// Where a node sits on the page: its `relativeTransform` in `parent_world`,
/// else its bounding box's corner, which is already on the page.
fn world_of(n: &Value, parent_world: Affine) -> Affine {
    let rows = arr(n, "relativeTransform");
    if let [r0, r1] = rows {
        let r = |row: &Value, i: usize| row.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        return parent_world
            * Affine::new([r(r0, 0), r(r1, 0), r(r0, 1), r(r1, 1), r(r0, 2), r(r1, 2)]);
    }
    n.get("absoluteBoundingBox").map_or(Affine::IDENTITY, |b| {
        Affine::translate((num(b, "x"), num(b, "y")))
    })
}

fn radii(n: &Value) -> RoundedRectRadii {
    if let [a, b, c, d] = arr(n, "rectangleCornerRadii") {
        let v = |x: &Value| x.as_f64().unwrap_or(0.0).max(0.0);
        return RoundedRectRadii::new(v(a), v(b), v(c), v(d));
    }
    RoundedRectRadii::from_single_radius(num(n, "cornerRadius").max(0.0))
}

fn arc(n: &Value) -> EllipseArc {
    let Some(a) = n.get("arcData") else {
        return EllipseArc::default();
    };
    let (start, end) = (num(a, "startingAngle"), num(a, "endingAngle"));
    let sweep = (end - start).to_degrees();
    if sweep.abs() >= 359.999 && num(a, "innerRadius") == 0.0 {
        return EllipseArc::default();
    }
    EllipseArc {
        start: start.to_degrees().rem_euclid(360.0),
        sweep: sweep
            .rem_euclid(360.0)
            .max(if sweep > 0.0 { 0.0 } else { 360.0 }),
        ratio: num(a, "innerRadius").clamp(0.0, 0.99),
    }
}

/// A node's outline from its `fillGeometry` or `strokeGeometry`, every path of
/// it, in the node's own space.
fn geometry(n: &Value, key: &str) -> Option<BezPath> {
    let mut out = BezPath::new();
    for g in arr(n, key) {
        if let Ok(p) = BezPath::from_svg(str_of(g, "path")) {
            out.extend(p);
        }
    }
    (!out.elements().is_empty()).then_some(out)
}

/// A node's centre line from its `vectorNetwork`, in the node's own space: each
/// segment a cubic from its tangents (relative to its ends) or a line when both
/// are zero, chained end to start, and a chain that comes back closed.
fn network(n: &Value) -> Option<BezPath> {
    let vn = n.get("vectorNetwork")?;
    let at: Vec<Point> = arr(vn, "vertices")
        .iter()
        .map(|v| {
            let p = v.get("position").unwrap_or(&Value::Null);
            Point::new(num(p, "x"), num(p, "y"))
        })
        .collect();
    let tangent = |s: &Value, key: &str| {
        let t = s.get(key).unwrap_or(&Value::Null);
        Vec2::new(num(t, "x"), num(t, "y"))
    };
    let segs: Vec<(usize, Vec2, Vec2, usize)> = arr(vn, "segments")
        .iter()
        .filter_map(|s| {
            let end = |key| s.get(key).and_then(Value::as_u64).map(|i| i as usize);
            let (a, b) = (end("start")?, end("end")?);
            (a < at.len() && b < at.len())
                .then(|| (a, tangent(s, "startTangent"), tangent(s, "endTangent"), b))
        })
        .collect();
    let mut leaving: Vec<Vec<usize>> = vec![Vec::new(); at.len()];
    let mut arriving = vec![0usize; at.len()];
    for (i, s) in segs.iter().enumerate() {
        leaving[s.0].push(i);
        arriving[s.3] += 1;
    }
    let mut used = vec![false; segs.len()];
    let mut out = BezPath::new();
    // Chains start where nothing leads in first, so a run is not split; loops
    // are what is left.
    let order = (0..segs.len()).filter(|&i| arriving[segs[i].0] == 0);
    let order: Vec<usize> = order.chain(0..segs.len()).collect();
    for first in order {
        if used[first] {
            continue;
        }
        let origin = segs[first].0;
        out.move_to(at[origin]);
        let mut next = Some(first);
        let mut end = origin;
        while let Some(i) = next {
            used[i] = true;
            let (a, t0, t1, b) = segs[i];
            if t0 == Vec2::ZERO && t1 == Vec2::ZERO {
                out.line_to(at[b]);
            } else {
                out.curve_to(at[a] + t0, at[b] + t1, at[b]);
            }
            end = b;
            next = leaving[b].iter().copied().find(|&j| !used[j]);
        }
        if end == origin {
            out.close_path();
        }
    }
    (!out.elements().is_empty()).then_some(out)
}

/// A gradient paint in a box of `size`: Figma's handles are fractions of it — the
/// start, the end, and the far edge of the ellipse's other axis.
fn gradient(p: &Value, kind: &str, size: Size, alpha: f32) -> crate::Brush {
    let handle = |i: usize| {
        arr(p, "gradientHandlePositions")
            .get(i)
            .map_or(Point::ZERO, |h| {
                Point::new(num(h, "x") * size.width, num(h, "y") * size.height)
            })
    };
    let stops: Vec<peniko::ColorStop> = arr(p, "gradientStops")
        .iter()
        .map(|s| peniko::ColorStop {
            offset: num(s, "position").clamp(0.0, 1.0) as f32,
            color: peniko::color::DynamicColor::from_alpha_color(
                s.get("color").map_or(Color::BLACK, colour),
            ),
        })
        .collect();
    let (p0, p1, p2) = (handle(0), handle(1), handle(2));
    let (gradient, transform) = match kind {
        "GRADIENT_LINEAR" => (
            peniko::Gradient::new_linear((p0.x, p0.y), (p1.x, p1.y)),
            Affine::IDENTITY,
        ),
        _ => {
            // The unit circle mapped onto the handles' ellipse.
            let (u, v) = (p1 - p0, p2 - p0);
            let onto = Affine::new([u.x, u.y, v.x, v.y, p0.x, p0.y]);
            let unit = if kind == "GRADIENT_ANGULAR" {
                peniko::Gradient::new_sweep((0.0, 0.0), 0.0, std::f32::consts::TAU)
            } else {
                peniko::Gradient::new_radial((0.0, 0.0), 1.0)
            };
            (unit, onto)
        }
    };
    peniko::Brush::Gradient(GradientBrush {
        gradient: gradient.with_stops(stops.as_slice()),
        transform,
        opacity: alpha,
    })
}

/// A Figma colour, channels in `0..=1`.
fn colour(c: &Value) -> Color {
    let ch = |k: &str| num(c, k).clamp(0.0, 1.0) as f32;
    Color::new([
        ch("r"),
        ch("g"),
        ch("b"),
        c.get("a").and_then(Value::as_f64).unwrap_or(1.0) as f32,
    ])
}

/// A Figma type style laid over `base`: what it states replaces what `base` has.
fn type_style(s: &Value, base: &TextStyle) -> TextStyle {
    let f = |k: &str| s.get(k).and_then(Value::as_f64);
    let line_height = match str_of(s, "lineHeightUnit") {
        "PIXELS" => f("lineHeightPx").map(|h| Some(Length::Px(h))),
        "FONT_SIZE_%" => f("lineHeightPercentFontSize").map(|p| Some(Length::Em(p / 100.0))),
        "INTRINSIC_%" => Some(None),
        _ => None,
    };
    let decoration = str_of(s, "textDecoration");
    TextStyle {
        font_family: s
            .get("fontFamily")
            .and_then(Value::as_str)
            .map_or_else(|| base.font_family.clone(), str::to_owned),
        font_size: f("fontSize").unwrap_or(base.font_size).max(1.0),
        weight: f("fontWeight").map_or(base.weight, |w| w as u16),
        italic: s
            .get("italic")
            .and_then(Value::as_bool)
            .unwrap_or(base.italic),
        letter_spacing: f("letterSpacing").map_or(base.letter_spacing, Length::Px),
        line_height: line_height.unwrap_or(base.line_height),
        underline: if decoration.is_empty() {
            base.underline
        } else {
            (decoration == "UNDERLINE").then(Decoration::default)
        },
        strikethrough: if decoration.is_empty() {
            base.strikethrough
        } else {
            (decoration == "STRIKETHROUGH").then(Decoration::default)
        },
        case: match str_of(s, "textCase") {
            "UPPER" => TextCase::Upper,
            "LOWER" => TextCase::Lower,
            "TITLE" => TextCase::Title,
            "ORIGINAL" => TextCase::Original,
            _ => base.case,
        },
        ..base.clone()
    }
}

/// A blend mode other than the default, or `None` for the default.
fn blend_mode(s: &str) -> Option<BlendMode> {
    Some(match s {
        "DARKEN" => BlendMode::Darken,
        "MULTIPLY" => BlendMode::Multiply,
        "COLOR_BURN" | "LINEAR_BURN" => BlendMode::ColorBurn,
        "LIGHTEN" => BlendMode::Lighten,
        "SCREEN" => BlendMode::Screen,
        "COLOR_DODGE" | "LINEAR_DODGE" => BlendMode::ColorDodge,
        "OVERLAY" => BlendMode::Overlay,
        "SOFT_LIGHT" => BlendMode::SoftLight,
        "HARD_LIGHT" => BlendMode::HardLight,
        "DIFFERENCE" => BlendMode::Difference,
        "EXCLUSION" => BlendMode::Exclusion,
        "HUE" => BlendMode::Hue,
        "SATURATION" => BlendMode::Saturation,
        "COLOR" => BlendMode::Color,
        "LUMINOSITY" => BlendMode::Luminosity,
        _ => return None,
    })
}
