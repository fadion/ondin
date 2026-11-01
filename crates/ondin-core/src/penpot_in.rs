//! Penpot import: a `.penpot` export read into a new document — its pages,
//! boards, shapes, components, library colours and typographies as Ondin's own.
//!
//! A `.penpot` file (Penpot 2's export, its third format) is a zip of JSON: a
//! `manifest.json` naming the files inside it, then per file `files/<file>.json`,
//! `files/<file>/pages/<page>.json`, one `files/<file>/pages/<page>/<shape>.json`
//! per shape, and `colors/`, `typographies/`, `components/` and `media/` beside
//! them; pictures sit under `objects/`. The older binary export is unpacked into
//! those same entries first (`crate::penpot_v1`).
//!
//! Penpot places every shape in page space: a box (`x`, `y`, `width`, `height`)
//! turned by `transform` about its centre, a path by its own absolute outline.
//! Each node here is placed in its parent's space by dividing out the parent's.
//!
//! An instance's shapes each name the main shape they copy (`shapeRef`), which
//! is Ondin's link; an instance whose every copy finds its main is linked whole,
//! any other is kept as plain layers and reported.
//!
//! An archive exported with its libraries holds those files too: the file that
//! is no other's library is read, and its libraries' pages come after its own so
//! their mains are in the document.

use crate::document::Document;
use crate::effect::{Effect, EffectKind, Shadow};
use crate::foreign::{Archive, Names, Tally, Unmapped, arr, b_of, num, sniff, solid, str_of};
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
use crate::variable::{BrushSource, Cell, Mode, VarId, VarKind, Variable};
use crate::{BlockStyle, TextSizing, VerticalAlign};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::Value;
use std::collections::BTreeMap;

/// A Penpot file read into a document.
pub struct Read {
    pub doc: Document,
    /// The file's own name, which the copy is filed under.
    pub name: String,
    /// What did not come in as it was, one row per kind, in a stable order.
    pub report: Vec<Unmapped>,
    /// Every font family the text asks for, with how many runs ask for it.
    pub families: BTreeMap<String, usize>,
}

/// Why a file could not be read at all.
#[derive(Debug, thiserror::Error)]
pub enum PenpotError {
    #[error("not a Penpot export: it has no manifest.json")]
    NoManifest,
    #[error(
        "a binary Penpot export of a version Ondin does not read; open it in Penpot and export it again"
    )]
    OldFormat,
    #[error("not a Penpot export: {0}")]
    Json(String),
    #[error("the file has no pages")]
    NoPages,
    #[error("the document did not build: {0}")]
    Build(#[from] OpError),
}

/// The id Penpot gives every page's root frame.
const ROOT: &str = "00000000-0000-0000-0000-000000000000";

/// How deep shapes may nest before the rest is left out. Shapes link by id, so
/// nothing in the JSON bounds the depth, and each level is a stack frame on the
/// reading worker; Penpot's own files nest a few dozen deep at most.
const MAX_NESTING: usize = 128;

/// Whether `bytes` are Penpot's older binary export rather than a zip; the
/// version `crate::penpot_v1` reads is told apart by `penpot_v1::is_binfile`.
pub fn is_old_format(bytes: &[u8]) -> bool {
    !bytes.starts_with(b"PK\x03\x04")
}

/// Read the Penpot export behind `archive` into a new document, minting from
/// `ids` and handing pictures to `images`.
pub fn read(
    archive: &mut dyn Archive,
    ids: &mut IdSource,
    images: &mut dyn ImageSink,
) -> Result<Read, PenpotError> {
    let manifest = json(archive, "manifest.json")?.ok_or(PenpotError::NoManifest)?;
    let files: Vec<(String, String)> = arr(&manifest, "files")
        .iter()
        .map(|f| (str_of(f, "id").to_owned(), str_of(f, "name").to_owned()))
        .collect();
    let relations: Vec<(String, String)> = arr(&manifest, "relations")
        .iter()
        .filter_map(|r| {
            let r = r.as_array()?;
            Some((
                r.first()?.as_str()?.to_owned(),
                r.get(1)?.as_str()?.to_owned(),
            ))
        })
        .collect();
    let libraries: FxHashSet<&str> = relations.iter().map(|(_, lib)| lib.as_str()).collect();
    let Some((main, name)) = files
        .iter()
        .find(|(id, _)| !libraries.contains(id.as_str()))
        .or(files.first())
        .cloned()
    else {
        return Err(PenpotError::NoPages);
    };
    let mut read: Vec<String> = vec![main.clone()];
    read.extend(
        relations
            .iter()
            .filter(|(file, _)| *file == main)
            .map(|(_, lib)| lib.clone()),
    );
    let names = archive.names();
    let first = ids.mint();
    let mut b = Builder {
        doc: Document::new(first),
        ids,
        archive,
        images,
        names,
        ops: Vec::new(),
        shapes: FxHashMap::default(),
        nodes: FxHashMap::default(),
        colours: FxHashMap::default(),
        typographies: FxHashMap::default(),
        pictures: FxHashMap::default(),
        tally: Tally::default(),
        taken: Names::default(),
        families: BTreeMap::new(),
        pages: 0,
        first,
    };
    let left = files.iter().filter(|(id, _)| !read.contains(id)).count();
    if left > 0 {
        b.tally.count(
            "Other files in the export",
            left,
            "",
            "Left out: one file at a time",
        );
    }
    b.assets(&read)?;
    for (i, file) in read.iter().enumerate() {
        let prefix = (i > 0).then(|| {
            files
                .iter()
                .find(|(id, _)| id == file)
                .map_or_else(String::new, |(_, n)| n.clone())
        });
        b.file(file, prefix.as_deref())?;
    }
    if b.pages == 0 {
        return Err(PenpotError::NoPages);
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
        name,
        report: tally.into_report(),
        families,
    })
}

fn json(archive: &mut dyn Archive, name: &str) -> Result<Option<Value>, PenpotError> {
    archive
        .entry(name)
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|e| PenpotError::Json(e.to_string())))
        .transpose()
}

struct Builder<'a> {
    doc: Document,
    ids: &'a mut IdSource,
    archive: &'a mut dyn Archive,
    images: &'a mut dyn ImageSink,
    /// Every entry in the archive, for the folders the manifest does not list.
    names: Vec<String>,
    ops: Vec<Operation>,
    /// Penpot shape id → the shape, for the second pass.
    shapes: FxHashMap<String, Value>,
    /// Penpot shape id → the node made for it.
    nodes: FxHashMap<String, NodeId>,
    /// Library colour id → its variable and the opacity its value carries.
    colours: FxHashMap<String, (VarId, f32)>,
    typographies: FxHashMap<String, (StyleId, TextStyle)>,
    pictures: FxHashMap<String, Option<ImageId>>,
    tally: Tally,
    /// Style names given out, by slot; `None` holds the colour variables'.
    taken: Names<Option<StyleSlot>>,
    families: BTreeMap<String, usize>,
    /// How many pages have been written.
    pages: usize,
    /// The document's own first page, which the first Penpot page becomes.
    first: NodeId,
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

    /// The entries under `dir/`, sorted.
    fn listed(&self, dir: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .names
            .iter()
            .filter(|n| n.starts_with(dir) && n.ends_with(".json"))
            .filter(|n| !n[dir.len()..].contains('/'))
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Library colours as colour variables in one mode, typographies as text
    /// styles, from every file read.
    fn assets(&mut self, files: &[String]) -> Result<(), PenpotError> {
        let mut colours = Vec::new();
        let mut types = Vec::new();
        for file in files {
            for name in self.listed(&format!("files/{file}/colors/")) {
                colours.extend(json(self.archive, &name)?);
            }
            for name in self.listed(&format!("files/{file}/typographies/")) {
                types.extend(json(self.archive, &name)?);
            }
        }
        let solid_colours: Vec<&Value> = colours
            .iter()
            .filter(|c| c.get("color").and_then(Value::as_str).is_some())
            .collect();
        if solid_colours.len() < colours.len() {
            self.count(
                "Gradient and picture colours",
                colours.len() - solid_colours.len(),
                "",
                "Left out",
            );
        }
        if !solid_colours.is_empty() {
            let mode = self.ids.mint_mode();
            self.push(Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            });
            for c in solid_colours {
                let var = self.ids.mint_var();
                let colour = hex(str_of(c, "color"), opacity(c, "opacity"));
                self.colours
                    .insert(str_of(c, "id").to_owned(), (var, opacity(c, "opacity")));
                let name = self
                    .taken
                    .unique(None, &asset_name(c), "Colour", &mut self.tally);
                self.ops.push(Operation::AddVariable {
                    var: Variable {
                        id: var,
                        name,
                        kind: VarKind::Color,
                        cells: [(mode, Cell::Color(colour))].into(),
                        origin: None,
                        forked: None,
                        description: String::new(),
                    },
                });
            }
        }
        for t in &types {
            let ts = crate::style::type_part(&leaf_style(t));
            let id = self.ids.mint_style();
            let name = self.taken.unique(
                Some(StyleSlot::Text),
                &asset_name(t),
                "Style",
                &mut self.tally,
            );
            self.push(Operation::AddStyle {
                style: Style {
                    id,
                    name,
                    value: StyleValue::Text(Box::new(ts.clone())),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            });
            self.typographies
                .insert(str_of(t, "id").to_owned(), (id, ts));
        }
        Ok(())
    }

    /// One file's pages, in Penpot's order, named after the file when it is a
    /// library brought along.
    fn file(&mut self, file: &str, library: Option<&str>) -> Result<(), PenpotError> {
        let mut pages = Vec::new();
        for name in self.listed(&format!("files/{file}/pages/")) {
            if let Some(page) = json(self.archive, &name)? {
                pages.push(page);
            }
        }
        pages.sort_by_key(|p| num(p, "index") as i64);
        if library.is_some() && !pages.is_empty() {
            self.count(
                "Library pages",
                pages.len(),
                "",
                "Brought in after the file's own, for their components",
            );
        }
        for page in pages {
            let pid = str_of(&page, "id").to_owned();
            let dir = format!("files/{file}/pages/{pid}/");
            for name in self.listed(&dir) {
                if let Some(shape) = json(self.archive, &name)? {
                    self.shapes.insert(str_of(&shape, "id").to_owned(), shape);
                }
            }
            // Shape ids are UUIDs, except the root's, which every page shares.
            let root = self.shapes.remove(ROOT);
            let name = match library {
                Some(lib) => format!("{lib} · {}", str_of(&page, "name")),
                None => str_of(&page, "name").to_owned(),
            };
            let id = if self.pages == 0 {
                self.first
            } else {
                self.ids.mint()
            };
            if self.pages == 0 {
                self.push(Operation::SetName { id, name });
            } else {
                self.push(Operation::AddPage {
                    id,
                    index: self.pages,
                    name,
                    ground: None,
                    draft: false,
                    mode: None,
                });
            }
            self.pages += 1;
            if let Some(root) = root {
                let mut placed = 0;
                for child in &child_ids(&root) {
                    placed += usize::from(self.shape(child, id, placed, Affine::IDENTITY, 1));
                }
            }
        }
        Ok(())
    }

    /// One shape and what it holds, at `index` under `parent`, whose world
    /// placement is `parent_world`, `depth` levels below the page. False when it
    /// was left out, so the caller's next sibling takes the same index. A shape
    /// already placed (one listed inside itself, or by two parents) is left out.
    fn shape(
        &mut self,
        sid: &str,
        parent: NodeId,
        index: usize,
        parent_world: Affine,
        depth: usize,
    ) -> bool {
        if self.nodes.contains_key(sid) {
            self.count(
                "Layers listed inside themselves or twice",
                1,
                "",
                "Placed once",
            );
            return false;
        }
        let Some(shape) = self.shapes.get(sid).cloned() else {
            return false;
        };
        if depth > MAX_NESTING {
            self.count("Layers nested too deeply", 1, "", "Left out");
            return false;
        }
        let kind = str_of(&shape, "type");
        if !arr(&shape, "interactions").is_empty() {
            self.count("Prototype interactions", 1, "", "Left out");
        }
        if !arr(&shape, "exports").is_empty() {
            self.count("Export settings", 1, "layers", "Left out");
        }
        if matches!(str_of(&shape, "layout"), "flex" | "grid") {
            self.count(
                "Flex and grid layout",
                1,
                "boards",
                "Kept at their drawn positions",
            );
        }
        let id = self.ids.mint();
        let size = box_size(&shape);
        let mut ink = None;
        let (node_kind, world) = match kind {
            "frame" => (
                NodeKind::Artboard {
                    size,
                    corner_radii: radii(&shape),
                    corner_smoothing: 0.0,
                },
                box_world(&shape),
            ),
            "rect" => (
                NodeKind::Rect {
                    size,
                    corner_radii: radii(&shape),
                    corner_smoothing: 0.0,
                },
                box_world(&shape),
            ),
            "circle" => (
                NodeKind::Ellipse {
                    size,
                    arc: EllipseArc::default(),
                },
                box_world(&shape),
            ),
            "image" => (
                NodeKind::Rect {
                    size,
                    corner_radii: radii(&shape),
                    corner_smoothing: 0.0,
                },
                box_world(&shape),
            ),
            "text" => {
                let (k, colour) = self.text_kind(&shape);
                ink = Some(colour);
                (k, box_world(&shape))
            }
            "group" => (NodeKind::Group, corner_world(&shape)),
            "bool" => (
                NodeKind::Boolean {
                    op: match str_of(&shape, "boolType") {
                        "difference" => BoolOp::Subtract,
                        "intersection" => BoolOp::Intersect,
                        "exclude" => BoolOp::Exclude,
                        _ => BoolOp::Union,
                    },
                },
                corner_world(&shape),
            ),
            "path" => {
                let world = corner_world(&shape);
                let Some(path) = path_of(&shape, world) else {
                    self.count("Paths that could not be read", 1, "", "Left out");
                    return false;
                };
                (
                    NodeKind::Path {
                        path,
                        corner_radii: Vec::new(),
                        corner_smoothing: 0.0,
                    },
                    world,
                )
            }
            "svg-raw" => {
                self.count("SVG layers", 1, "", "Left out");
                return false;
            }
            _ => {
                self.count("Layers of an unknown kind", 1, "", "Left out");
                return false;
            }
        };
        self.push(Operation::CreateNode {
            id,
            parent,
            index,
            kind: node_kind,
            transform: Some(parent_world.inverse() * world),
            name: Some(str_of(&shape, "name").to_owned()),
        });
        self.nodes.insert(sid.to_owned(), id);
        if b_of(&shape, "hidden") {
            self.push(Operation::SetVisible { id, visible: false });
        }
        if b_of(&shape, "blocked") {
            self.push(Operation::SetLocked { id, locked: true });
        }
        if kind == "frame" {
            self.push(Operation::SetClip {
                id,
                clip: !b_of(&shape, "showContent"),
            });
        }
        self.style(id, &shape, kind, size);
        if let Some(c) = ink {
            self.push(Operation::SetFills {
                id,
                fills: keyed_by_position([solid(c)]),
            });
            // Bound to its typography while the first run is still drawn as it.
            let content = shape.get("content").unwrap_or(&Value::Null);
            let first = paragraphs(content).first().and_then(|p| kids(p).first());
            if let Some(leaf) = first
                && let Some((style, typed)) =
                    self.typographies
                        .get(get(leaf, "typographyRefId", "typography-ref-id"))
                && *typed == crate::style::type_part(&leaf_style(leaf))
            {
                let style = *style;
                self.push(Operation::SetTypeStyle {
                    id,
                    style: Some(style),
                });
            }
        }
        if kind == "image" {
            let media = shape.get("metadata").map(|m| str_of(m, "id").to_owned());
            match media.and_then(|m| self.picture(&m)) {
                // Penpot stretches an image layer's picture onto its box.
                Some(image) => self.push(Operation::SetFills {
                    id,
                    fills: keyed_by_position([Fill {
                        brush: fitted_brush(image, ImageFit::Crop).into(),
                        visible: true,
                    }]),
                }),
                None => self.count("Pictures", 1, "", "Could not be read"),
            }
        }
        let children = child_ids(&shape);
        let mut placed = 0;
        for child in &children {
            placed += usize::from(self.shape(child, id, placed, world, depth + 1));
        }
        // A masked group's first shape is its mask.
        if kind == "group"
            && b_of(&shape, "maskedGroup")
            && let Some(&mask) = children.first().and_then(|c| self.nodes.get(c))
        {
            self.push(Operation::SetMask {
                id: mask,
                mask: true,
            });
        }
        true
    }

    /// A shape's paints, effects, opacity and blend onto `id`. A text's colour
    /// lives in its runs; its node fill is the first run's.
    fn style(&mut self, id: NodeId, shape: &Value, kind: &str, size: Size) {
        let o = num_or(shape, "opacity", 1.0);
        if o < 1.0 {
            self.push(Operation::SetOpacity {
                id,
                opacity: o as f32,
            });
        }
        let blend = blend_mode(str_of(shape, "blendMode"));
        if blend != BlendMode::Normal {
            self.push(Operation::SetBlend { id, blend });
        }
        // Penpot lists paints topmost first; Ondin paints in list order.
        if kind != "text" && kind != "image" {
            let fills: Vec<Fill> = arr(shape, "fills")
                .iter()
                .rev()
                .filter_map(|f| {
                    Some(Fill {
                        brush: self.brush(f, "fill", size)?,
                        visible: true,
                    })
                })
                .collect();
            if !fills.is_empty() {
                self.push(Operation::SetFills {
                    id,
                    fills: keyed_by_position(fills),
                });
            }
        }
        let strokes: Vec<Stroke> = arr(shape, "strokes")
            .iter()
            .rev()
            .filter_map(|s| {
                let brush = self.brush(s, "stroke", size)?;
                let width = num_or(s, "strokeWidth", 1.0);
                // Penpot's own pattern, which grows with the width; its dots
                // are always round, whatever the stroke's caps.
                let dotted = str_of(s, "strokeStyle") == "dotted";
                let dashes = match str_of(s, "strokeStyle") {
                    "dashed" => vec![
                        num_or(s, "strokeDash", width + 10.0),
                        num_or(s, "strokeGap", width + 10.0),
                    ],
                    "dotted" => vec![0.0, width + 5.0],
                    "mixed" => vec![width + 5.0, width + 5.0, width + 1.0, width + 5.0],
                    _ => Vec::new(),
                };
                let plain = |k: &str| matches!(str_of(s, k), "" | "round" | "square");
                if dotted {
                    // Round dots, whatever the caps say.
                } else if !plain("strokeCapStart") || !plain("strokeCapEnd") {
                    self.count("Arrow and marker caps", 1, "strokes", "Drawn without them");
                } else if str_of(s, "strokeCapStart") != str_of(s, "strokeCapEnd") {
                    self.count(
                        "Different caps at each end",
                        1,
                        "strokes",
                        "Both drawn as the end's",
                    );
                }
                let cap = match str_of(s, "strokeCapEnd") {
                    _ if dotted => crate::kurbo::Cap::Round,
                    "round" => crate::kurbo::Cap::Round,
                    "square" => crate::kurbo::Cap::Square,
                    _ => crate::kurbo::Cap::Butt,
                };
                Some(Stroke {
                    brush,
                    width,
                    width_var: None,
                    join: crate::kurbo::Join::Miter,
                    cap,
                    dashes,
                    dash_offset: 0.0,
                    miter_limit: 4.0,
                    dash_fit: false,
                    align: match str_of(s, "strokeAlignment") {
                        "inner" => StrokeAlign::Inside,
                        "outer" => StrokeAlign::Outside,
                        _ => StrokeAlign::Center,
                    },
                    sides: crate::StrokeSides::All,
                    visible: true,
                })
            })
            .collect();
        if !strokes.is_empty() {
            self.push(Operation::SetStrokes {
                id,
                strokes: keyed_by_position(strokes),
            });
        }
        if shape
            .get("svgAttrs")
            .and_then(|a| a.get("fillRule"))
            .and_then(Value::as_str)
            == Some("evenodd")
        {
            self.push(Operation::SetFillRule {
                id,
                rule: FillRule::EvenOdd,
            });
        }
        let mut effects = Vec::new();
        for s in arr(shape, "shadow") {
            let colour = s.get("color").map_or(Color::BLACK, |c| {
                hex(str_of(c, "color"), opacity(c, "opacity"))
            });
            let shadow = Shadow {
                offset: Vec2::new(num(s, "offsetX"), num(s, "offsetY")),
                blur: num(s, "blur"),
                spread: num(s, "spread"),
                color: colour,
                blend: BlendMode::Normal,
            };
            let kind = if str_of(s, "style") == "inner-shadow" {
                EffectKind::InnerShadow(shadow)
            } else {
                EffectKind::DropShadow(shadow)
            };
            effects.push(Effect {
                visible: !b_of(s, "hidden"),
                ..Effect::new(kind)
            });
        }
        if let Some(blur) = shape.get("blur").filter(|b| b.is_object()) {
            // Penpot draws a background blur at half its value, as Ondin draws a
            // radius, but a layer blur's value is the deviation itself.
            let value = num(blur, "value");
            effects.push(Effect {
                visible: !b_of(blur, "hidden"),
                ..Effect::new(match str_of(blur, "type") {
                    "background-blur" => EffectKind::BackgroundBlur { radius: value },
                    _ => EffectKind::LayerBlur {
                        radius: value / crate::effect::BLUR_DEVIATION,
                    },
                })
            });
        }
        if !effects.is_empty() {
            self.push(Operation::SetEffects {
                id,
                effects: keyed_by_position(effects),
            });
        }
    }

    /// A fill's or stroke's paint, `key` being `fill` or `stroke`: a colour
    /// (bound to its library colour when it names one), a gradient or a picture.
    fn brush(&mut self, f: &Value, key: &str, size: Size) -> Option<BrushSource> {
        let alpha = opacity(f, &format!("{key}Opacity"));
        if let Some(g) = f
            .get(format!("{key}ColorGradient"))
            .filter(|g| g.is_object())
        {
            return Some(gradient(g, size, alpha).into());
        }
        if let Some(img) = f.get(format!("{key}Image")).filter(|i| i.is_object()) {
            let Some(id) = self.picture(str_of(img, "id")) else {
                self.count("Pictures", 1, "", "Could not be read");
                return None;
            };
            let fit = if b_of(img, "keepAspectRatio") {
                ImageFit::Fill
            } else {
                ImageFit::Crop
            };
            return Some(fitted_brush(id, fit).into());
        }
        let colour = f.get(format!("{key}Color")).and_then(Value::as_str)?;
        if let Some(&(var, own)) = f
            .get(format!("{key}ColorRefId"))
            .and_then(Value::as_str)
            .and_then(|r| self.colours.get(r))
        {
            // Penpot copies the colour's opacity into the fill's; the variable
            // already draws it, so only what the fill adds stays here.
            let alpha = if own > 0.0 {
                (alpha / own).min(1.0)
            } else {
                alpha
            };
            return Some(BrushSource::Bound { var, alpha });
        }
        Some(peniko::Brush::Solid(hex(colour, alpha)).into())
    }

    /// The picture a media id names, read once: through the file's media entry
    /// to its stored object, or the object itself.
    fn picture(&mut self, media: &str) -> Option<ImageId> {
        if media.is_empty() {
            return None;
        }
        if let Some(known) = self.pictures.get(media) {
            return known.clone();
        }
        let object = self
            .names
            .iter()
            .find(|n| n.contains("/media/") && n.ends_with(&format!("{media}.json")))
            .cloned()
            .and_then(|n| self.archive.entry(&n))
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|m| str_of(&m, "mediaId").to_owned())
            .unwrap_or_else(|| media.to_owned());
        let id = self
            .names
            .iter()
            .find(|n| n.starts_with(&format!("objects/{object}.")) && !n.ends_with(".json"))
            .cloned()
            .and_then(|n| self.archive.entry(&n))
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
        self.pictures.insert(media.to_owned(), id.clone());
        id
    }

    /// A text shape's node and the ink its first run draws in: its paragraphs
    /// joined by line breaks, the first run as the default and later runs as spans.
    fn text_kind(&mut self, shape: &Value) -> (NodeKind, Color) {
        let content = shape.get("content").cloned().unwrap_or(Value::Null);
        let mut text = String::new();
        let mut runs: Vec<(std::ops::Range<usize>, TextStyle, Option<Color>)> = Vec::new();
        let mut align = TextAlign::Start;
        for (p, paragraph) in paragraphs(&content).into_iter().enumerate() {
            if p > 0 {
                text.push('\n');
            }
            if p == 0 {
                align = match get(paragraph, "textAlign", "text-align") {
                    "center" => TextAlign::Center,
                    "right" => TextAlign::End,
                    "justify" => TextAlign::Justify,
                    _ => TextAlign::Start,
                };
            }
            for leaf in kids(paragraph) {
                let s = leaf.get("text").and_then(Value::as_str).unwrap_or("");
                let start = text.len();
                text.push_str(s);
                let mut style = leaf_style(leaf);
                if style.line_height.is_none() {
                    style.set(CharAttr::LineHeight(line_height(paragraph)));
                }
                *self.families.entry(style.font_family.clone()).or_default() += 1;
                let colour = arr(leaf, "fills").first().and_then(|f| {
                    f.get("fillColor")
                        .and_then(Value::as_str)
                        .map(|c| hex(c, opacity(f, "fillOpacity")))
                });
                runs.push((start..text.len(), style, colour));
            }
        }
        let default = runs.first().map(|r| r.1.clone()).unwrap_or_default();
        let ink = runs.first().and_then(|r| r.2).unwrap_or(Color::BLACK);
        let mut spans = CharSpans::default();
        for (range, style, colour) in &runs {
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
            let c = colour.filter(|c| *c != ink);
            spans.set(range.clone(), CharAttr::Color(c), &default);
        }
        let size = box_size(shape);
        let kind = NodeKind::Text {
            content: text,
            style: Box::new(default),
            spans,
            para_spans: ParaSpans::default(),
            paragraph: ParagraphStyle {
                align,
                ..ParagraphStyle::default()
            },
            block: BlockStyle {
                vertical_align: match get(&content, "verticalAlign", "vertical-align") {
                    "center" => VerticalAlign::Middle,
                    "bottom" => VerticalAlign::Bottom,
                    _ => VerticalAlign::Top,
                },
                ..BlockStyle::default()
            },
            sizing: match str_of(shape, "growType") {
                "auto-width" => TextSizing::Auto,
                "auto-height" => TextSizing::AutoHeight(size.width),
                _ => TextSizing::Fixed(size),
            },
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        };
        (kind, ink)
    }

    /// Main components become components; an instance whose every copy finds
    /// the main shape it copies is linked to it, node by node.
    fn components(&mut self) -> Result<(), PenpotError> {
        let mut ops = Vec::new();
        let mut mains: FxHashSet<NodeId> = FxHashSet::default();
        for (sid, shape) in &self.shapes {
            if b_of(shape, "mainInstance")
                && let Some(&id) = self.nodes.get(sid)
            {
                mains.insert(id);
            }
        }
        // A main inside another main cannot be one here.
        let inside = |doc: &Document, id: NodeId, mains: &FxHashSet<NodeId>| {
            let mut at = doc.get(id).and_then(|n| n.parent());
            while let Some(a) = at {
                if mains.contains(&a) {
                    return true;
                }
                at = doc.get(a).and_then(|n| n.parent());
            }
            false
        };
        let nested: Vec<NodeId> = mains
            .iter()
            .copied()
            .filter(|&m| inside(&self.doc, m, &mains))
            .collect();
        for m in &nested {
            mains.remove(m);
        }
        if !nested.is_empty() {
            self.count(
                "Main components inside other components",
                nested.len(),
                "",
                "Kept as plain layers",
            );
        }
        for &m in &mains {
            if matches!(
                self.doc.get(m).map(|n| n.kind()),
                Some(NodeKind::Artboard { .. } | NodeKind::Group)
            ) {
                ops.push(Operation::SetComponent {
                    id: m,
                    component: true,
                });
            }
        }
        // Each instance's copies, grouped under the root they belong to.
        let mut instances: BTreeMap<String, Vec<(NodeId, NodeId)>> = BTreeMap::new();
        let mut broken: FxHashSet<String> = FxHashSet::default();
        for (sid, shape) in &self.shapes {
            if b_of(shape, "mainInstance") {
                continue;
            }
            let Some(source) = shape.get("shapeRef").and_then(Value::as_str) else {
                continue;
            };
            let Some(&id) = self.nodes.get(sid) else {
                continue;
            };
            let root = self.instance_root(sid);
            match self.nodes.get(source) {
                Some(&target) => instances.entry(root).or_default().push((id, target)),
                None => {
                    broken.insert(root);
                }
            }
        }
        let (mut unlinked, mut linked) = (0, 0);
        for (root, links) in &instances {
            if broken.contains(root) || links.iter().any(|(_, t)| nested.contains(t)) {
                unlinked += 1;
                continue;
            }
            linked += 1;
            ops.extend(links.iter().map(|&(id, target)| Operation::SetLink {
                id,
                link: Some(target),
            }));
        }
        unlinked += broken
            .iter()
            .filter(|r| !instances.contains_key(*r))
            .count();
        if unlinked > 0 {
            self.count(
                "Instances whose main is not in the file",
                unlinked,
                "",
                "Kept as plain layers",
            );
        }
        // Linked as a whole or not at all: a rule the commit refuses is a whole
        // instance kept plain, not a failed import.
        if self.commit(ops.clone()).is_err() {
            let unmade: Vec<Operation> = ops
                .into_iter()
                .filter(|op| matches!(op, Operation::SetComponent { .. }))
                .collect();
            self.commit(unmade)?;
            self.count(
                "Instances that did not match their main",
                linked,
                "",
                "Kept as plain layers",
            );
        }
        Ok(())
    }

    /// The Penpot id of the instance root `sid` belongs to: the nearest shape at
    /// or above it marked as a component root.
    fn instance_root(&self, sid: &str) -> String {
        let mut at = sid.to_owned();
        for _ in 0..self.shapes.len() {
            let Some(shape) = self.shapes.get(&at) else {
                break;
            };
            if b_of(shape, "componentRoot") {
                return at;
            }
            let parent = str_of(shape, "parentId");
            if parent.is_empty() || parent == ROOT || parent == at {
                break;
            }
            at = parent.to_owned();
        }
        sid.to_owned()
    }
}

/// A shape's children, back to front.
fn child_ids(shape: &Value) -> Vec<String> {
    arr(shape, "shapes")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// A picture's brush at `fit`; `Crop` with the whole picture maps it onto the
/// box exactly, which is how Penpot draws a picture whose aspect is not kept.
fn fitted_brush(id: ImageId, fit: ImageFit) -> crate::Brush {
    let mut brush = image_brush(id);
    if let peniko::Brush::Image(b) = &mut brush {
        b.image.fit = fit;
    }
    brush
}

/// A box's size, `selrect`'s for a shape without its own.
fn box_size(shape: &Value) -> Size {
    let w = shape.get("width").and_then(Value::as_f64);
    let h = shape.get("height").and_then(Value::as_f64);
    match (w, h) {
        (Some(w), Some(h)) => Size::new(w.max(0.0), h.max(0.0)),
        _ => shape.get("selrect").map_or(Size::ZERO, |r| {
            Size::new(num(r, "width").max(0.0), num(r, "height").max(0.0))
        }),
    }
}

/// Where a box shape sits on the page: its box at (`x`, `y`) turned by
/// `transform` about the box's centre.
fn box_world(shape: &Value) -> Affine {
    let (x, y) = (num(shape, "x"), num(shape, "y"));
    let size = box_size(shape);
    let c = Vec2::new(x + size.width / 2.0, y + size.height / 2.0);
    Affine::translate(c) * matrix(shape) * Affine::translate(-c) * Affine::translate((x, y))
}

/// Where a shape whose children or outline are in page space sits: at its
/// selection box's corner, unturned.
fn corner_world(shape: &Value) -> Affine {
    let r = shape.get("selrect");
    Affine::translate((
        r.map_or(0.0, |r| num(r, "x")),
        r.map_or(0.0, |r| num(r, "y")),
    ))
}

fn matrix(shape: &Value) -> Affine {
    let Some(t) = shape.get("transform").filter(|t| t.is_object()) else {
        return Affine::IDENTITY;
    };
    Affine::new([
        num_or(t, "a", 1.0),
        num(t, "b"),
        num(t, "c"),
        num_or(t, "d", 1.0),
        num(t, "e"),
        num(t, "f"),
    ])
}

fn radii(shape: &Value) -> RoundedRectRadii {
    let rx = num(shape, "rx");
    let r = |k: &str| shape.get(k).and_then(Value::as_f64).unwrap_or(rx).max(0.0);
    RoundedRectRadii::new(r("r1"), r("r2"), r("r3"), r("r4"))
}

/// A path's outline in the space of `world`: Penpot stores it on the page, as
/// SVG path data or as a list of commands.
fn path_of(shape: &Value, world: Affine) -> Option<BezPath> {
    let mut path = match shape.get("content")? {
        Value::String(d) => BezPath::from_svg(&d.replace("ZZ", "Z")).ok()?,
        Value::Array(commands) => {
            let mut p = BezPath::new();
            for c in commands {
                let params = c.get("params").unwrap_or(&Value::Null);
                let at = |x: &str, y: &str| Point::new(num(params, x), num(params, y));
                match str_of(c, "command") {
                    "move-to" => p.move_to(at("x", "y")),
                    "line-to" => p.line_to(at("x", "y")),
                    "curve-to" => p.curve_to(at("c1x", "c1y"), at("c2x", "c2y"), at("x", "y")),
                    "close-path" => p.close_path(),
                    _ => {}
                }
            }
            p
        }
        _ => return None,
    };
    path.apply_affine(world.inverse());
    Some(path)
}

/// A gradient in a box of `size`: Penpot's points are fractions of the box. A
/// radial one is a circle in those fractions, so an ellipse on a box that is not
/// square, its axis across the start-to-end line `width` times the other.
fn gradient(g: &Value, size: Size, alpha: f32) -> crate::Brush {
    let at = |x: &str, y: &str| Point::new(num(g, x) * size.width, num(g, y) * size.height);
    let unit = |x: &str, y: &str| Point::new(num(g, x), num(g, y));
    let stops: Vec<peniko::ColorStop> = arr(g, "stops")
        .iter()
        .map(|s| peniko::ColorStop {
            offset: num(s, "offset").clamp(0.0, 1.0) as f32,
            color: peniko::color::DynamicColor::from_alpha_color(hex(
                str_of(s, "color"),
                opacity(s, "opacity"),
            )),
        })
        .collect();
    let (start, end) = (at("startX", "startY"), at("endX", "endY"));
    let (s, axis) = (
        unit("startX", "startY"),
        unit("endX", "endY") - unit("startX", "startY"),
    );
    let r = axis.hypot();
    let (gradient, transform) = if str_of(g, "type") == "radial" && r > 1e-9 {
        // The unit circle placed as Penpot's `gradientTransform` places it,
        // then the box's fractions scaled to the box.
        let turn = axis.y.atan2(axis.x) + std::f64::consts::FRAC_PI_2;
        let onto = Affine::scale_non_uniform(size.width, size.height)
            * Affine::translate(s.to_vec2())
            * Affine::rotate(turn)
            * Affine::scale_non_uniform(num_or(g, "width", 1.0) * r, r);
        (peniko::Gradient::new_radial((0.0, 0.0), 1.0), onto)
    } else if str_of(g, "type") == "radial" {
        (
            peniko::Gradient::new_radial((start.x, start.y), 0.0),
            Affine::IDENTITY,
        )
    } else {
        (
            peniko::Gradient::new_linear((start.x, start.y), (end.x, end.y)),
            Affine::IDENTITY,
        )
    };
    peniko::Brush::Gradient(GradientBrush {
        gradient: gradient.with_stops(stops.as_slice()),
        transform,
        opacity: alpha,
    })
}

/// `#rrggbb` at `alpha`.
fn hex(s: &str, alpha: f32) -> Color {
    peniko::color::parse_color(s.trim())
        .map(|c| c.to_alpha_color::<peniko::color::Srgb>())
        .unwrap_or(Color::BLACK)
        .multiply_alpha(alpha)
}

/// An opacity field, `1` when absent.
fn opacity(v: &Value, k: &str) -> f32 {
    num_or(v, k, 1.0).clamp(0.0, 1.0) as f32
}

fn num_or(v: &Value, k: &str, default: f64) -> f64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(default),
        // Text attributes are written as strings.
        Some(Value::String(s)) => s.trim().trim_end_matches("px").parse().unwrap_or(default),
        _ => default,
    }
}

/// A key as Penpot writes it, camel-cased or not.
fn get<'a>(v: &'a Value, camel: &str, kebab: &str) -> &'a str {
    v.get(camel)
        .or_else(|| v.get(kebab))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn num_kv(v: &Value, camel: &str, kebab: &str, default: f64) -> f64 {
    if v.get(camel).is_some() {
        num_or(v, camel, default)
    } else {
        num_or(v, kebab, default)
    }
}

fn kids(v: &Value) -> &[Value] {
    arr(v, "children")
}

/// The paragraphs of a text's content, through its paragraph sets.
fn paragraphs(content: &Value) -> Vec<&Value> {
    kids(content)
        .iter()
        .flat_map(|set| {
            if str_of(set, "type") == "paragraph-set" {
                kids(set).iter().collect::<Vec<_>>()
            } else {
                vec![set]
            }
        })
        .collect()
}

fn line_height(v: &Value) -> Option<Length> {
    let h = num_kv(v, "lineHeight", "line-height", 0.0);
    (h > 0.0).then_some(Length::Em(h))
}

/// The character style a text leaf (or a typography) describes.
fn leaf_style(v: &Value) -> TextStyle {
    let family = match get(v, "fontFamily", "font-family") {
        "" | "sourcesanspro" => "Source Sans Pro".to_owned(),
        f => f.to_owned(),
    };
    let weight = num_kv(v, "fontWeight", "font-weight", 400.0) as u16;
    let decoration = get(v, "textDecoration", "text-decoration");
    crate::foreign::canonical_style(TextStyle {
        font_family: family,
        font_size: num_kv(v, "fontSize", "font-size", 14.0),
        weight: weight.clamp(1, 1000),
        italic: get(v, "fontStyle", "font-style") == "italic",
        letter_spacing: Length::Px(num_kv(v, "letterSpacing", "letter-spacing", 0.0)),
        line_height: line_height(v),
        underline: (decoration == "underline").then(Decoration::default),
        strikethrough: (decoration == "line-through").then(Decoration::default),
        case: match get(v, "textTransform", "text-transform") {
            "uppercase" => TextCase::Upper,
            "lowercase" => TextCase::Lower,
            "capitalize" => TextCase::Title,
            _ => TextCase::Original,
        },
        ..TextStyle::default()
    })
}

/// An asset's name with its group path, as Ondin's slash groups.
fn asset_name(v: &Value) -> String {
    let path = str_of(v, "path").trim_matches('/');
    let name = str_of(v, "name");
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}/{name}")
    }
}

fn blend_mode(s: &str) -> BlendMode {
    match s {
        "darken" => BlendMode::Darken,
        "multiply" => BlendMode::Multiply,
        "color-burn" => BlendMode::ColorBurn,
        "lighten" => BlendMode::Lighten,
        "screen" => BlendMode::Screen,
        "color-dodge" => BlendMode::ColorDodge,
        "overlay" => BlendMode::Overlay,
        "soft-light" => BlendMode::SoftLight,
        "hard-light" => BlendMode::HardLight,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        "hue" => BlendMode::Hue,
        "saturation" => BlendMode::Saturation,
        "color" => BlendMode::Color,
        "luminosity" => BlendMode::Luminosity,
        _ => BlendMode::Normal,
    }
}
