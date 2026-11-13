//! The verb registry: every document edit a caller outside the canvas can name — an
//! MCP tool, a Command Mode line, a headless script — with typed parameters, in one
//! list. A verb reads the document and answers a [`Transaction`] and the ids it minted;
//! the caller commits it, as every edit is committed.
//!
//! Names are the public ones MCP and Command Mode use, so they are spelled for an
//! agent (`set_fills`, `boolean`), not after the builder behind them. Arguments
//! arrive as a JSON object; nodes as their wire form (`"<actor-hex>:<seq>"`), model
//! values (a fill list, a transform) in their serde form.

use crate::build::{self, Axis, Edge, ZMove};
use crate::container::{Display, Insets, LayoutItem};
use crate::document::Document;
use crate::effect::Effect;
use crate::export::ExportSpec;
use crate::id::{IdSource, NodeId};
use crate::item::{Keyed, PastTheEnd, rekey_by_position};
use crate::node::{
    BlendMode, BlockStyle, BoolOp, CharSpans, Fill, NodeKind, ParagraphStyle, Pivot, Stroke,
    TextStyle,
};
use crate::op::{GeometryPatch, OpError, Operation, Transaction};
use crate::resolve::Resolved;
use crate::style::{StyleId, StyleSlot, StyleValue};
use crate::svg_in::ImageSink;
use crate::typography::{CharAttr, Length, ParaAttr};
use crate::variable::{Cell, CollectionId, ModeId, VarId, VarKind};
use kurbo::Affine;
use serde_json::{Map, Value};

mod loose;

/// What a parameter takes, for checking an argument before a verb runs and for
/// describing the verb to a caller (an MCP schema, Command Mode's completion).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// One node, by its wire form.
    Node,
    /// One or more nodes, as a list of wire forms.
    Nodes,
    Number,
    /// A whole number, zero or more.
    Index,
    Bool,
    Text,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// A model value in its serde form; the name says which type.
    Model(&'static str),
    /// The id of a thing that is not a layer, in the node wire form; the name says
    /// which: a variable, a mode, a collection, a style.
    Id(&'static str),
}

/// One parameter of a [`Verb`].
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    pub doc: &'static str,
}

/// A required parameter.
const fn req(name: &'static str, kind: ParamKind, doc: &'static str) -> Param {
    Param {
        name,
        kind,
        required: true,
        doc,
    }
}

/// An optional parameter.
const fn opt(name: &'static str, kind: ParamKind, doc: &'static str) -> Param {
    Param {
        name,
        kind,
        required: false,
        doc,
    }
}

/// What a verb reads and mints with.
pub struct Ctx<'a> {
    pub doc: &'a Document,
    pub res: &'a Resolved,
    pub ids: &'a mut IdSource,
    /// What hashes and sizes a picture's bytes, which core cannot do; `None`
    /// refuses `add_image`.
    pub images: Option<&'a mut dyn ImageSink>,
    /// What measures contrast against a rendered backdrop, which core cannot do;
    /// `None` refuses `fix_contrast`.
    pub contrast: Option<&'a mut dyn crate::contrast::Probe>,
}

/// What a caller lends a script's verbs that core cannot do itself.
#[derive(Default)]
pub struct Lent<'a> {
    pub images: Option<&'a mut dyn ImageSink>,
    pub contrast: Option<&'a mut dyn crate::contrast::Probe>,
}

/// A verb's answer: the edit to commit, the nodes it creates, in order, the id
/// of what else it made (a picture, a variable, a mode, a style) as a caller
/// names it, and a line on what it did.
#[derive(Debug, Default)]
pub struct Done {
    pub tx: Transaction,
    pub created: Vec<NodeId>,
    pub made: Option<String>,
    /// A line on what the edit did that its caller cannot read off the document:
    /// how many matches a replace wrote, what an import left out.
    pub said: Option<String>,
}

impl Done {
    fn edit(tx: Transaction) -> Self {
        Self {
            tx,
            ..Self::default()
        }
    }

    fn ops(ops: Vec<Operation>) -> Self {
        Self::edit(Transaction(ops))
    }

    fn made(tx: Transaction, created: Vec<NodeId>) -> Self {
        Self {
            tx,
            created,
            ..Self::default()
        }
    }

    /// `self`, with `said` saying what it did.
    fn saying(self, said: String) -> Self {
        Self {
            said: Some(said),
            ..self
        }
    }

    /// `ops`, which make the thing whose id is `made`.
    fn making(ops: Vec<Operation>, made: String) -> Self {
        Self {
            made: Some(made),
            ..Self::ops(ops)
        }
    }
}

/// Why a verb did not answer an edit.
#[derive(thiserror::Error, Debug)]
pub enum VerbError {
    #[error("no verb is called `{0}`")]
    Unknown(String),
    #[error("`{verb}` needs `{param}`")]
    Missing {
        verb: &'static str,
        param: &'static str,
    },
    #[error("`{verb}` takes no `{param}`")]
    Unexpected { verb: &'static str, param: String },
    #[error("`{param}` of `{verb}`: {why}")]
    Invalid {
        verb: &'static str,
        param: &'static str,
        why: String,
    },
    #[error("the arguments of `{0}` must be an object")]
    NotAnObject(&'static str),
    #[error(transparent)]
    Refused(#[from] OpError),
    #[error("{0}")]
    Nothing(&'static str),
}

/// One verb: its public name, a line on what it does, its parameters, and the
/// function that answers it.
pub struct Verb {
    pub name: &'static str,
    pub doc: &'static str,
    pub params: &'static [Param],
    run: fn(&mut Ctx<'_>, &Args<'_>) -> Result<Done, VerbError>,
}

impl std::fmt::Debug for Verb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verb").field("name", &self.name).finish()
    }
}

/// A verb's arguments, checked against its parameters.
pub struct Args<'a> {
    verb: &'static Verb,
    map: &'a Map<String, Value>,
}

/// Parameter `p`'s value `v` with its short forms expanded, as it is read; a tree's
/// `like`s left out, since only the verb can see the layers they name. A design
/// tokens file is read as written: its `$value`s are its own grammar.
fn loosened<'v>(p: &Param, v: &'v Value) -> std::borrow::Cow<'v, Value> {
    match p.kind {
        ParamKind::Model("DtcgFile") => std::borrow::Cow::Borrowed(v),
        ParamKind::Model(model) => {
            let mut v = v.clone();
            if model == "Tree" {
                fn unlike(v: &mut Value) {
                    if let Value::Object(map) = v {
                        map.remove("like");
                        if let Some(Value::Array(children)) = map.get_mut("children") {
                            children.iter_mut().for_each(unlike);
                        }
                    }
                }
                unlike(&mut v);
            }
            loose::loosen(model_key(model, p.name), &mut v);
            std::borrow::Cow::Owned(v)
        }
        _ => std::borrow::Cow::Borrowed(v),
    }
}

/// `value` checked against the parameters of the tool called `name`: an object
/// holding every required one, no other, each of its kind. MCP's own tools, which
/// are not verbs, are checked by it too.
pub fn check_args<'a>(
    name: &'static str,
    params: &[Param],
    value: &'a Value,
) -> Result<&'a Map<String, Value>, VerbError> {
    let map = match value {
        Value::Object(map) => map,
        Value::Null if params.iter().all(|p| !p.required) => {
            static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
            EMPTY.get_or_init(Map::new)
        }
        _ => return Err(VerbError::NotAnObject(name)),
    };
    if let Some(extra) = map.keys().find(|k| !params.iter().any(|p| p.name == *k)) {
        return Err(VerbError::Unexpected {
            verb: name,
            param: extra.clone(),
        });
    }
    for p in params {
        match map.get(p.name) {
            None | Some(Value::Null) if p.required => {
                return Err(VerbError::Missing {
                    verb: name,
                    param: p.name,
                });
            }
            None | Some(Value::Null) => {}
            Some(v) => check_kind(p.kind, &loosened(p, v)).map_err(|why| VerbError::Invalid {
                verb: name,
                param: p.name,
                why,
            })?,
        }
    }
    Ok(map)
}

impl<'a> Args<'a> {
    /// `value` checked against `verb`'s parameters: an object holding every required
    /// one, no other, each of its kind.
    fn check(verb: &'static Verb, value: &'a Value) -> Result<Self, VerbError> {
        let map = check_args(verb.name, verb.params, value)?;
        Ok(Self { verb, map })
    }

    fn invalid(&self, param: &'static str, why: impl ToString) -> VerbError {
        VerbError::Invalid {
            verb: self.verb.name,
            param,
            why: why.to_string(),
        }
    }

    fn get(&self, name: &'static str) -> Option<&Value> {
        self.map.get(name).filter(|v| !v.is_null())
    }

    /// The `Node` parameter `name`.
    pub fn node(&self, name: &'static str) -> Result<NodeId, VerbError> {
        let v = self.get(name).ok_or(VerbError::Missing {
            verb: self.verb.name,
            param: name,
        })?;
        wire(v).map_err(|why| self.invalid(name, why))
    }

    /// The `Node` parameter `name`, refused when it names a page: a page has no
    /// placement, opacity, eye, lock, blend or export a user can see or clear.
    pub fn layer(&self, doc: &Document, name: &'static str) -> Result<NodeId, VerbError> {
        let id = self.node(name)?;
        match doc.is_page(id) {
            true => Err(self.invalid(name, "a page is not a layer")),
            false => Ok(id),
        }
    }

    /// The `Nodes` parameter `name`, refused when any names a page, as [`Self::layer`].
    pub fn layers(&self, doc: &Document, name: &'static str) -> Result<Vec<NodeId>, VerbError> {
        let ids = self.nodes(name)?;
        match ids.iter().any(|id| doc.is_page(*id)) {
            true => Err(self.invalid(name, "a page is not a layer")),
            false => Ok(ids),
        }
    }

    /// The `Nodes` parameter `name`.
    pub fn nodes(&self, name: &'static str) -> Result<Vec<NodeId>, VerbError> {
        let Some(Value::Array(list)) = self.get(name) else {
            return Err(self.invalid(name, "expected a list of node ids"));
        };
        list.iter()
            .map(|v| wire(v).map_err(|why| self.invalid(name, why)))
            .collect()
    }

    /// The `Number` parameter `name`, or `None` when an optional one is absent.
    pub fn number(&self, name: &'static str) -> Option<f64> {
        self.get(name).and_then(Value::as_f64)
    }

    /// The `Index` parameter `name`, or `None` when an optional one is absent.
    pub fn index(&self, name: &'static str) -> Option<usize> {
        self.get(name)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    }

    /// The `Bool` parameter `name`, or `None` when an optional one is absent.
    pub fn bool(&self, name: &'static str) -> Option<bool> {
        self.get(name).and_then(Value::as_bool)
    }

    /// The `Text` parameter `name`, or `None` when an optional one is absent.
    pub fn text(&self, name: &'static str) -> Option<&str> {
        self.get(name).and_then(Value::as_str)
    }

    /// The `Choice` parameter `name`, or `None` when an optional one is absent.
    pub fn choice(&self, name: &'static str) -> Option<&str> {
        self.text(name)
    }

    /// The `Model` parameter `name`, deserialized from its short forms (hex
    /// colours, paints without their default fields) as well as its own shape.
    pub fn model<T: serde::de::DeserializeOwned>(
        &self,
        name: &'static str,
    ) -> Result<T, VerbError> {
        let v = self.get(name).ok_or(VerbError::Missing {
            verb: self.verb.name,
            param: name,
        })?;
        let v = match self.verb.params.iter().find(|p| p.name == name) {
            Some(p) => loosened(p, v).into_owned(),
            None => v.clone(),
        };
        serde_json::from_value(v).map_err(|e| self.invalid(name, e))
    }

    /// The `Id` parameter `name`, as the id type it names.
    pub fn id<T: serde::de::DeserializeOwned>(&self, name: &'static str) -> Result<T, VerbError> {
        self.model(name)
    }

    /// The `Id` parameter `name` when given.
    pub fn id_opt<T: serde::de::DeserializeOwned>(
        &self,
        name: &'static str,
    ) -> Result<Option<T>, VerbError> {
        self.model_opt(name)
    }

    /// The `Model` parameter `name` when given.
    pub fn model_opt<T: serde::de::DeserializeOwned>(
        &self,
        name: &'static str,
    ) -> Result<Option<T>, VerbError> {
        match self.get(name) {
            None => Ok(None),
            Some(_) => self.model(name).map(Some),
        }
    }
}

/// A node id from its wire form.
fn wire(v: &Value) -> Result<NodeId, String> {
    let s = v.as_str().ok_or("expected a node id")?;
    NodeId::from_wire(s).ok_or_else(|| format!("`{s}` is not a node id"))
}

/// Whether `v` is of `kind`, or why not.
fn check_kind(kind: ParamKind, v: &Value) -> Result<(), String> {
    match kind {
        ParamKind::Node => wire(v).map(|_| ()),
        ParamKind::Nodes => match v {
            Value::Array(list) if !list.is_empty() => {
                list.iter().try_for_each(|v| wire(v).map(|_| ()))
            }
            _ => Err("expected a non-empty list of node ids".into()),
        },
        ParamKind::Number => match v.as_f64() {
            Some(n) if n.is_finite() => Ok(()),
            _ => Err("expected a number".into()),
        },
        ParamKind::Index => v
            .as_u64()
            .map(|_| ())
            .ok_or_else(|| "expected a whole number".into()),
        ParamKind::Bool => v
            .as_bool()
            .map(|_| ())
            .ok_or_else(|| "expected true or false".into()),
        ParamKind::Text => v
            .as_str()
            .map(|_| ())
            .ok_or_else(|| "expected a string".into()),
        ParamKind::Choice(words) => match v.as_str() {
            Some(s) if words.contains(&s) => Ok(()),
            _ => Err(format!("expected one of {}", words.join(", "))),
        },
        ParamKind::Model(model) => check_model(model, v),
        ParamKind::Id(what) => wire(v)
            .map(|_| ())
            .map_err(|_| format!("expected a {what}'s id")),
    }
}

/// Whether `v`, in its own shape or the short forms, reads as the model type
/// `model` names (a [`ParamKind::Model`]'s name), or why not.
pub fn check_model(model: &str, v: &Value) -> Result<(), String> {
    fn reads<T: serde::de::DeserializeOwned>(v: &Value) -> Result<(), String> {
        read_model::<T>(v).map(|_| ())
    }
    let mut loose = v.clone();
    if model != "DtcgFile" {
        loose::loosen(model_key(model, ""), &mut loose);
    }
    let v = &loose;
    match model {
        "Affine" => reads::<Affine>(v),
        "BlockStyle" => reads::<BlockStyle>(v),
        "CharSpans" => reads::<CharSpans>(v),
        "Display" => reads::<Display>(v),
        "GeometryPatch" => reads::<GeometryPatch>(v),
        "Insets" => reads::<Insets>(v),
        "LayoutItem" => reads::<LayoutItem>(v),
        "NodeKind" => reads::<NodeKind>(v),
        "ParagraphStyle" => reads::<ParagraphStyle>(v),
        "Pivot" => reads::<Pivot>(v),
        "TextStyle" => reads::<TextStyle>(v),
        "[CharAttr]" => reads::<Vec<CharAttr>>(v),
        "[ParaAttr]" => reads::<Vec<ParaAttr>>(v),
        "Cell" => reads::<Cell>(v),
        "StyleValue" => reads::<StyleInput>(v),
        "BoundField" => reads::<build::BoundField>(v),
        "Color" => reads::<peniko::Color>(v),
        "[Hex]" => reads::<Vec<String>>(v),
        "Scope" => reads::<Vec<String>>(v),
        // A list of node ids that may be empty, where `Nodes` may not.
        "[Node]" => match v {
            Value::Array(list) => list.iter().try_for_each(|v| wire(v).map(|_| ())),
            _ => Err("expected a list of node ids".into()),
        },
        "PropValue" => match v {
            Value::Bool(_) | Value::String(_) => Ok(()),
            _ => Err("expected true, false or a string".into()),
        },
        "Tree" => reads::<Tree>(v),
        "[Effect]" => reads::<Vec<Effect>>(v),
        "[ExportSpec]" => reads::<Vec<ExportSpec>>(v),
        "[Fill]" => reads::<Vec<Fill>>(v),
        "[Stroke]" => reads::<Vec<Stroke>>(v),
        "ChartTable" => reads::<crate::chart::Table>(v),
        // Patches, whose nulls reset what they name (`patched`).
        "ChartMapping" => reads::<crate::chart::Mapping>(&without_nulls(v)),
        "ChartOptions" => reads::<crate::chart::Options>(&without_nulls(v)),
        // MCP's own tools: their handlers read the rest.
        "KitModeMap" if v.is_object() => Ok(()),
        "KitModeMap" => Err("expected an object of mode ids".into()),
        "DtcgFile" if v.is_object() || v.is_string() => Ok(()),
        "DtcgFile" => Err("expected the tokens file, as text or as an object".into()),
        "Steps" if v.as_array().is_some_and(|s| s.iter().all(Value::is_object)) => Ok(()),
        "Steps" => Err("expected a list of steps, each an object".into()),
        "Version" if v.is_u64() || v.is_string() => Ok(()),
        "Version" => Err("expected a version's `at` or its name".into()),
        "Region"
            if v.as_array()
                .is_some_and(|r| r.len() == 4 && r.iter().all(Value::is_number)) =>
        {
            Ok(())
        }
        "Region" => Err("expected [x, y, w, h]".into()),
        "Props" | "Args" if v.is_object() => Ok(()),
        "Props" | "Args" => Err("expected an object".into()),
        "[Name]" => reads::<Vec<String>>(v),
        _ => Err(format!("no model type is called `{model}`")),
    }
}

/// `v` read as a `T`, refused naming where it went wrong: the path of the value
/// serde refused, or of a field `T` has no place for, which serde alone would
/// drop without a word (`Flex { gap }` laid out with no gap).
pub(crate) fn read_model<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, String> {
    let mut unknown = Vec::new();
    let mut note = |path: serde_ignored::Path<'_>| unknown.push(ignored_steps(&path));
    let de = serde_ignored::Deserializer::new(v, &mut note);
    let t = serde_path_to_error::deserialize(de).map_err(|e| {
        let steps = e.path().iter().map(|s| {
            use serde_path_to_error::Segment;
            match s {
                Segment::Seq { index } => Some(Step::Index(*index)),
                Segment::Map { key } => Some(Step::Key(key.clone())),
                Segment::Enum { variant } => Some(Step::Key(variant.clone())),
                Segment::Unknown => None,
            }
        });
        match json_path(v, steps.collect()) {
            p if p.is_empty() => e.into_inner().to_string(),
            p => format!("at `{p}`: {}", e.into_inner()),
        }
    })?;
    match unknown.into_iter().next() {
        None => Ok(t),
        Some(steps) => Err(format!("at `{}`: no such field", json_path(v, steps))),
    }
}

/// One step down a JSON value: a member by its key or an item by its place.
#[derive(Clone)]
enum Step {
    Key(String),
    Index(usize),
}

/// The steps of a path `serde_ignored` reports, `None` where it cannot name one
/// (an enum's variant).
fn ignored_steps(path: &serde_ignored::Path<'_>) -> Vec<Option<Step>> {
    use serde_ignored::Path;
    let (parent, step) = match path {
        Path::Root => return Vec::new(),
        Path::Seq { parent, index } => (*parent, Some(Step::Index(*index))),
        Path::Map { parent, key } => (*parent, Some(Step::Key(key.clone()))),
        Path::Some { parent } | Path::NewtypeStruct { parent } => return ignored_steps(parent),
        Path::NewtypeVariant { parent } => (*parent, None),
    };
    let mut steps = ignored_steps(parent);
    steps.push(step);
    steps
}

/// `steps` written against `v` as `children[1].kind.Text`, an unnamed step taken
/// as the one key of an externally tagged enum, which is how the value spells it.
fn json_path(v: &Value, steps: Vec<Option<Step>>) -> String {
    let mut out = String::new();
    let mut at = Some(v);
    for step in steps {
        let step = match step {
            Some(s) => s,
            None => match at.and_then(Value::as_object) {
                Some(map) if map.len() == 1 => {
                    Step::Key(map.keys().next().cloned().unwrap_or_default())
                }
                _ => continue,
            },
        };
        match &step {
            Step::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
                at = at.and_then(|v| v.get(k));
            }
            Step::Index(i) => {
                out.push_str(&format!("[{i}]"));
                at = at.and_then(|v| v.get(*i));
            }
        }
    }
    out
}

/// `v` with every null member of an object left out.
fn without_nulls(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), without_nulls(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A value of the model type `model` names, in the form a parameter of it takes:
/// what a caller is shown, since a serde form cannot be guessed (a rectangle
/// needs its `corner_radii`, an image brush its `sampler`). `None` for a name no
/// parameter uses.
pub fn example(model: &str) -> Option<Value> {
    use crate::container::Flex;
    use crate::effect::{EffectKind, Shadow};
    use crate::export::{ExportFormat, ExportScale};
    use crate::typography::CharAttr;
    use kurbo::{Size, Vec2};
    use peniko::Color;

    fn json<T: serde::Serialize>(v: &T) -> Value {
        serde_json::to_value(v).unwrap_or_default()
    }
    let rect = NodeKind::Rect {
        size: Size::new(120.0, 80.0),
        corner_radii: kurbo::RoundedRectRadii::from_single_radius(8.0),
        corner_smoothing: 0.0,
    };
    let fill = Fill {
        brush: peniko::Brush::Solid(Color::from_rgb8(0x33, 0x66, 0xcc)).into(),
        visible: true,
    };
    let image_fill = Fill {
        brush: crate::image::image_brush(crate::image::ImageId("<add_image's id>".into())).into(),
        visible: true,
    };
    let flex = Display::Flex(Flex::default());
    Some(match model {
        "Affine" => json(&Affine::translate((40.0, 24.0))),
        "BlockStyle" => json(&BlockStyle::default()),
        "CharSpans" => {
            let mut spans = CharSpans::default();
            spans.set(0..5, CharAttr::Weight(700), &TextStyle::default());
            json(&spans)
        }
        "Display" => json(&flex),
        "GeometryPatch" => json(&GeometryPatch::Size(Size::new(200.0, 100.0))),
        "Insets" => json(&Insets::default()),
        "LayoutItem" => json(&LayoutItem::default()),
        "NodeKind" => json(&rect),
        "ParagraphStyle" => json(&ParagraphStyle::default()),
        "Pivot" => json(&Pivot::Normalized(Vec2::new(0.5, 0.5))),
        "TextStyle" => json(&TextStyle::default()),
        "[CharAttr]" => json(&vec![
            CharAttr::Weight(700),
            CharAttr::Size(24.0),
            CharAttr::LetterSpacing(Length::Em(0.02)),
        ]),
        "Cell" => json(&Cell::Color(Color::from_rgb8(0x33, 0x66, 0xcc))),
        "StyleValue" => serde_json::json!({ "fills": [json(&fill)] }),
        "BoundField" => json(&build::BoundField::Fill(0)),
        "Color" => json(&Color::from_rgb8(0x1f, 0x3a, 0x5f)),
        "[Hex]" => serde_json::json!(["#203b5eff", "#1e395fff"]),
        "[Node]" => serde_json::json!(["7e5b:12", "7e5b:40"]),
        "Props" => serde_json::json!({
            "name": "Title",
            "fills": ["#12352a"],
            "size": { "width": 320.0, "height": 48.0 },
            "at": [120.0, 96.0],
        }),
        "Scope" => serde_json::json!(["corner", "spacing"]),
        "PropValue" => Value::Bool(true),
        "[ParaAttr]" => json(&vec![
            ParaAttr::Spacing(Length::Px(12.0)),
            ParaAttr::IndentStart(Length::Px(24.0)),
        ]),
        // Written in the short forms, so the example shows them.
        "Tree" => serde_json::json!({
            "kind": { "Artboard": { "size": { "width": 320, "height": 200 } } },
            "name": "Card",
            "fills": ["#ffffff"],
            "display": { "Flex": { "direction": "Column", "gap": 12, "padding": 16 } },
            "children": [
                {
                    "kind": { "Rect": { "size": { "width": 288, "height": 96 }, "corner_radii": 8 } },
                    "name": "Picture",
                    "fills": ["#3366cc"],
                    "effects": [{ "kind": { "type": "drop_shadow", "offset": [0, 6], "blur": 14, "spread": 0, "color": "#12352a2e" } }],
                },
                {
                    "kind": { "Text": {
                        "content": "Hello",
                        "sizing": { "AutoHeight": 288 },
                        "paragraph": { "align": "Center" },
                        "style": { "font_family": "Inter", "font_size": 16, "weight": 600, "line_height": "22px" },
                    } },
                    "fills": ["#12352a"],
                },
            ],
        }),
        "[Effect]" => json(&vec![Effect::new(
            EffectKind::DropShadow(Shadow::default()),
        )]),
        "[ExportSpec]" => json(&vec![ExportSpec::new(
            ExportFormat::Png,
            ExportScale::default(),
        )]),
        "[Fill]" => json(&vec![fill, image_fill]),
        "[Stroke]" => json(&vec![Stroke::default()]),
        "ChartTable" => json(&crate::chart::Chart::sample(crate::chart::ChartKind::Column).table),
        "ChartMapping" => {
            json(&crate::chart::Chart::sample(crate::chart::ChartKind::Column).mapping)
        }
        "ChartOptions" => serde_json::json!({
            "bars": { "stacking": "Stacked", "radius": 4.0 },
            "legend": { "position": "Bottom" },
        }),
        _ => return None,
    })
    .map(|mut v| {
        // Shown in the short forms, which are the ones worth writing.
        loose::tighten(model_key(model, ""), &mut v);
        v
    })
}

/// The field name the short forms read a model's value under: its own kind's for
/// a colour or paint list, whatever its parameter is called, else the parameter's.
fn model_key<'a>(model: &str, param: &'a str) -> &'a str {
    match model {
        "Color" => "color",
        "TextStyle" => "style",
        "[Fill]" => "fills",
        "[Stroke]" => "strokes",
        _ => param,
    }
}

/// `n` with the noun that counts it: "1 match", "3 matches".
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The verb called `name`.
pub fn find(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.name == name)
}

/// Check `args` against the verb called `name` and answer its edit.
pub fn call(ctx: &mut Ctx<'_>, name: &str, args: &Value) -> Result<Done, VerbError> {
    let verb = find(name).ok_or_else(|| VerbError::Unknown(name.to_owned()))?;
    let args = Args::check(verb, args)?;
    (verb.run)(ctx, &args)
}

const NODE: ParamKind = ParamKind::Node;
const NODES: ParamKind = ParamKind::Nodes;
const BOOL: ParamKind = ParamKind::Bool;
const TEXT: ParamKind = ParamKind::Text;
const INDEX: ParamKind = ParamKind::Index;

const BOOL_OPS: &[&str] = &["union", "subtract", "intersect", "exclude"];
const EDGES: &[&str] = &["left", "center", "right", "top", "middle", "bottom"];
const AXES: &[&str] = &["horizontal", "vertical"];
const VAR_KINDS: &[&str] = &["color", "number", "boolean", "string"];
const RESETS: &[&str] = &["all", "fields", "children", "order"];
const PUSHES: &[&str] = &["all", "properties", "fields", "layers", "order", "removals"];
const LEVELS: &[&str] = &["aa", "aaa", "apca"];
const PROP_KINDS: &[&str] = &["boolean", "text", "swap"];
/// The mask modes by the word a verb takes, in [`crate::node::MaskMode::ALL`]'s order.
const MASK_MODES: &[&str] = &["shape", "alpha", "luminance"];
/// The HTML elements by the tag a verb takes them as.
const ELEMENTS: &[&str] = &[
    "main", "section", "article", "aside", "header", "footer", "nav", "ul", "button", "a", "h1",
    "h2", "h3", "h4", "h5", "h6",
];
/// The browser states a variant value can be tied to, as `variant::Interaction`
/// writes them.
const STATES: &[&str] = &["hover", "focus-visible", "active", "disabled"];
const STYLE_SLOTS: &[&str] = &["fills", "strokes", "effects", "text", "layout"];

/// The style slot a [`STYLE_SLOTS`] word names.
fn slot_of(word: &str) -> StyleSlot {
    match word {
        "strokes" => StyleSlot::Strokes,
        "effects" => StyleSlot::Effects,
        "text" => StyleSlot::Text,
        "layout" => StyleSlot::Layout,
        _ => StyleSlot::Fills,
    }
}
const MOVES: &[&str] = &["front", "forward", "backward", "back"];
/// The blend modes by [`BlendMode::label`], as the snapshot names them, in
/// [`BlendMode::ALL`]'s order.
pub const BLENDS: &[&str] = &[
    "Pass through",
    "Normal",
    "Darken",
    "Multiply",
    "Color burn",
    "Lighten",
    "Screen",
    "Color dodge",
    "Overlay",
    "Soft light",
    "Hard light",
    "Difference",
    "Exclusion",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];

/// The layer named `id`, or [`OpError::NoSuchNode`].
fn node<'d>(ctx: &Ctx<'d>, id: NodeId) -> Result<&'d crate::node::Node, VerbError> {
    ctx.doc
        .get(id)
        .ok_or(VerbError::Refused(OpError::NoSuchNode(id)))
}

/// The layers a style verb's `nodes` names, each checked to exist: a paint
/// slot's [`crate::style::targets`] drops a missing id, which would otherwise
/// answer as done with nothing bound.
fn style_nodes(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Vec<NodeId>, VerbError> {
    let nodes = a.nodes("nodes")?;
    for id in &nodes {
        node(ctx, *id)?;
    }
    Ok(nodes)
}

/// `set_point`: one anchor of a path edited as the inspector's point fields edit
/// it, read and written in the layer's own space. A coordinate left out keeps
/// its value, a handle is placed after the point moves, and one handle given
/// mirrors the other only on a smooth point. The whole path is written back
/// through `edit::rewrite_points`, which keeps a mirrored path symmetric.
fn set_point(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    use crate::edit::Side;
    const EDITS: [&str; 7] = ["x", "y", "in_x", "in_y", "out_x", "out_y", "radius"];
    let id = a.layer(ctx.doc, "node")?;
    node(ctx, id)?;
    let (world, base) = crate::edit::path_subpaths(ctx.doc, ctx.res, id)
        .ok_or_else(|| a.invalid("node", "not a path"))?;
    let sub = a.index("subpath").unwrap_or(0);
    let count = base
        .get(sub)
        .ok_or_else(|| a.invalid("subpath", format!("the path has {} subpaths", base.len())))?
        .anchors
        .len();
    let at = a.index("point").unwrap_or(0);
    if at >= count {
        return Err(a.invalid("point", format!("subpath {sub} has {count} points")));
    }
    if EDITS.iter().all(|p| a.number(p).is_none()) {
        return Err(VerbError::Nothing(
            "Give the point a place, a handle or a radius",
        ));
    }
    let radius = a.number("radius");
    if radius.is_some_and(|r| r < 0.0) {
        return Err(a.invalid("radius", "a radius is not negative"));
    }
    let to_local = world.inverse();
    // Absent coordinates fall back to `now`, in the layer's space.
    let place = |x: &'static str, y: &'static str, now: kurbo::Point| {
        let now = to_local * now;
        (a.number(x).is_some() || a.number(y).is_some()).then(|| {
            world * kurbo::Point::new(a.number(x).unwrap_or(now.x), a.number(y).unwrap_or(now.y))
        })
    };
    let mut subpaths = base.clone();
    let an = &mut subpaths[sub].anchors[at];
    if let Some(p) = place("x", "y", an.at) {
        an.at = p;
    }
    let smooth = an.is_smooth();
    let aim_in = place("in_x", "in_y", an.arriving());
    let aim_out = place("out_x", "out_y", an.leaving());
    match (aim_in, aim_out) {
        (Some(i), Some(o)) => {
            an.back = i - an.at;
            an.out = o - an.at;
        }
        (Some(i), None) => an.pull_side(Side::Back, i, !smooth),
        (None, Some(o)) => an.pull_side(Side::Out, o, !smooth),
        (None, None) => {}
    }
    if let Some(r) = radius {
        an.radius = r;
    }
    Ok(Done::edit(crate::edit::rewrite_points(
        ctx.doc, ctx.res, id, world, &base, subpaths,
    )))
}

/// Whether `patch` writes the drawn outline of the rounded path `id` back as its
/// points, which would bake each fillet into curves and round it again.
///
/// The snapshot's `svg` is that outline, so it is the likely mistake. A patch is
/// caught by any one point the outline has only because a corner is rounded, so
/// the outline written back with an edit is caught too. Four decimal places is the
/// snapshot's precision; the tolerance sits above it.
fn bakes_rounding(ctx: &Ctx<'_>, id: NodeId, patch: &GeometryPatch) -> bool {
    const NEAR: f64 = 1e-3;
    let GeometryPatch::Path { path: written, .. } = patch else {
        return false;
    };
    let Some(
        kind @ NodeKind::Path {
            path, corner_radii, ..
        },
    ) = ctx.res.used_kind(ctx.doc, id)
    else {
        return false;
    };
    if corner_radii.iter().all(|r| *r <= 0.0) {
        return false;
    }
    let points = |p: &kurbo::BezPath, radii: &[f64]| -> Vec<kurbo::Point> {
        crate::edit::pen_anchors(p, radii)
            .iter()
            .flat_map(|s| s.anchors.iter().map(|a| a.at))
            .collect()
    };
    let near =
        |list: &[kurbo::Point], p: kurbo::Point| list.iter().any(|q| (*q - p).hypot() < NEAR);
    let anchors = points(path, corner_radii);
    let drawn = crate::geometry::local_path(kind).unwrap_or_default();
    let fillets: Vec<kurbo::Point> = points(&drawn, &[])
        .into_iter()
        .filter(|p| !near(&anchors, *p))
        .collect();
    points(written, &[]).into_iter().any(|p| near(&fillets, p))
}

/// A style's value as a caller writes it: the lists as the single-layer verbs take
/// them, with no item ids, which the verb keys.
#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum StyleInput {
    Fills(Vec<Fill>),
    Strokes(Vec<Stroke>),
    Effects(Vec<Effect>),
    Text(Box<TextStyle>),
    Layout(Box<crate::style::LayoutStyle>),
}

/// `input` as a style holds it, each list position keeping the item id `own`'s
/// list holds there (fresh ids past it, or for a new style), and a text style
/// with its colour and language cleared (`style::type_part`).
fn style_value(ctx: &mut Ctx<'_>, input: StyleInput, own: Option<&StyleValue>) -> StyleValue {
    match (input, own) {
        (StyleInput::Fills(v), Some(StyleValue::Fills(o))) => StyleValue::Fills(rekeyed(ctx, o, v)),
        (StyleInput::Fills(v), _) => StyleValue::Fills(rekeyed(ctx, &[], v)),
        (StyleInput::Strokes(v), Some(StyleValue::Strokes(o))) => {
            StyleValue::Strokes(rekeyed(ctx, o, v))
        }
        (StyleInput::Strokes(v), _) => StyleValue::Strokes(rekeyed(ctx, &[], v)),
        (StyleInput::Effects(v), Some(StyleValue::Effects(o))) => {
            StyleValue::Effects(rekeyed(ctx, o, v))
        }
        (StyleInput::Effects(v), _) => StyleValue::Effects(rekeyed(ctx, &[], v)),
        (StyleInput::Text(t), _) => StyleValue::Text(Box::new(crate::style::type_part(&t))),
        (StyleInput::Layout(l), _) => StyleValue::Layout(l),
    }
}

/// The style the `style` argument names, the document's own.
fn own_style<'d>(ctx: &Ctx<'d>, a: &Args<'_>) -> Result<&'d crate::style::Style, VerbError> {
    let id: StyleId = a.id("style")?;
    let st = ctx
        .doc
        .styles()
        .get(id)
        .ok_or_else(|| a.invalid("style", "no style has that id"))?;
    if st.origin.is_some() {
        return Err(a.invalid("style", "a kit's style is read-only here"));
    }
    Ok(st)
}

/// The variable the `variable` argument names, the document's own: a kit's is
/// written by an update only.
fn own_variable(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<VarId, VerbError> {
    let id: VarId = a.id("variable")?;
    match ctx.doc.variables().get(id) {
        None => Err(a.invalid("variable", "no variable has that id")),
        Some(v) if v.origin.is_some() => {
            Err(a.invalid("variable", "a kit's variable is read-only here"))
        }
        Some(_) => Ok(id),
    }
}

/// The mode the `mode` argument names, the document's own: a kit's is written by
/// an update only.
fn own_mode(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<ModeId, VerbError> {
    let mode: ModeId = a.id("mode")?;
    let table = ctx.doc.variables();
    if table.kit_mode_of(mode).is_some() {
        return Err(a.invalid("mode", "a kit's mode is read-only here"));
    }
    match table.mode(mode) {
        None => Err(a.invalid("mode", "no mode has that id")),
        Some(_) => Ok(mode),
    }
}

/// The collection the `collection` argument names, or `None` when it is not
/// given; refused when no collection has that id.
fn collection_arg(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Option<CollectionId>, VerbError> {
    match a.id_opt::<CollectionId>("collection")? {
        Some(c) if ctx.doc.variables().collection(c).is_none() => {
            Err(a.invalid("collection", "no collection has that id"))
        }
        c => Ok(c),
    }
}

/// The collection the required `collection` argument names.
fn collection_req(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<CollectionId, VerbError> {
    collection_arg(ctx, a)?.ok_or(VerbError::Missing {
        verb: a.verb.name,
        param: "collection",
    })
}

/// `name` trimmed as a collection's, refused when empty or another collection
/// than `own` has it.
fn collection_name(
    ctx: &Ctx<'_>,
    a: &Args<'_>,
    name: &str,
    own: Option<CollectionId>,
) -> Result<String, VerbError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(a.invalid("name", "a collection needs a name"));
    }
    let taken = ctx
        .doc
        .variables()
        .collections()
        .iter()
        .any(|c| Some(c.id) != own && c.name == name);
    match taken {
        true => Err(a.invalid("name", format!("{name} already exists"))),
        false => Ok(name.to_owned()),
    }
}

/// Why layer `id`'s element and link are not its own to set, as the identity
/// card's Element row dims: a layer inside an instance takes its main's, and a
/// shape takes none.
fn element_refusal(ctx: &Ctx<'_>, a: &Args<'_>, id: NodeId) -> Result<(), VerbError> {
    let n = node(ctx, id)?;
    let main = n
        .link()
        .and_then(|_| crate::component::instance_root(ctx.doc, id))
        .and_then(|r| crate::component::main_of(ctx.doc, r))
        .and_then(|m| ctx.doc.get(m));
    if let Some(m) = main {
        let why = format!("set on the main component, “{}”", m.name());
        return Err(a.invalid("node", why));
    }
    match crate::element::kind_refusal(n.kind()) {
        Some(why) => Err(a.invalid("node", why)),
        None => Ok(()),
    }
}

/// The main component or set the `node` argument names, which alone carry a
/// description and a documentation link of their own.
fn component_owner(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<NodeId, VerbError> {
    let id = a.layer(ctx.doc, "node")?;
    let n = node(ctx, id)?;
    match n.component() || n.set().is_some() {
        true => Ok(id),
        false => Err(a.invalid("node", "neither a main component nor a component set")),
    }
}

/// What a variant verb says for a property name that is empty or taken.
const PROP_NAME_TAKEN: &str = "a property needs a name no other property of the set or main has";
/// What a variant verb says for a value name that is empty or taken.
const VALUE_NAME_TAKEN: &str = "a value needs a name no other value of the property has";

/// The layers the `nodes` argument names, each checked to exist: a bulk paint
/// write drops a missing id, which would otherwise answer as done.
fn existing_layers(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Vec<NodeId>, VerbError> {
    let ids = a.layers(ctx.doc, "nodes")?;
    for id in &ids {
        node(ctx, *id)?;
    }
    Ok(ids)
}

/// The layers the `nodes` argument names, refused when none of them reaches a
/// layer that takes paint (`build::paint_targets`): an empty group, or slices,
/// would otherwise answer as done with nothing painted.
fn painted(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Vec<NodeId>, VerbError> {
    let ids = existing_layers(ctx, a)?;
    match build::paint_targets(ctx.doc, &ids).is_empty() {
        true => Err(a.invalid(
            "nodes",
            "none of them takes paint: a group passes paint to the shapes inside it, and holds none",
        )),
        false => Ok(ids),
    }
}

/// The text layers the `nodes` argument names; any other layer is refused.
fn texts(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Vec<NodeId>, VerbError> {
    let ids = existing_layers(ctx, a)?;
    match ids.iter().find(|id| {
        !matches!(
            ctx.doc.get(**id).map(|n| n.kind()),
            Some(NodeKind::Text { .. })
        )
    }) {
        Some(id) => Err(a.invalid("nodes", format!("`{}` is not a text layer", id.to_wire()))),
        None => Ok(ids),
    }
}

/// The component set the `node` argument names: the set, or one of its variants.
fn set_arg(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<NodeId, VerbError> {
    let id = a.layer(ctx.doc, "node")?;
    node(ctx, id)?;
    match crate::variant::is_set(ctx.doc, id) {
        true => Ok(id),
        false => crate::variant::set_of(ctx.doc, id)
            .ok_or_else(|| a.invalid("node", "neither a component set nor a variant")),
    }
}

/// The variant property of `set` the `property` argument names, and its place.
fn variant_prop<'d>(
    ctx: &Ctx<'d>,
    a: &Args<'_>,
    set: NodeId,
) -> Result<(usize, &'d crate::variant::VariantProp), VerbError> {
    let vs = node(ctx, set)?
        .set()
        .ok_or_else(|| a.invalid("node", "not in a component set"))?;
    let name = a.text("property").unwrap_or_default();
    vs.props
        .iter()
        .enumerate()
        .find(|(_, p)| p.name == name)
        .ok_or_else(|| {
            let names: Vec<&str> = vs.props.iter().map(|p| p.name.as_str()).collect();
            a.invalid(
                "property",
                format!("the set's properties are {}", names.join(", ")),
            )
        })
}

/// The place of the value of `p` the `value` argument names.
fn variant_value(a: &Args<'_>, p: &crate::variant::VariantProp) -> Result<usize, VerbError> {
    let name = a.text("value").unwrap_or_default();
    p.values.iter().position(|v| v == name).ok_or_else(|| {
        a.invalid(
            "value",
            format!("{}'s values are {}", p.name, p.values.join(", ")),
        )
    })
}

/// The owner of the component property the `property` argument names — the
/// main or set the `node` argument names, or a variant's set — and the property.
/// A showing of nested properties is nameless, and never named here.
fn component_prop(
    ctx: &Ctx<'_>,
    a: &Args<'_>,
) -> Result<(NodeId, Keyed<crate::variant::Property>), VerbError> {
    let id = a.layer(ctx.doc, "node")?;
    node(ctx, id)?;
    let owner = crate::variant::owner_of_main(ctx.doc, id)
        .ok_or_else(|| a.invalid("node", "neither a main component nor a component set"))?;
    let props = node(ctx, owner)?.props();
    let name = a.text("property").unwrap_or_default();
    let named = |p: &&Keyed<crate::variant::Property>| {
        p.kind != crate::variant::PropKind::Nested && !p.name.is_empty()
    };
    props
        .iter()
        .filter(named)
        .find(|p| p.name == name)
        .map(|p| (owner, p.clone()))
        .ok_or_else(|| {
            let names: Vec<&str> = props
                .iter()
                .filter(named)
                .map(|p| p.name.as_str())
                .collect();
            a.invalid(
                "property",
                match names.as_slice() {
                    [] => "it has no component properties".to_owned(),
                    _ => format!("its properties are {}", names.join(", ")),
                },
            )
        })
}

/// What `make_component` says for a rule it refused by.
fn not_main(why: crate::component::NotMain) -> &'static str {
    use crate::component::NotMain;
    match why {
        NotMain::Kind => "only a frame or a group can become a component",
        NotMain::Already => "already a component",
        NotMain::Inside => "a component cannot sit inside a component or an instance",
        NotMain::Instance => "an instance cannot become a component; detach it first",
        NotMain::Holds => "a component cannot hold another component or a component set",
        NotMain::SetKind => {
            "a component set holds one kind of layer; its variants are another kind"
        }
    }
}

/// The text layer the `node` argument names, its styling and its text.
fn text_of<'d>(
    ctx: &Ctx<'d>,
    a: &Args<'_>,
) -> Result<(build::TextStyling<'d>, &'d str), VerbError> {
    build::TextStyling::of(node(ctx, a.node("node")?)?)
        .ok_or_else(|| a.invalid("node", "not a text layer"))
}

/// The byte range of `content` the `start` and `end` arguments name in characters,
/// and whether it is all of it.
fn char_range(a: &Args<'_>, content: &str) -> Result<(std::ops::Range<usize>, bool), VerbError> {
    let chars = content.chars().count();
    let (start, end) = (
        a.index("start").unwrap_or(0),
        a.index("end").unwrap_or(chars),
    );
    if start > end || end > chars {
        let why = format!("the text has {chars} characters");
        return Err(a.invalid(if end > chars { "end" } else { "start" }, why));
    }
    let byte = |n: usize| {
        content
            .char_indices()
            .nth(n)
            .map_or(content.len(), |(b, _)| b)
    };
    Ok((byte(start)..byte(end), start == 0 && end == chars))
}

/// A page's `name` trimmed, as the pages list keeps it; an empty one is refused.
fn page_name<'n>(a: &Args<'_>, name: &'n str) -> Result<&'n str, VerbError> {
    match name.trim() {
        "" => Err(a.invalid("name", "a page's name is not empty")),
        t => Ok(t),
    }
}

/// One op per layer of the `nodes` argument, built by `op`; a page is refused.
fn each(
    doc: &Document,
    args: &Args<'_>,
    op: impl Fn(NodeId) -> Operation,
) -> Result<Done, VerbError> {
    Ok(Done::ops(
        args.layers(doc, "nodes")?.into_iter().map(op).collect(),
    ))
}

/// A paint list written whole, each position keeping the item id the node's own
/// list holds there.
fn rekeyed<T>(ctx: &mut Ctx<'_>, own: &[Keyed<T>], values: Vec<T>) -> Vec<Keyed<T>> {
    rekey_by_position(own, values, &mut PastTheEnd::default(), ctx.ids)
}

/// Every verb, in the order a caller is shown them.
pub static VERBS: &[Verb] = &[
    // --- structure ---------------------------------------------------------
    Verb {
        name: "create_node",
        doc: "Create a layer of `kind` in `parent`, on top unless `index` says where",
        params: &[
            req("parent", NODE, "the frame, group or page to create it in"),
            req(
                "kind",
                ParamKind::Model("NodeKind"),
                "what the layer is, with its geometry",
            ),
            opt(
                "index",
                INDEX,
                "its place among the parent's children, 0 the bottom",
            ),
            opt(
                "transform",
                ParamKind::Model("Affine"),
                "its placement in the parent",
            ),
            opt("name", TEXT, "its name"),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let id = ctx.ids.mint();
            let tx = Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index: a.index("index").unwrap_or(count).min(count),
                kind: a.model::<NodeKind>("kind")?,
                transform: a.model_opt::<Affine>("transform")?,
                name: a.text("name").map(str::to_owned),
            }]);
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "create_text",
        doc: "Create a text layer in `parent`, its box sized to the text; answers it",
        params: &[
            req("parent", NODE, "the frame, group or page to create it in"),
            req(
                "text",
                TEXT,
                "what it says; a line break starts a paragraph",
            ),
            opt(
                "x",
                ParamKind::Number,
                "its box's left edge in the parent; 0 unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "its box's top edge in the parent; 0 unless given",
            ),
            opt(
                "width",
                ParamKind::Number,
                "the width a longer line wraps at; none unless given",
            ),
            opt(
                "index",
                INDEX,
                "its place among the parent's children, 0 the bottom",
            ),
            opt("name", TEXT, "its name"),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let wrap = a.number("width");
            if wrap.is_some_and(|w| w <= 0.0) {
                return Err(a.invalid("width", "a width is more than 0"));
            }
            let (kind, _, offset) = crate::edit::text_layer_kind(
                a.text("text").unwrap_or_default(),
                crate::TextParts::default(),
                wrap,
            );
            let at = kurbo::Vec2::new(a.number("x").unwrap_or(0.0), a.number("y").unwrap_or(0.0));
            let id = ctx.ids.mint();
            let tx = Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index: a.index("index").unwrap_or(count).min(count),
                kind,
                transform: Some(Affine::translate(at - offset)),
                name: a.text("name").map(str::to_owned),
            }]);
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "delete_node",
        doc: "Delete layers, with their frames' guides; instances of a deleted main are detached",
        params: &[req("nodes", NODES, "the layers to delete")],
        run: |ctx, a| Ok(Done::edit(build::delete(ctx.doc, &a.nodes("nodes")?))),
    },
    Verb {
        name: "duplicate",
        doc: "Copy layers, each copy one step above its original; answers the copies",
        params: &[
            req("nodes", NODES, "the layers to copy"),
            opt(
                "dx",
                ParamKind::Number,
                "how far right to move the copies, 0 unless given",
            ),
            opt(
                "dy",
                ParamKind::Number,
                "how far down to move the copies, 0 unless given",
            ),
        ],
        run: |ctx, a| {
            let offset =
                kurbo::Vec2::new(a.number("dx").unwrap_or(0.0), a.number("dy").unwrap_or(0.0));
            let (tx, created) = build::duplicate(ctx.doc, ctx.ids, &a.nodes("nodes")?, offset);
            if created.is_empty() {
                return Err(VerbError::Nothing("none of the layers can be copied"));
            }
            Ok(Done::made(tx, created))
        },
    },
    Verb {
        name: "insert_subtree",
        doc: "Create a whole tree of layers in `parent` in one edit; answers its layers, root first. Copy an existing layer's shape from `get_tree`, take a layer's look with `like`, and make one layer with `create_node`",
        params: &[
            req("parent", NODE, "the frame, group or page to create it in"),
            req(
                "tree",
                ParamKind::Model("Tree"),
                "the root layer, its properties and its `children`, bottom first; a layer's `like` names one whose paint and text style it takes where it gives none",
            ),
            opt(
                "index",
                INDEX,
                "the root's place among the parent's children, 0 the bottom",
            ),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let index = a.index("index").unwrap_or(count).min(count);
            let mut raw = a.get("tree").cloned().ok_or(VerbError::Missing {
                verb: a.verb.name,
                param: "tree",
            })?;
            expand_like(ctx.doc, &mut raw).map_err(|why| a.invalid("tree", why))?;
            loose::loosen("tree", &mut raw);
            let tree: Tree = serde_json::from_value(raw).map_err(|e| a.invalid("tree", e))?;
            let mut done = Done::default();
            tree_ops(ctx, a, &tree, parent, index, &mut done)?;
            Ok(done)
        },
    },
    Verb {
        name: "import_svg",
        doc: "Read SVG markup into layers, in one new group in `parent`; answers the group",
        params: &[
            req("parent", NODE, "the frame, group or page to place it in"),
            req("svg", TEXT, "the markup, one `<svg>` element"),
            opt(
                "x",
                ParamKind::Number,
                "how far right of the parent's origin it goes, 0 unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "how far down from the parent's origin it goes, 0 unless given",
            ),
            opt("name", TEXT, "the group's name"),
            opt(
                "index",
                INDEX,
                "the group's place among the parent's children, 0 the bottom; on top unless given",
            ),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let index = a.index("index").unwrap_or(count).min(count);
            let svg = a.text("svg").unwrap_or_default();
            // Reborrowed to the one lifetime `import` asks of both; `as_deref_mut`
            // keeps the sink's own, and the borrow checker refuses the call.
            let images: Option<&mut dyn ImageSink> = match &mut ctx.images {
                Some(s) => Some(&mut **s),
                None => None,
            };
            let mut out = crate::svg_in::import(svg, &mut *ctx.ids, parent, index, images)
                .map_err(|e| a.invalid("svg", e))?;
            if out.shapes == 0 {
                return Err(VerbError::Nothing("the markup draws nothing"));
            }
            let root = out.root;
            let shift =
                Affine::translate((a.number("x").unwrap_or(0.0), a.number("y").unwrap_or(0.0)));
            if let Some(Operation::CreateNode {
                transform, name, ..
            }) = out
                .tx
                .0
                .iter_mut()
                .find(|op| matches!(op, Operation::CreateNode { id, .. } if *id == root))
            {
                *transform = Some(shift * transform.unwrap_or(Affine::IDENTITY));
                if let Some(given) = a.text("name") {
                    *name = Some(given.to_owned());
                }
            }
            let done = Done::made(out.tx, vec![root]);
            Ok(match out.skipped.is_empty() {
                true => done,
                false => done.saying(format!("Left out: {}", out.skipped.join(", "))),
            })
        },
    },
    Verb {
        name: "reparent",
        doc: "Move a layer into another parent, keeping where it is drawn",
        params: &[
            req("node", NODE, "the layer to move"),
            req("parent", NODE, "its new parent"),
            opt(
                "index",
                INDEX,
                "its place there, 0 the bottom; on top unless given",
            ),
        ],
        run: |ctx, a| {
            let (id, parent) = (a.node("node")?, a.node("parent")?);
            let count = node(ctx, parent)?.children().len();
            let index = a.index("index").unwrap_or(count).min(count);
            Ok(Done::edit(build::reparent_preserving_world(
                ctx.doc, ctx.res, id, parent, index,
            )?))
        },
    },
    Verb {
        name: "reorder",
        doc: "Move a layer to `index` among its siblings, 0 the bottom",
        params: &[
            req("node", NODE, "the layer to move"),
            req("index", INDEX, "its new place"),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::Reorder {
                id: a.node("node")?,
                index: a.index("index").unwrap_or(0),
            }]))
        },
    },
    Verb {
        name: "arrange",
        doc: "Bring layers to the front or forward, or send them backward or to the back",
        params: &[
            req("nodes", NODES, "the layers to restack"),
            req("move", ParamKind::Choice(MOVES), "where they go"),
        ],
        run: |ctx, a| {
            let mv = match a.choice("move") {
                Some("front") => ZMove::Front,
                Some("forward") => ZMove::Forward,
                Some("backward") => ZMove::Backward,
                _ => ZMove::Back,
            };
            Ok(Done::edit(build::z_order(ctx.doc, &a.nodes("nodes")?, mv)))
        },
    },
    Verb {
        name: "group",
        doc: "Group layers; answers the new group",
        params: &[req("nodes", NODES, "the layers to group")],
        run: |ctx, a| {
            let (tx, group) = build::group(ctx.doc, ctx.res, ctx.ids, &a.nodes("nodes")?)?;
            Ok(Done::made(tx, vec![group]))
        },
    },
    Verb {
        name: "frame",
        doc: "Wrap layers in a new frame sized to them, with no fill and no clip; answers the frame",
        params: &[req("nodes", NODES, "the layers to frame; one is enough")],
        run: |ctx, a| {
            let nodes = a.layers(ctx.doc, "nodes")?;
            if !build::can_frame(ctx.doc, &nodes) {
                let parents: std::collections::HashSet<NodeId> = nodes
                    .iter()
                    .filter_map(|n| ctx.doc.get(*n)?.parent())
                    .collect();
                return Err(VerbError::Nothing(match parents.len() {
                    0 | 1 => "a frame cannot go inside a boolean or a mask",
                    _ => "the layers have different parents; frame each parent's layers apart",
                }));
            }
            let (tx, frame) = build::frame(ctx.doc, ctx.res, ctx.ids, &nodes)?;
            Ok(Done::made(tx, vec![frame]))
        },
    },
    Verb {
        name: "ungroup",
        doc: "Lift each group's or boolean's children into its parent, where they are drawn",
        params: &[req("nodes", NODES, "the groups to ungroup")],
        run: |ctx, a| {
            Ok(Done::edit(build::ungroup_all(
                ctx.doc,
                ctx.res,
                &a.nodes("nodes")?,
            )?))
        },
    },
    Verb {
        name: "boolean",
        doc: "Combine shapes into a live boolean; answers the new layer",
        params: &[
            req("nodes", NODES, "the shapes, bottom first"),
            req("op", ParamKind::Choice(BOOL_OPS), "how they combine"),
        ],
        run: |ctx, a| {
            let op = match a.choice("op") {
                Some("subtract") => BoolOp::Subtract,
                Some("intersect") => BoolOp::Intersect,
                Some("exclude") => BoolOp::Exclude,
                _ => BoolOp::Union,
            };
            let (tx, id) = build::boolean(ctx.doc, ctx.res, ctx.ids, &a.nodes("nodes")?, op, None)?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "flatten",
        doc: "Bake layers into one path as drawn; answers the new path",
        params: &[req("nodes", NODES, "the layers to flatten")],
        run: |ctx, a| {
            let (tx, id) = build::flatten(ctx.doc, ctx.res, ctx.ids, &a.nodes("nodes")?)?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "outline_shape",
        doc: "Turn a shape into the path its outline traces, corners baked in; answers the path",
        params: &[req(
            "node",
            NODE,
            "a rectangle, ellipse, polygon, star, line or path",
        )],
        run: |ctx, a| {
            let (tx, id) = build::outline(ctx.doc, ctx.res, ctx.ids, a.layer(ctx.doc, "node")?)?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "outline_stroke",
        doc: "Turn a layer's visible strokes into filled paths; answers the layer to select",
        params: &[req(
            "node",
            NODE,
            "a shape, path or boolean with a visible stroke",
        )],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            if !build::can_outline_stroke(ctx.doc, id) {
                return Err(VerbError::Nothing("this layer has no stroke to outline"));
            }
            let (tx, made) = build::outline_stroke(ctx.doc, ctx.res, ctx.ids, id)?;
            Ok(Done::made(tx, vec![made]))
        },
    },
    Verb {
        name: "outline_text",
        doc: "Replace a text layer with the path its glyphs trace; the text is gone. Answers the path",
        params: &[req("node", NODE, "the text layer")],
        run: |ctx, a| {
            let (tx, id) =
                build::outline_text(ctx.doc, ctx.res, ctx.ids, a.layer(ctx.doc, "node")?)?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "divide",
        doc: "Cut overlapping shapes, and any lines across them, into one path per face, in a new group; answers the group",
        params: &[req("nodes", NODES, "the shapes and lines to cut")],
        run: |ctx, a| {
            use crate::shape_builder::{self, Refusal};
            let nodes = a.layers(ctx.doc, "nodes")?;
            let arr = shape_builder::arrange(ctx.doc, ctx.res, &nodes).map_err(|r| match r {
                Refusal::Abandoned => VerbError::Nothing("the shapes could not be cut"),
                Refusal::TooFew | Refusal::NoOverlap => VerbError::Nothing(
                    "divide needs overlapping shapes, or a shape and a line across it",
                ),
            })?;
            let (tx, group) = shape_builder::divide(ctx.doc, ctx.res, ctx.ids, &nodes, &arr)?;
            Ok(Done::made(tx, vec![group]))
        },
    },
    Verb {
        name: "blend_shapes",
        doc: "Put shapes between two, stepping from one to the other, in a new group; answers the group",
        params: &[
            req("nodes", NODES, "exactly two shapes, the first lowest"),
            opt(
                "steps",
                INDEX,
                "how many shapes go between: 1 to 200, 5 unless given",
            ),
            opt(
                "paint",
                BOOL,
                "whether fills, strokes and opacity move from one to the other; true unless given",
            ),
            opt(
                "reverse",
                BOOL,
                "run the second shape's outline the other way round; false unless given",
            ),
        ],
        run: |ctx, a| {
            let steps = a.index("steps").unwrap_or(5);
            if !(1..=crate::blend::MAX_STEPS).contains(&steps) {
                return Err(a.invalid("steps", "expected 1 to 200"));
            }
            let blend = crate::blend::Blend {
                steps,
                paint: a.bool("paint").unwrap_or(true),
                start: kurbo::Vec2::ZERO,
                reverse: a.bool("reverse").unwrap_or(false),
            };
            let nodes = a.layers(ctx.doc, "nodes")?;
            let (tx, group) = crate::blend::blend(ctx.doc, ctx.res, ctx.ids, &nodes, blend)
                .map_err(|r| VerbError::Nothing(r.says()))?;
            Ok(Done::made(tx, vec![group]))
        },
    },
    Verb {
        name: "join_paths",
        doc: "Join two paths at their nearest open ends into the first; the second goes",
        params: &[
            req("node", NODE, "the path kept, with its paint and name"),
            req("other", NODE, "the path joined into it"),
        ],
        run: |ctx, a| {
            let (keep, other) = (a.node("node")?, a.node("other")?);
            if keep == other {
                return Err(a.invalid("other", "a path cannot join itself"));
            }
            crate::edit::join_paths(ctx.doc, ctx.res, keep, other)
                .map(Done::edit)
                .map_err(VerbError::Nothing)
        },
    },
    Verb {
        name: "repeat",
        doc: "Repeat layers round a point into a new group, as spokes or a ring; answers the group",
        params: &[
            req("nodes", NODES, "the layers to repeat, sharing a parent"),
            req(
                "count",
                INDEX,
                "how many in all, the originals counted: 2 to 360",
            ),
            opt(
                "angle",
                ParamKind::Number,
                "degrees from one to the next, clockwise; a full turn split evenly unless given",
            ),
            opt(
                "turn",
                BOOL,
                "whether each copy turns with its step, as spokes do; true unless given",
            ),
            opt(
                "x",
                ParamKind::Number,
                "the point repeated round, across the page; the layers' centre unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "the point repeated round, down the page",
            ),
            opt(
                "linked",
                BOOL,
                "make the one layer a component first, so its copies are instances; false unless given",
            ),
        ],
        run: |ctx, a| {
            let nodes = build::outermost(ctx.doc, &a.nodes("nodes")?);
            let count = a.index("count").unwrap_or(0);
            if !(2..=crate::repeat::MAX_COUNT).contains(&count) {
                return Err(a.invalid("count", "expected 2 to 360"));
            }
            let mut ring = crate::repeat::Ring::full(count, a.bool("turn").unwrap_or(true));
            if let Some(deg) = a.number("angle") {
                ring.step = deg.to_radians();
            }
            let centre = crate::repeat::centre(ctx.res, &nodes)
                .ok_or(VerbError::Nothing("the layers have no bounds"))?;
            let pivot = kurbo::Point::new(
                a.number("x").unwrap_or(centre.x),
                a.number("y").unwrap_or(centre.y),
            );
            let (tx, group) = if a.bool("linked").unwrap_or(false) {
                use crate::repeat::NotLinked;
                match crate::repeat::why_not_linked(ctx.doc, &nodes) {
                    Some(NotLinked::Several) => {
                        return Err(a.invalid("linked", "linked copies repeat one layer at a time"));
                    }
                    Some(NotLinked::Linked) => {
                        return Err(a.invalid("linked", "copies of a component are linked already"));
                    }
                    Some(NotLinked::Not(why)) => return Err(a.invalid("linked", not_main(why))),
                    None => {}
                }
                crate::repeat::linked(ctx.doc, ctx.res, ctx.ids, nodes[0], pivot, ring)?
            } else {
                crate::repeat::repeat(
                    ctx.doc,
                    ctx.res,
                    ctx.ids,
                    &nodes,
                    pivot,
                    ring,
                    crate::repeat::Place::Group,
                )?
            };
            Ok(Done::made(tx, group.into_iter().collect()))
        },
    },
    Verb {
        name: "align",
        doc: "Line layers up on one edge or centre of the box round them all",
        params: &[
            req("nodes", NODES, "two or more layers"),
            req("edge", ParamKind::Choice(EDGES), "which edge or centre"),
        ],
        run: |ctx, a| {
            let ids = build::outermost(ctx.doc, &a.nodes("nodes")?);
            if ids.len() < 2 {
                return Err(VerbError::Nothing("align needs two or more layers"));
            }
            let target = ids
                .iter()
                .filter_map(|id| ctx.res.world_bounds(*id))
                .reduce(|a, b| a.union(b))
                .ok_or(VerbError::Nothing("the layers have no bounds"))?;
            let (axis, edge) = match a.choice("edge") {
                Some("left") => (Axis::X, Edge::Min),
                Some("center") => (Axis::X, Edge::Mid),
                Some("right") => (Axis::X, Edge::Max),
                Some("top") => (Axis::Y, Edge::Min),
                Some("middle") => (Axis::Y, Edge::Mid),
                _ => (Axis::Y, Edge::Max),
            };
            Ok(Done::edit(build::align(
                ctx.doc, ctx.res, &ids, target, axis, edge,
            )))
        },
    },
    Verb {
        name: "distribute",
        doc: "Space three or more layers evenly between the outer two",
        params: &[
            req("nodes", NODES, "three or more layers"),
            req("axis", ParamKind::Choice(AXES), "across or down"),
        ],
        run: |ctx, a| {
            let axis = match a.choice("axis") {
                Some("horizontal") => Axis::X,
                _ => Axis::Y,
            };
            Ok(Done::edit(build::distribute(
                ctx.doc,
                ctx.res,
                &a.nodes("nodes")?,
                axis,
            )))
        },
    },
    Verb {
        name: "tidy_up",
        doc: "Lay layers out as the grid they roughly sit in, on even gaps",
        params: &[req("nodes", NODES, "two or more layers")],
        run: |ctx, a| {
            let nodes = a.layers(ctx.doc, "nodes")?;
            if nodes.len() < 2 {
                return Err(VerbError::Nothing("tidy up needs two or more layers"));
            }
            let tx = build::tidy_up(ctx.doc, ctx.res, &nodes);
            if tx.0.is_empty() {
                return Err(VerbError::Nothing("already tidy"));
            }
            Ok(Done::edit(tx))
        },
    },
    Verb {
        name: "slice",
        doc: "Add a slice fitted to layers' bounds, exporting PNG at 1×, just above the topmost; answers the slice",
        params: &[req("nodes", NODES, "the layers to slice round")],
        run: |ctx, a| {
            let nodes = a.layers(ctx.doc, "nodes")?;
            let all_slices = nodes.iter().all(|i| {
                matches!(
                    ctx.doc.get(*i).map(|n| n.kind()),
                    Some(NodeKind::Slice { .. })
                )
            });
            if all_slices {
                return Err(VerbError::Nothing("a slice on a slice cuts nothing new"));
            }
            let id = ctx.ids.mint();
            let tx = build::slice_selection(ctx.doc, ctx.res, &nodes, id)
                .ok_or(VerbError::Nothing("the layers have no bounds"))?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    // --- components -----------------------------------------------------------
    Verb {
        name: "make_component",
        doc: "Make a frame or group a main component, or wrap layers in a frame made one; answers the main",
        params: &[req(
            "nodes",
            NODES,
            "one frame or group, or the layers to wrap",
        )],
        run: |ctx, a| match build::make_component(ctx.doc, ctx.res, ctx.ids, &a.nodes("nodes")?) {
            Ok((tx, main)) => Ok(Done::made(tx, vec![main])),
            Err(build::MakeError::Nothing) => {
                Err(VerbError::Nothing("no layers to make a component of"))
            }
            Err(build::MakeError::Not(why)) => Err(VerbError::Nothing(not_main(why))),
            Err(build::MakeError::Frame(e)) => Err(e.into()),
        },
    },
    Verb {
        name: "create_instance",
        doc: "Place an instance of a main component, or of a set's default variant, in `parent`; answers it",
        params: &[
            req("main", NODE, "the main component or set"),
            req("parent", NODE, "the frame, group or page to place it in"),
            opt(
                "x",
                ParamKind::Number,
                "its box's left edge in the parent; where the main's is unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "its box's top edge in the parent; where the main's is unless given",
            ),
        ],
        run: |ctx, a| {
            let (main, parent) = (a.node("main")?, a.node("parent")?);
            let m = node(ctx, main)?;
            if !m.component() && m.set().is_none() {
                return Err(a.invalid("main", "not a main component or a set"));
            }
            node(ctx, parent)?;
            let (mut tx, id) = build::place_instance(ctx.doc, ctx.ids, main, parent, None)
                .ok_or(VerbError::Nothing("the set has no variant to place"))?;
            let (x, y) = (a.number("x"), a.number("y"));
            if (x.is_some() || y.is_some())
                && let Some(op) = build::cornered_at(ctx.doc, &tx, id, x, y)
            {
                tx.0.push(op);
            }
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "detach_instance",
        doc: "Turn the instances layers sit in into ordinary layers that keep their look",
        params: &[req("nodes", NODES, "instances, or layers inside them")],
        run: |ctx, a| {
            let mut roots: Vec<NodeId> = a
                .nodes("nodes")?
                .iter()
                .filter_map(|id| crate::component::instance_root(ctx.doc, *id))
                .collect();
            roots.sort();
            roots.dedup();
            let ops: Vec<Operation> = build::outermost(ctx.doc, &roots)
                .iter()
                .filter_map(|r| crate::component::detach(ctx.doc, *r))
                .flat_map(|tx| tx.0)
                .collect();
            if ops.is_empty() {
                return Err(VerbError::Nothing("none of the layers is in an instance"));
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "swap_instance",
        doc: "Show another main in an instance, its overrides carried by name",
        params: &[
            req("node", NODE, "the instance, or a nested copy inside one"),
            req("main", NODE, "the main to show, of the same kind"),
        ],
        run: |ctx, a| {
            let (copy, main) = (a.node("node")?, a.node("main")?);
            // The main it shows already relinks nothing, as the instance card's pick.
            if crate::component::main_of(ctx.doc, copy) == Some(main) {
                return Ok(Done::default());
            }
            // An instance linked straight to a main relinks; a nested copy swaps,
            // carrying its variant values by name, as the instance card does.
            let tx = if crate::swap::can_relink(ctx.doc, copy) {
                crate::swap::can_relink_to(ctx.doc, copy, main)
                    .then(|| crate::variant::switch(ctx.doc, copy, main, ctx.ids))
                    .flatten()
            } else {
                crate::variant::swap_carrying(ctx.doc, copy, main)
            };
            tx.map(Done::edit).ok_or(VerbError::Nothing(
                "that instance cannot show that main: it must be a main of the same kind",
            ))
        },
    },
    Verb {
        name: "combine_as_variants",
        doc: "Combine main components of one kind into a component set; answers the set",
        params: &[req(
            "nodes",
            NODES,
            "the mains, sharing a parent and in no set",
        )],
        run: |ctx, a| {
            let mains = a.nodes("nodes")?;
            if !crate::variant::one_kind(ctx.doc, &mains) {
                return Err(VerbError::Nothing(
                    "a component set holds one kind of layer: these mains mix frames and groups",
                ));
            }
            let (tx, set) = crate::variant::combine(ctx.doc, ctx.res, ctx.ids, &mains)?;
            Ok(Done::made(tx, vec![set]))
        },
    },
    Verb {
        name: "switch_variant",
        doc: "Show another variant in an instance by one property's value",
        params: &[
            req("node", NODE, "the instance of a variant"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "its value to show"),
        ],
        run: |ctx, a| {
            let root = a.node("node")?;
            let main = crate::component::main_of(ctx.doc, root)
                .ok_or_else(|| a.invalid("node", "not an instance"))?;
            let set = crate::variant::set_of(ctx.doc, main)
                .and_then(|s| ctx.doc.get(s)?.set())
                .ok_or_else(|| a.invalid("node", "its main is in no component set"))?;
            let name = a.text("property").unwrap_or_default();
            let prop = set
                .props
                .iter()
                .position(|p| p.name == name)
                .ok_or_else(|| {
                    let names: Vec<&str> = set.props.iter().map(|p| p.name.as_str()).collect();
                    a.invalid(
                        "property",
                        format!("the set's properties are {}", names.join(", ")),
                    )
                })?;
            let value = a.text("value").unwrap_or_default();
            let to = crate::variant::switch_target(ctx.doc, main, prop, value)
                .ok_or_else(|| a.invalid("value", "no variant of the set has it"))?;
            if to == main {
                return Ok(Done::default());
            }
            crate::variant::switch(ctx.doc, root, to, ctx.ids)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("that instance cannot switch variants"))
        },
    },
    Verb {
        name: "define_property",
        doc: "Bind a layer inside a main to a component property, a new one unless `property` names one; answers a new one's id",
        params: &[
            req("node", NODE, "the layer inside a main"),
            req(
                "kind",
                ParamKind::Choice(PROP_KINDS),
                "what the property drives: the layer's visibility, its text, or the main a nested instance shows",
            ),
            opt(
                "property",
                TEXT,
                "an existing property of that kind to bind to, by name",
            ),
            opt(
                "name",
                TEXT,
                "a new property's name; named for the layer unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::variant::{Bind, PropKind};
            let node_id = a.node("node")?;
            let n = node(ctx, node_id)?;
            let owner = crate::variant::owner_above(ctx.doc, node_id)
                .ok_or_else(|| a.invalid("node", "not a layer inside a main component"))?;
            let shows = n
                .link
                .filter(|l| ctx.doc.get(*l).is_some_and(|m| m.component));
            let kind = match a.choice("kind") {
                Some("text") if matches!(n.kind(), NodeKind::Text { .. }) => PropKind::Text,
                Some("swap") if shows.is_some() => PropKind::Swap,
                Some("boolean") => PropKind::Boolean,
                _ => {
                    return Err(a.invalid(
                        "kind",
                        "a text property needs a text layer, a swap one a nested instance",
                    ));
                }
            };
            let props = ctx
                .doc
                .get(owner)
                .map(|o| o.props().to_vec())
                .unwrap_or_default();
            let (pick, item) = match a.text("property") {
                Some(name) => {
                    let p = props
                        .iter()
                        .find(|p| p.kind == kind && p.name == name)
                        .ok_or_else(|| {
                            a.invalid(
                                "property",
                                "the main has no property of that kind by that name",
                            )
                        })?;
                    (Bind::To(p.id), None)
                }
                None => {
                    let item = ctx.ids.mint_item();
                    (Bind::New(item), Some(item))
                }
            };
            let filter = shows
                .and_then(|m| ctx.doc.get(m))
                .map(|m| crate::swap::suggested_filter(m.name()))
                .unwrap_or_default();
            let mut next = crate::variant::rebind(&props, kind, node_id, pick, n.name(), &filter);
            if let (Some(item), Some(name)) = (item, a.text("name").map(str::trim)) {
                let reserved = ctx
                    .doc
                    .get(owner)
                    .and_then(|o| o.set())
                    .is_some_and(|s| s.props.iter().any(|p| p.name == name));
                if name.is_empty()
                    || reserved
                    || next.iter().any(|p| p.id != item && p.name == name)
                {
                    return Err(a.invalid(
                        "name",
                        "a property needs a name no other property of the main has",
                    ));
                }
                if let Some(p) = next.iter_mut().find(|p| p.id == item) {
                    p.value.name = name.to_owned();
                }
            }
            let ops = vec![Operation::SetProperties {
                id: owner,
                props: next,
            }];
            Ok(match item {
                Some(item) => Done::making(ops, item.0.to_wire()),
                None => Done::ops(ops),
            })
        },
    },
    Verb {
        name: "set_instance_property",
        doc: "Set one of an instance's component properties: a visibility, a text, or the main a slot shows",
        params: &[
            req("node", NODE, "the instance"),
            req("property", TEXT, "the property, by name"),
            req(
                "value",
                ParamKind::Model("PropValue"),
                "true or false, a text, or a main's id, as the property's kind takes",
            ),
        ],
        run: |ctx, a| {
            use crate::variant::{PropKind, PropValue};
            let root = crate::component::instance_root(ctx.doc, a.node("node")?)
                .ok_or_else(|| a.invalid("node", "not in an instance"))?;
            let name = a.text("property").unwrap_or_default();
            let props = crate::variant::instance_properties(ctx.doc, root);
            let p = props
                .iter()
                .find(|p| p.name == name && p.kind != PropKind::Nested)
                .ok_or_else(|| {
                    let names: Vec<&str> = props
                        .iter()
                        .map(|p| p.name.as_str())
                        .filter(|n| !n.is_empty())
                        .collect();
                    a.invalid(
                        "property",
                        format!("its properties are {}", names.join(", ")),
                    )
                })?;
            let v = a.get("value").cloned().unwrap_or(Value::Null);
            let value = match (p.kind, &v) {
                (PropKind::Boolean, Value::Bool(b)) => PropValue::Boolean(*b),
                (PropKind::Text, Value::String(s)) => PropValue::Text(s.clone()),
                (PropKind::Swap, Value::String(_)) => {
                    let main = wire(&v).map_err(|why| a.invalid("value", why))?;
                    if !ctx.doc.get(main).is_some_and(|m| m.component) {
                        return Err(a.invalid("value", "not a main component"));
                    }
                    let slots = crate::variant::counterparts(ctx.doc, root, p);
                    if slots
                        .iter()
                        .any(|c| !crate::swap::can_swap_to(ctx.doc, *c, main))
                    {
                        return Err(a.invalid(
                            "value",
                            "the slot cannot show that main: it must be a main of the same kind, not one holding the slot",
                        ));
                    }
                    PropValue::Swap(main)
                }
                _ => return Err(a.invalid("value", "not a value of the property's kind")),
            };
            Ok(Done::ops(crate::variant::set_property_detaching(
                ctx.doc,
                &[root],
                p,
                &value,
            )))
        },
    },
    Verb {
        name: "reset_overrides",
        doc: "Put instances, or layers inside them, back as their main has them",
        params: &[
            req("nodes", NODES, "instances, or layers inside them"),
            opt(
                "scope",
                ParamKind::Choice(RESETS),
                "everything (`all`, unless given), the fields alone, removed children restored, or the order",
            ),
        ],
        run: |ctx, a| {
            let scopes: Vec<NodeId> = build::outermost(ctx.doc, &a.nodes("nodes")?)
                .into_iter()
                .filter(|id| crate::component::instance_root(ctx.doc, *id).is_some())
                .collect();
            if scopes.is_empty() {
                return Err(VerbError::Nothing("none of the layers is in an instance"));
            }
            let kind = match a.choice("scope") {
                Some("fields") => crate::reset::Kind::Fields,
                Some("children") => crate::reset::Kind::Children,
                Some("order") => crate::reset::Kind::Order,
                _ => crate::reset::Kind::All,
            };
            Ok(Done::ops(crate::reset::stated(
                ctx.doc, kind, &scopes, ctx.ids,
            )))
        },
    },
    Verb {
        name: "push_to_main",
        doc: "Write an instance's changes into its main, so every instance of it takes them",
        params: &[
            req("node", NODE, "the instance, or a layer inside it"),
            opt(
                "scope",
                ParamKind::Choice(PUSHES),
                "properties, fields, local layers and order (`all`, unless given), or one of them, or the main's layers this instance removed",
            ),
        ],
        run: |ctx, a| {
            use crate::push::{self, Blocked, Kind};
            let id = a.layer(ctx.doc, "node")?;
            let root = crate::component::instance_root(ctx.doc, id)
                .ok_or(VerbError::Nothing("the layer is in no instance"))?;
            match push::blocked(ctx.doc, root) {
                Some(Blocked::Kit) => {
                    return Err(VerbError::Nothing(
                        "its main comes from a kit — change it in the kit",
                    ));
                }
                Some(Blocked::Locked) => {
                    return Err(VerbError::Nothing("its main is locked"));
                }
                None => {}
            }
            let kind = match a.choice("scope") {
                Some("properties") => Kind::Properties,
                Some("fields") => Kind::Fields,
                Some("layers") => Kind::Layers,
                Some("order") => Kind::Order,
                Some("removals") => Kind::Removals,
                _ => Kind::All,
            };
            let ops = push::push(ctx.doc, kind, root, ctx.ids);
            if ops.is_empty() {
                return Err(VerbError::Nothing("nothing to push"));
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "add_variant",
        doc: "Add a variant to a set: a copy of one variant taking the first free combination of values; answers it",
        params: &[req(
            "node",
            NODE,
            "the set, copying its last variant, or the variant to copy",
        )],
        run: |ctx, a| {
            use crate::variant;
            let id = a.layer(ctx.doc, "node")?;
            let (set, from) = match variant::is_set(ctx.doc, id) {
                true => (id, None),
                false => match variant::set_of(ctx.doc, id) {
                    Some(set) => (set, Some(id)),
                    None => return Err(a.invalid("node", "neither a set nor a variant")),
                },
            };
            let (tx, new) = variant::add_variant(ctx.doc, ctx.res, ctx.ids, set, from)
                .ok_or(VerbError::Nothing("this set has no variant to copy"))?;
            Ok(Done::made(tx, vec![new]))
        },
    },
    Verb {
        name: "set_variant_value",
        doc: "Set one of a variant's values, as the variant card's dropdowns do; a combination another variant holds shows as a clash",
        params: &[
            req("node", NODE, "the variant, a main in a set"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "one of the property's values"),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let set = crate::variant::set_of(ctx.doc, id)
                .ok_or_else(|| a.invalid("node", "not a variant of a component set"))?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let value = &p.values[variant_value(a, p)?];
            let mut values = node(ctx, id)?.variant().to_vec();
            if values.get(prop) == Some(value) {
                return Ok(Done::default());
            }
            if let Some(v) = values.get_mut(prop) {
                v.clone_from(value);
            }
            crate::variant::set_values(ctx.doc, id, values)
                .map(Done::edit)
                .ok_or(VerbError::Nothing(
                    "the variant's values do not fit its set",
                ))
        },
    },
    Verb {
        name: "rename_variant_property",
        doc: "Rename a component set's variant property",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("name", TEXT, "its new name"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, _) = variant_prop(ctx, a, set)?;
            let name = a.text("name").unwrap_or_default();
            crate::variant::rename_property(ctx.doc, set, prop, name)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("name", PROP_NAME_TAKEN))
        },
    },
    Verb {
        name: "rename_variant_value",
        doc: "Rename a value of a variant property, in the set and in every variant holding it",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "the value, by name"),
            req("name", TEXT, "its new name"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let value = variant_value(a, p)?;
            let name = a.text("name").unwrap_or_default();
            crate::variant::rename_value(ctx.doc, set, prop, value, name)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("name", VALUE_NAME_TAKEN))
        },
    },
    Verb {
        name: "add_variant_property",
        doc: "Add a variant property to a component set, every variant taking its one value",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            opt(
                "name",
                TEXT,
                "its name; the first free `Property N` unless given",
            ),
            opt("value", TEXT, "its one value; `Default` unless given"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let vs = node(ctx, set)?.set().cloned().unwrap_or_default();
            let name = match a.text("name") {
                Some(n) => n.to_owned(),
                None => crate::variant::next_property_name(&vs),
            };
            let value = a.text("value").unwrap_or("Default");
            if value.trim().is_empty() {
                return Err(a.invalid("value", "a value needs a name"));
            }
            crate::variant::add_property(ctx.doc, set, &name, value)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("name", PROP_NAME_TAKEN))
        },
    },
    Verb {
        name: "add_variant_value",
        doc: "Add a value to a variant property, last",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            opt(
                "value",
                TEXT,
                "its name; the first free `Value N` unless given",
            ),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, _) = variant_prop(ctx, a, set)?;
            let value = match a.text("value") {
                Some(v) => v.to_owned(),
                None => {
                    let vs = node(ctx, set)?.set().cloned().unwrap_or_default();
                    crate::variant::next_value_name(&vs, prop)
                }
            };
            crate::variant::add_value(ctx.doc, set, prop, &value)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("value", VALUE_NAME_TAKEN))
        },
    },
    Verb {
        name: "delete_variant_property",
        doc: "Delete a variant property from a component set and from every variant",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, _) = variant_prop(ctx, a, set)?;
            crate::variant::delete_property(ctx.doc, set, prop)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("a set needs at least one property"))
        },
    },
    Verb {
        name: "move_variant_value",
        doc: "Move a value of a variant property to another place in its list",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "the value, by name"),
            req("index", INDEX, "its new place, counted from 0"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let from = variant_value(a, p)?;
            let to = a.index("index").unwrap_or(0);
            if to >= p.values.len() {
                let why = format!(
                    "the property has {}",
                    plural(p.values.len(), "value", "values")
                );
                return Err(a.invalid("index", why));
            }
            if to == from {
                return Ok(Done::default());
            }
            crate::variant::move_value(ctx.doc, set, prop, from, to)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("that value cannot move there"))
        },
    },
    Verb {
        name: "delete_variant_value",
        doc: "Delete a value of a variant property, and the variants holding it; their instances detach",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "the value, by name"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let value = variant_value(a, p)?;
            let gone = crate::variant::using_value(ctx.doc, set, prop, value).len();
            let tx = crate::variant::delete_value(ctx.doc, set, prop, value)
                .ok_or(VerbError::Nothing("a property needs at least one value"))?;
            let done = Done::edit(tx);
            Ok(match gone {
                0 => done,
                n => done.saying(format!("deleted {}", plural(n, "variant", "variants"))),
            })
        },
    },
    Verb {
        name: "tie_state",
        doc: "Tie a value of a variant property to a browser state, making it the set's interaction state, or untie it",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "the value, by name"),
            opt(
                "state",
                ParamKind::Choice(STATES),
                "the state it stands for; absent unties it",
            ),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let value = variant_value(a, p)?;
            let state = a
                .choice("state")
                .map(|w| serde_json::from_value(Value::from(w)))
                .transpose()
                .map_err(|e| a.invalid("state", e))?;
            if p.state_of(&p.values[value]) == state {
                return Ok(Done::default());
            }
            if state.is_some()
                && let Some(why) = crate::variant::why_not_tie(ctx.doc, set, prop, value)
            {
                return Err(a.invalid("value", why));
            }
            crate::variant::tie_state(ctx.doc, set, prop, value, state)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("state", "another value is tied to that state"))
        },
    },
    Verb {
        name: "show_nested_properties",
        doc: "Show a nested instance's own properties on every instance of the main holding it, or stop",
        params: &[
            req("node", NODE, "the instance nested inside a main"),
            req("shown", BOOL, "whether its properties show"),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let nested = node(ctx, id)?
                .link()
                .and_then(|l| ctx.doc.get(l))
                .is_some_and(|l| l.component());
            if !nested || crate::variant::owner_above(ctx.doc, id).is_none() {
                return Err(a.invalid("node", "not an instance nested inside a main"));
            }
            let on = a.bool("shown").unwrap_or(true);
            Ok(crate::variant::set_shown(ctx.doc, ctx.ids, id, on)
                .map(Done::edit)
                .unwrap_or_default())
        },
    },
    Verb {
        name: "edit_property",
        doc: "Rename a component property, or change a swap property's filter and preferred mains",
        params: &[
            req("node", NODE, "the main or set owning it, or a variant"),
            req("property", TEXT, "the property, by name"),
            opt("name", TEXT, "its new name"),
            opt(
                "filter",
                TEXT,
                "a swap property's filter: the start of the names of the mains its picker offers; empty offers every main",
            ),
            opt(
                "preferred",
                ParamKind::Model("[Node]"),
                "a swap property's preferred mains, offered first, in order",
            ),
        ],
        run: |ctx, a| {
            use crate::variant::{PropKind, Property};
            let (owner, p) = component_prop(ctx, a)?;
            let name = match a.text("name").map(str::trim) {
                Some(n) => {
                    let o = node(ctx, owner)?;
                    let taken = o.props().iter().any(|q| q.id != p.id && q.name == n)
                        || o.set().is_some_and(|s| s.props.iter().any(|v| v.name == n));
                    if n.is_empty() || taken {
                        return Err(a.invalid("name", PROP_NAME_TAKEN));
                    }
                    n.to_owned()
                }
                None => p.name.clone(),
            };
            let swap = |param: &'static str| match p.kind {
                PropKind::Swap => Ok(()),
                _ => Err(a.invalid(param, "only a swap property has one")),
            };
            let filter = match a.text("filter") {
                Some(f) => {
                    swap("filter")?;
                    f.trim().to_owned()
                }
                None => p.filter.clone(),
            };
            let preferred = match a.model_opt::<Vec<String>>("preferred")? {
                Some(list) => {
                    swap("preferred")?;
                    // What the picker offers the slot, as the card's own picker of
                    // preferred mains lists it; each once.
                    let slot = p.bound.first().copied().ok_or_else(|| {
                        a.invalid(
                            "preferred",
                            "the property is bound to no nested instance yet",
                        )
                    })?;
                    let offered = crate::swap::options(ctx.doc, slot, "", "");
                    let mut mains = Vec::new();
                    for s in &list {
                        let m = NodeId::from_wire(s)
                            .filter(|m| offered.contains(m))
                            .ok_or_else(|| {
                                a.invalid(
                                    "preferred",
                                    format!("`{s}` is not a main the slot can show"),
                                )
                            })?;
                        if !mains.contains(&m) {
                            mains.push(m);
                        }
                    }
                    mains
                }
                None => p.preferred.clone(),
            };
            let next = Property {
                name,
                filter,
                preferred,
                ..p.value.clone()
            };
            if next == p.value {
                return Ok(Done::default());
            }
            crate::variant::edit_property(ctx.doc, owner, p.id, |_| Some(next))
                .map(Done::edit)
                .ok_or(VerbError::Nothing("that property is gone"))
        },
    },
    Verb {
        name: "delete_property",
        doc: "Delete a component property; the layers bound to it keep their values",
        params: &[
            req("node", NODE, "the main or set owning it, or a variant"),
            req("property", TEXT, "the property, by name"),
        ],
        run: |ctx, a| {
            let (owner, p) = component_prop(ctx, a)?;
            crate::variant::edit_property(ctx.doc, owner, p.id, |_| None)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("that property is gone"))
        },
    },
    Verb {
        name: "duplicate_as_component",
        doc: "Copy a main as a new, independent main beside it rather than an instance; answers it",
        params: &[
            req("node", NODE, "the main component"),
            opt(
                "dx",
                ParamKind::Number,
                "how far right to move the copy, 0 unless given",
            ),
            opt(
                "dy",
                ParamKind::Number,
                "how far down to move the copy, 0 unless given",
            ),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            if !node(ctx, id)?.component() {
                return Err(a.invalid("node", "not a main component"));
            }
            let (Some(nodes), Some(parent)) =
                (ctx.doc.capture_subtree(id), node(ctx, id)?.parent())
            else {
                return Err(VerbError::Nothing("this component cannot be copied"));
            };
            let index = node(ctx, parent)?
                .children()
                .iter()
                .position(|c| *c == id)
                .map(|i| i + 1);
            let offset =
                kurbo::Vec2::new(a.number("dx").unwrap_or(0.0), a.number("dy").unwrap_or(0.0));
            let (tx, created) = build::insert_subtrees_as(
                ctx.doc,
                ctx.ids,
                &[build::Placement {
                    nodes,
                    parent,
                    index,
                }],
                offset,
                crate::component::MainCopy::NewMain,
            );
            Ok(Done::made(tx, created.into_iter().take(1).collect()))
        },
    },
    // --- variables and modes ----------------------------------------------------
    Verb {
        name: "add_variable",
        doc: "Add a variable holding one value in every mode; answers its id",
        params: &[
            req("kind", ParamKind::Choice(VAR_KINDS), "what it holds"),
            opt(
                "name",
                TEXT,
                "its name, `/` between groups; the first free `kind N` unless given",
            ),
            opt(
                "value",
                ParamKind::Model("Cell"),
                "what it holds; the kind's default unless given",
            ),
            opt(
                "collection",
                ParamKind::Id("collection"),
                "its collection; the first unless given",
            ),
        ],
        run: |ctx, a| {
            let kind: VarKind =
                serde_json::from_value(Value::from(a.choice("kind").unwrap_or("color")))
                    .map_err(|e| a.invalid("kind", e))?;
            let table = ctx.doc.variables();
            let c = collection_arg(ctx, a)?.unwrap_or(table.first_collection().id);
            let value = a.model_opt::<Cell>("value")?;
            let (ops, id) =
                crate::variable::add_variable_ops(table, ctx.ids, c, kind, a.text("name"), value)
                    .map_err(|why| a.invalid("name", why))?;
            Ok(Done::making(ops, id.0.to_wire()))
        },
    },
    Verb {
        name: "remove_variable",
        doc: "Remove a variable; its users keep the value they drew, and aliases of it resolve",
        params: &[req("variable", ParamKind::Id("variable"), "the variable")],
        run: |ctx, a| {
            let id = own_variable(ctx, a)?;
            Ok(Done::ops(crate::variable::remove_ops(ctx.doc, &[id])))
        },
    },
    Verb {
        name: "rename_variable",
        doc: "Rename a variable",
        params: &[
            req("variable", ParamKind::Id("variable"), "the variable"),
            req("name", TEXT, "its new name, `/` between groups"),
        ],
        run: |ctx, a| {
            let id = own_variable(ctx, a)?;
            crate::variable::rename_variable_ops(
                ctx.doc.variables(),
                id,
                a.text("name").unwrap_or_default(),
            )
            .map(Done::ops)
            .map_err(|why| a.invalid("name", why))
        },
    },
    Verb {
        name: "set_variable_value",
        doc: "Set what a variable holds in one mode: a value, or an alias of another variable",
        params: &[
            req("variable", ParamKind::Id("variable"), "the variable"),
            opt(
                "mode",
                ParamKind::Id("mode"),
                "the mode, of the variable's collection; its only one when it has one",
            ),
            req(
                "value",
                ParamKind::Model("Cell"),
                "the value, or `{ \"alias\": <variable id> }`",
            ),
        ],
        run: |ctx, a| {
            let id = own_variable(ctx, a)?;
            let only = || match ctx.doc.variables().collection_of(id).map(|c| &c.modes[..]) {
                Some([m]) => Ok(m.id),
                _ => Err(a.invalid(
                    "mode",
                    "the variable's collection has more than one mode: name one",
                )),
            };
            let mode: ModeId = match a.id_opt("mode")? {
                Some(mode) => mode,
                None => only()?,
            };
            crate::variable::set_cell_ops(ctx.doc.variables(), id, mode, a.model("value")?)
                .map(Done::ops)
                .map_err(|why| a.invalid("value", why))
        },
    },
    Verb {
        name: "add_mode",
        doc: "Add a mode to a collection, a copy of its first; answers its id",
        params: &[
            opt(
                "name",
                TEXT,
                "its name; the first free `Mode N` unless given",
            ),
            opt(
                "collection",
                ParamKind::Id("collection"),
                "the collection; the first unless given",
            ),
        ],
        run: |ctx, a| {
            let c = collection_arg(ctx, a)?.unwrap_or(ctx.doc.variables().first_collection().id);
            let (ops, id) = crate::variable::add_mode_ops(ctx.doc, ctx.ids, c, a.text("name"))
                .map_err(|why| a.invalid("name", why))?;
            Ok(Done::making(ops, id.0.to_wire()))
        },
    },
    Verb {
        name: "remove_mode",
        doc: "Remove a mode; frames that picked it inherit their collection's again",
        params: &[req("mode", ParamKind::Id("mode"), "the mode")],
        run: |ctx, a| {
            let mode = own_mode(ctx, a)?;
            Ok(Done::ops(crate::variable::remove_mode_ops(ctx.doc, mode)))
        },
    },
    Verb {
        name: "rename_mode",
        doc: "Rename a mode",
        params: &[
            req("mode", ParamKind::Id("mode"), "the mode"),
            req("name", TEXT, "its new name, unique in its collection"),
        ],
        run: |ctx, a| {
            let mode = own_mode(ctx, a)?;
            let name = a.text("name").unwrap_or_default().trim();
            if let Some(why) = crate::variable::mode_name_refusal(ctx.doc.variables(), mode, name) {
                return Err(a.invalid("name", why));
            }
            Ok(Done::ops(crate::variable::rename_mode_ops(
                ctx.doc, mode, name,
            )))
        },
    },
    Verb {
        name: "set_frame_mode",
        doc: "Pick which mode of a collection a frame or page shows, or let it inherit",
        params: &[
            req("node", NODE, "the frame or page"),
            opt("mode", ParamKind::Id("mode"), "the mode; absent inherits"),
            opt(
                "collection",
                ParamKind::Id("collection"),
                "the collection; the mode's, else the first",
            ),
        ],
        run: |ctx, a| {
            let table = ctx.doc.variables();
            let mode = a.id_opt::<ModeId>("mode")?;
            let c = match (collection_arg(ctx, a)?, mode) {
                (Some(c), _) => c,
                (None, Some(m)) => {
                    table
                        .collection_of_mode(m)
                        .ok_or_else(|| a.invalid("mode", "no mode has that id"))?
                        .id
                }
                (None, None) => table.first_collection().id,
            };
            crate::variable::frame_mode_ops(ctx.doc, a.node("node")?, c, mode)
                .map(Done::ops)
                .map_err(|why| a.invalid("node", why))
        },
    },
    Verb {
        name: "move_mode",
        doc: "Move a mode to another place in its collection; the first is the default every unset frame reads",
        params: &[
            req("mode", ParamKind::Id("mode"), "the mode"),
            req("index", INDEX, "its new place, counted from 0"),
        ],
        run: |ctx, a| {
            let mode = own_mode(ctx, a)?;
            let t = ctx.doc.variables();
            let col = t
                .collection_of_mode(mode)
                .ok_or_else(|| a.invalid("mode", "no mode has that id"))?;
            if col.by_width() {
                return Err(a.invalid(
                    "mode",
                    "a collection picked by width orders its modes by their widths",
                ));
            }
            let to = a.index("index").unwrap_or(0);
            if to >= col.modes.len() {
                let why = format!(
                    "the collection has {}",
                    plural(col.modes.len(), "mode", "modes")
                );
                return Err(a.invalid("index", why));
            }
            Ok(Done::ops(crate::variable::move_mode_ops(ctx.doc, mode, to)))
        },
    },
    Verb {
        name: "add_collection",
        doc: "Add a variable collection with one mode, last in the list; answers its id",
        params: &[opt(
            "name",
            TEXT,
            "its name; the first free `Collection N` unless given",
        )],
        run: |ctx, a| {
            let t = ctx.doc.variables();
            let taken: Vec<&str> = t.collections().iter().map(|c| c.name.as_str()).collect();
            let name = match a.text("name") {
                Some(n) => collection_name(ctx, a, n, None)?,
                None => crate::variable::numbered_name(&taken, "Collection", taken.len() + 1),
            };
            let (id, mode) = (ctx.ids.mint_collection(), ctx.ids.mint_mode());
            let ops = crate::variable::new_collection_ops(id, mode, &name, taken.len());
            Ok(Done::making(ops, id.0.to_wire()))
        },
    },
    Verb {
        name: "rename_collection",
        doc: "Rename a variable collection",
        params: &[
            req("collection", ParamKind::Id("collection"), "the collection"),
            req("name", TEXT, "its new name, unique among the collections"),
        ],
        run: |ctx, a| {
            let c = collection_req(ctx, a)?;
            let name = collection_name(ctx, a, a.text("name").unwrap_or_default(), Some(c))?;
            if ctx
                .doc
                .variables()
                .collection(c)
                .is_some_and(|col| col.name == name)
            {
                return Ok(Done::default());
            }
            Ok(Done::ops(vec![Operation::SetCollectionName {
                id: c,
                name,
            }]))
        },
    },
    Verb {
        name: "duplicate_collection",
        doc: "Copy a variable collection after itself, its modes and variables under new ids, aliases within it pointing at the copies; answers the copy's id",
        params: &[
            req("collection", ParamKind::Id("collection"), "the collection"),
            opt(
                "name",
                TEXT,
                "the copy's name; `<name> copy`, numbered if taken, unless given",
            ),
        ],
        run: |ctx, a| {
            let c = collection_req(ctx, a)?;
            let t = ctx.doc.variables();
            let name = match a.text("name") {
                Some(n) => collection_name(ctx, a, n, None)?,
                None => {
                    let taken: Vec<&str> =
                        t.collections().iter().map(|c| c.name.as_str()).collect();
                    let stem = format!("{} copy", t.collection(c).map_or("", |c| c.name.as_str()));
                    match taken.contains(&stem.as_str()) {
                        true => crate::variable::numbered_name(&taken, &stem, 2),
                        false => stem,
                    }
                }
            };
            let ops = crate::variable::duplicate_collection_ops(ctx.doc, c, &name, ctx.ids);
            let made = ops.iter().find_map(|op| match op {
                Operation::AddCollection { collection, .. } => Some(collection.id.0.to_wire()),
                _ => None,
            });
            Ok(match made {
                Some(id) => Done::making(ops, id),
                None => Done::ops(ops),
            })
        },
    },
    Verb {
        name: "delete_collection",
        doc: "Delete a variable collection and its variables; their users keep the values they drew, and frames set to its modes inherit",
        params: &[req(
            "collection",
            ParamKind::Id("collection"),
            "the collection",
        )],
        run: |ctx, a| {
            let c = collection_req(ctx, a)?;
            let ops = crate::variable::remove_collection_ops(ctx.doc, c);
            if ops.is_empty() {
                return Err(VerbError::Nothing("the last collection cannot go"));
            }
            let (_, layers, _) = crate::variable::collection_reach(ctx.doc, c);
            let done = Done::ops(ops);
            Ok(match layers {
                0 => done,
                n => done.saying(format!(
                    "{} {} their values",
                    plural(n, "layer", "layers"),
                    if n == 1 { "keeps" } else { "keep" }
                )),
            })
        },
    },
    Verb {
        name: "set_by_width",
        doc: "Pick a collection's mode by the width of the frame a layer sits in, or stop; one collection at most",
        params: &[
            req("collection", ParamKind::Id("collection"), "the collection"),
            req("on", BOOL, "whether its modes are picked by width"),
        ],
        run: |ctx, a| {
            let c = collection_req(ctx, a)?;
            let ops = crate::variable::by_width_ops(ctx.doc, c, a.bool("on").unwrap_or(true))
                .map_err(|why| a.invalid("collection", why))?;
            let pinned = ops
                .iter()
                .filter(|op| matches!(op, Operation::SetModes { .. }))
                .count();
            let done = Done::ops(ops);
            Ok(match pinned {
                0 => done,
                n => done.saying(format!(
                    "{} {} the mode {} drew",
                    plural(n, "frame", "frames"),
                    if n == 1 { "keeps" } else { "keep" },
                    if n == 1 { "it" } else { "they" }
                )),
            })
        },
    },
    Verb {
        name: "set_breakpoint",
        doc: "Set the minimum width a mode of a collection picked by width starts at",
        params: &[
            req(
                "mode",
                ParamKind::Id("mode"),
                "the mode, not its collection's first",
            ),
            req("width", ParamKind::Number, "its minimum width in px"),
        ],
        run: |ctx, a| {
            let mode = own_mode(ctx, a)?;
            let by_width = ctx
                .doc
                .variables()
                .collection_of_mode(mode)
                .is_some_and(|c| c.by_width());
            if !by_width {
                return Err(a.invalid(
                    "mode",
                    "its collection is not picked by width: `set_by_width` first",
                ));
            }
            crate::variable::breakpoint_ops(ctx.doc, mode, a.number("width").unwrap_or(0.0))
                .map(Done::ops)
                .map_err(|why| a.invalid("width", why))
        },
    },
    Verb {
        name: "set_width_query",
        doc: "Let a main's contents pick a collection's mode by the main's own width, or stop",
        params: &[
            req(
                "node",
                NODE,
                "the main component, a frame whose width is set from outside; or a frame that has a width query and is no instance, to stop",
            ),
            req("on", BOOL, "whether it picks by its width"),
            opt(
                "collection",
                ParamKind::Id("collection"),
                "the collection; the first unless given",
            ),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let n = node(ctx, id)?;
            // As the mode rows offer *By width*: a main, or a frame holding a
            // query of its own that follows no main, so a detached copy can let
            // its main's go.
            if !(n.component() || (n.link().is_none() && !n.queries().is_empty())) {
                return Err(a.invalid(
                    "node",
                    "only a main component picks modes by its own width: make it a component first",
                ));
            }
            let c = collection_arg(ctx, a)?.unwrap_or(ctx.doc.variables().first_collection().id);
            let on = a.bool("on").unwrap_or(true);
            if on && let Some(why) = crate::variable::why_no_query(ctx.doc, id) {
                return Err(a.invalid("node", why));
            }
            let modes = ctx
                .doc
                .variables()
                .collection(c)
                .map_or(0, |c| c.modes.len());
            if on && modes < 2 {
                return Err(a.invalid(
                    "collection",
                    "a width picks between two modes or more, and the collection has fewer",
                ));
            }
            crate::variable::query_ops(ctx.doc, id, crate::variable::ModeOwner::Collection(c), on)
                .map(Done::ops)
                .map_err(|why| a.invalid("node", why))
        },
    },
    Verb {
        name: "set_scope",
        doc: "Narrow the fields a variable may bind, or let it bind every field of its kind; fields it leaves detach to what they drew",
        params: &[
            req("variable", ParamKind::Id("variable"), "the variable"),
            opt(
                "scope",
                ParamKind::Model("Scope"),
                "the fields, by name, of the variable's kind; absent for every field",
            ),
        ],
        run: |ctx, a| {
            use crate::variable::{Field, Scope};
            let id = own_variable(ctx, a)?;
            let t = ctx.doc.variables();
            let kind = t.get(id).map(|v| v.kind);
            let scope = match a.model_opt::<Vec<String>>("scope")? {
                None => None,
                Some(words) => {
                    let mut fields = Vec::new();
                    for w in &words {
                        let f = Field::from_wire(w)
                            .filter(|f| Some(f.kind()) == kind)
                            .ok_or_else(|| {
                                let ok: Vec<&str> = kind
                                    .into_iter()
                                    .flat_map(Field::of_kind)
                                    .map(Field::wire)
                                    .collect();
                                a.invalid("scope", format!("`{w}` is not one of {}", ok.join(", ")))
                            })?;
                        fields.push(f);
                    }
                    if fields.is_empty() {
                        return Err(a.invalid(
                            "scope",
                            "a scope names a field at least; leave it out for every field",
                        ));
                    }
                    Some(Scope::of(fields))
                }
            };
            if let Some(held) = crate::variable::scope_held_by(ctx.doc, id, scope) {
                let name = |v| t.get(v).map_or("", |v| v.name.as_str());
                return Err(a.invalid(
                    "scope",
                    format!(
                        "{} reads {} from another collection, so its words cannot change",
                        name(held),
                        name(id)
                    ),
                ));
            }
            let dropped = crate::variable::users_by_field(ctx.doc, id)
                .into_iter()
                .filter(|(f, _)| crate::variable::scope_drops(scope, *f))
                .flat_map(|(_, ids)| ids)
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let done = Done::ops(crate::variable::scope_ops(ctx.doc, id, scope));
            Ok(match dropped {
                0 => done,
                n => done.saying(format!(
                    "{} {} detached",
                    plural(n, "layer", "layers"),
                    if n == 1 { "was" } else { "were" }
                )),
            })
        },
    },
    Verb {
        name: "bind_variable",
        doc: "Bind a field of layers to a variable, or detach it to the value it draws",
        params: &[
            req("nodes", NODES, "the layers"),
            req(
                "field",
                ParamKind::Model("BoundField"),
                "the field, with its place: a fill's index, a corner, a gap",
            ),
            opt(
                "variable",
                ParamKind::Id("variable"),
                "the variable, of the field's kind; absent detaches",
            ),
        ],
        run: |ctx, a| {
            let field = a.model::<build::BoundField>("field")?;
            if !field.in_range() {
                return Err(a.invalid("field", "no such place in that field"));
            }
            let var = a.id_opt::<VarId>("variable")?;
            if var.is_some_and(|v| ctx.doc.variables().get(v).is_none()) {
                return Err(a.invalid("variable", "no variable has that id"));
            }
            // A page has no eye or opacity control, as `set_visible` and
            // `set_opacity` refuse it; its paint fields reach its shapes.
            let nodes = match field {
                build::BoundField::Visible | build::BoundField::Opacity => {
                    a.layers(ctx.doc, "nodes")?
                }
                _ => a.nodes("nodes")?,
            };
            for id in &nodes {
                node(ctx, *id)?;
            }
            let has_item = |i: usize, stroke: bool| {
                build::paint_targets(ctx.doc, &nodes).iter().any(|id| {
                    ctx.doc.get(*id).is_some_and(|n| match stroke {
                        true => n.paint().strokes.len() > i,
                        false => n.paint().fills.len() > i,
                    })
                })
            };
            match field {
                build::BoundField::Fill(i) if !has_item(i, false) => {
                    return Err(a.invalid("field", "none of the layers has a fill there"));
                }
                build::BoundField::Stroke(i) if !has_item(i, true) => {
                    return Err(a.invalid("field", "none of the layers has a stroke there"));
                }
                _ => {}
            }
            Ok(Done::edit(build::bind_field_ops(
                ctx.doc, &nodes, field, var,
            )))
        },
    },
    // --- styles -----------------------------------------------------------------
    Verb {
        name: "add_style",
        doc: "Add a style from a value or from a layer's look, and apply it to layers; answers its id",
        params: &[
            opt(
                "value",
                ParamKind::Model("StyleValue"),
                "what it holds; or give `from`",
            ),
            opt(
                "from",
                NODE,
                "a layer whose look in `slot` it takes; or give `value`",
            ),
            opt(
                "slot",
                ParamKind::Choice(STYLE_SLOTS),
                "the slot `from` is read in",
            ),
            opt(
                "name",
                TEXT,
                "its name; `<Slot> style`, numbered if taken, unless given",
            ),
            opt("nodes", NODES, "layers to apply it to"),
        ],
        run: |ctx, a| {
            let value = match (a.model_opt::<StyleInput>("value")?, a.get("from")) {
                (Some(input), None) => style_value(ctx, input, None),
                (None, Some(_)) => {
                    let slot = a.choice("slot").map(slot_of).ok_or(VerbError::Missing {
                        verb: "add_style",
                        param: "slot",
                    })?;
                    crate::style::value_drawn(ctx.doc, a.node("from")?, slot).ok_or_else(|| {
                        a.invalid("from", "it has nothing in that slot to make a style of")
                    })?
                }
                _ => {
                    return Err(a.invalid("value", "give a `value` or a `from` layer, one of them"));
                }
            };
            let slot = value.slot();
            let styles = ctx.doc.styles();
            let name = match a.text("name").map(str::trim) {
                Some(n) => {
                    if let Some(why) = crate::style::name_refusal(styles, slot, n, None) {
                        return Err(a.invalid("name", why));
                    }
                    n.to_owned()
                }
                None => crate::style::free_name(styles, slot, &format!("{} style", slot.title())),
            };
            let id = ctx.ids.mint_style();
            let mut ops = vec![crate::style::add_op(id, &name, value)];
            if a.get("nodes").is_some() {
                ops.extend(
                    crate::style::targets(ctx.doc, &style_nodes(ctx, a)?, slot)
                        .into_iter()
                        .map(|n| crate::style::bind_op(n, slot, Some(id))),
                );
            }
            Ok(Done::making(ops, id.0.to_wire()))
        },
    },
    Verb {
        name: "remove_style",
        doc: "Remove a style; its users keep the look they drew",
        params: &[req("style", ParamKind::Id("style"), "the style")],
        run: |ctx, a| {
            let id = own_style(ctx, a)?.id;
            Ok(Done::ops(crate::style::remove_ops(ctx.doc, &[id])))
        },
    },
    Verb {
        name: "rename_style",
        doc: "Rename a style",
        params: &[
            req("style", ParamKind::Id("style"), "the style"),
            req("name", TEXT, "its new name, unique among its slot's"),
        ],
        run: |ctx, a| {
            let id: StyleId = a.id("style")?;
            crate::style::rename_ops(ctx.doc.styles(), id, a.text("name").unwrap_or_default())
                .map(Done::ops)
                .map_err(|why| a.invalid("name", why))
        },
    },
    Verb {
        name: "set_style_value",
        doc: "Change what a style holds; every layer applying it follows",
        params: &[
            req("style", ParamKind::Id("style"), "the style"),
            req(
                "value",
                ParamKind::Model("StyleValue"),
                "its new value, of its own slot",
            ),
        ],
        run: |ctx, a| {
            let st = own_style(ctx, a)?;
            let (id, own) = (st.id, st.value.clone());
            let value = style_value(ctx, a.model("value")?, Some(&own));
            if value.slot() != own.slot() {
                return Err(a.invalid(
                    "value",
                    format!(
                        "a {} style holds {} only",
                        own.slot().title(),
                        own.slot().title()
                    ),
                ));
            }
            Ok(Done::ops(vec![Operation::SetStyleValue { id, value }]))
        },
    },
    Verb {
        name: "apply_style",
        doc: "Apply a style to layers, or detach them from the style in a slot",
        params: &[
            req("nodes", NODES, "the layers"),
            opt(
                "style",
                ParamKind::Id("style"),
                "the style; absent detaches",
            ),
            opt(
                "slot",
                ParamKind::Choice(STYLE_SLOTS),
                "the slot to detach; the style's when it is given",
            ),
        ],
        run: |ctx, a| {
            let nodes = style_nodes(ctx, a)?;
            let (style, slot) = match a.id_opt::<StyleId>("style")? {
                Some(id) => {
                    let st = ctx
                        .doc
                        .styles()
                        .get(id)
                        .ok_or_else(|| a.invalid("style", "no style has that id"))?;
                    (Some(id), st.slot())
                }
                None => {
                    let slot = a.choice("slot").map(slot_of).ok_or(VerbError::Missing {
                        verb: "apply_style",
                        param: "slot",
                    })?;
                    (None, slot)
                }
            };
            let ops = crate::style::targets(ctx.doc, &nodes, slot)
                .into_iter()
                .filter(|n| {
                    style.is_some()
                        || ctx
                            .doc
                            .get(*n)
                            .is_some_and(|x| x.styles().get(slot).is_some())
                })
                .map(|n| crate::style::bind_op(n, slot, style))
                .collect();
            Ok(Done::ops(ops))
        },
    },
    // --- pages ----------------------------------------------------------------
    Verb {
        name: "add_page",
        doc: "Add a page; answers it",
        params: &[
            opt(
                "name",
                TEXT,
                "its name; the first free `Page N` unless given",
            ),
            opt(
                "index",
                INDEX,
                "its place in the page order, 0 the first; last unless given",
            ),
        ],
        run: |ctx, a| {
            let index = a.index("index").unwrap_or(usize::MAX);
            let name = a
                .text("name")
                .map(|n| page_name(a, n).map(str::to_owned))
                .transpose()?;
            let (tx, id) = crate::page::add(ctx.doc, ctx.ids, index, name, None);
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "duplicate_page",
        doc: "Copy a page below it, with its layers and guides; answers the copy",
        params: &[req("page", NODE, "the page")],
        run: |ctx, a| {
            let page = a.node("page")?;
            let (tx, id) = crate::page::duplicate(ctx.doc, ctx.ids, page)
                .ok_or_else(|| a.invalid("page", "not a page"))?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "delete_page",
        doc: "Delete a page with its layers and guides; instances of a main on it are detached",
        params: &[req("page", NODE, "the page")],
        run: |ctx, a| {
            let page = a.node("page")?;
            if !ctx.doc.is_page(page) {
                return Err(a.invalid("page", "not a page"));
            }
            crate::page::delete(ctx.doc, page)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("a document keeps at least one page"))
        },
    },
    Verb {
        name: "move_page",
        doc: "Move a page to `index` in the page order, 0 the first",
        params: &[
            req("page", NODE, "the page"),
            req("index", INDEX, "its new place"),
        ],
        run: |ctx, a| {
            let page = a.node("page")?;
            if !ctx.doc.is_page(page) {
                return Err(a.invalid("page", "not a page"));
            }
            let last = ctx.doc.pages().len() - 1;
            Ok(Done::ops(vec![Operation::MovePage {
                id: page,
                index: a.index("index").unwrap_or(0).min(last),
            }]))
        },
    },
    Verb {
        name: "set_page_draft",
        doc: "Mark a page as a draft, left out of publishing, export all and present, or not",
        params: &[req("page", NODE, "the page"), req("draft", BOOL, "a draft")],
        run: |ctx, a| {
            let page = a.node("page")?;
            if !ctx.doc.is_page(page) {
                return Err(a.invalid("page", "not a page"));
            }
            Ok(Done::ops(vec![Operation::SetPageDraft {
                id: page,
                draft: a.bool("draft").unwrap_or(true),
            }]))
        },
    },
    // --- placement and geometry -------------------------------------------
    Verb {
        name: "set_transform",
        doc: "Set a layer's placement in its parent",
        params: &[
            req("node", NODE, "the layer"),
            req(
                "transform",
                ParamKind::Model("Affine"),
                "the six coefficients",
            ),
        ],
        run: |ctx, a| {
            Ok(Done::ops(vec![Operation::SetTransform {
                id: a.layer(ctx.doc, "node")?,
                transform: a.model("transform")?,
            }]))
        },
    },
    Verb {
        name: "set_pivot",
        doc: "Set or clear the point a layer turns and scales about",
        params: &[
            req("node", NODE, "the layer"),
            opt(
                "pivot",
                ParamKind::Model("Pivot"),
                "the point; absent clears it",
            ),
        ],
        run: |ctx, a| {
            Ok(Done::ops(vec![Operation::SetPivot {
                id: a.layer(ctx.doc, "node")?,
                pivot: a.model_opt::<Pivot>("pivot")?,
            }]))
        },
    },
    Verb {
        name: "set_geometry",
        doc: "Change a layer's kind-specific geometry: a size, a path, a corner radius",
        params: &[
            req("node", NODE, "the layer"),
            req("geometry", ParamKind::Model("GeometryPatch"), "the change"),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            let geometry = a.model::<GeometryPatch>("geometry")?;
            if bakes_rounding(ctx, id, &geometry) {
                return Err(a.invalid(
                    "geometry",
                    "this is the path as drawn, its rounded corners expanded; write the points the snapshot's `subpaths` lists, or move one with `set_point`",
                ));
            }
            Ok(Done::ops(vec![Operation::SetGeometry { id, geometry }]))
        },
    },
    Verb {
        name: "set_point",
        doc: "Move one point of a path, aim its handles or round its corner, in the layer's own space as the snapshot's `subpaths` lists them",
        params: &[
            req("node", NODE, "the path"),
            req(
                "point",
                INDEX,
                "which point of the subpath, counted from 0 as `subpaths` lists them",
            ),
            opt("subpath", INDEX, "which subpath; 0 unless given"),
            opt(
                "x",
                ParamKind::Number,
                "where the point goes across; its handles move with it",
            ),
            opt(
                "y",
                ParamKind::Number,
                "where the point goes down; its handles move with it",
            ),
            opt(
                "in_x",
                ParamKind::Number,
                "where the handle arriving at the point ends, across",
            ),
            opt(
                "in_y",
                ParamKind::Number,
                "where the handle arriving at the point ends, down",
            ),
            opt(
                "out_x",
                ParamKind::Number,
                "where the handle leaving the point ends, across; one handle given on a smooth point mirrors the other",
            ),
            opt(
                "out_y",
                ParamKind::Number,
                "where the handle leaving the point ends, down",
            ),
            opt(
                "radius",
                ParamKind::Number,
                "how far the corner at the point is rounded; 0 for sharp",
            ),
        ],
        run: set_point,
    },
    Verb {
        name: "set_size",
        doc: "Make a layer this wide and high, whatever its kind; a text gets a fixed box",
        params: &[
            req("node", NODE, "the layer"),
            req("width", ParamKind::Number, "its width"),
            req("height", ParamKind::Number, "its height"),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            node(ctx, id)?;
            let want = kurbo::Size::new(
                a.number("width").unwrap_or(0.0),
                a.number("height").unwrap_or(0.0),
            );
            if want.width < 0.0 || want.height < 0.0 {
                return Err(a.invalid("width", "a size is not negative"));
            }
            Ok(Done::edit(crate::edit::resize_to(
                ctx.doc, ctx.res, id, want,
            )))
        },
    },
    Verb {
        name: "pixel_fit",
        doc: "Put layers' edges and points on whole units, so they render crisp; text and turned layers are left",
        params: &[req(
            "nodes",
            NODES,
            "the layers, groups and frames taken whole",
        )],
        run: |ctx, a| {
            let (tx, _) = crate::pixel_fit::fit(ctx.doc, ctx.res, &a.nodes("nodes")?);
            Ok(Done::edit(tx))
        },
    },
    // --- layer state --------------------------------------------------------
    Verb {
        name: "set_name",
        doc: "Rename a layer or a page",
        params: &[
            req("node", NODE, "the layer or page"),
            req("name", TEXT, "its new name"),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            let name = a.text("name").unwrap_or_default();
            let name = match ctx.doc.is_page(id) {
                true => page_name(a, name)?,
                false => name,
            };
            Ok(Done::ops(vec![Operation::SetName {
                id,
                name: name.to_owned(),
            }]))
        },
    },
    Verb {
        name: "set_visible",
        doc: "Show or hide layers",
        params: &[
            req("nodes", NODES, "the layers"),
            req("visible", BOOL, "shown"),
        ],
        run: |ctx, a| {
            let visible = a.bool("visible").unwrap_or(true);
            each(ctx.doc, a, |id| Operation::SetVisible { id, visible })
        },
    },
    Verb {
        name: "set_locked",
        doc: "Lock or unlock layers",
        params: &[
            req("nodes", NODES, "the layers"),
            req("locked", BOOL, "locked"),
        ],
        run: |ctx, a| {
            let locked = a.bool("locked").unwrap_or(false);
            each(ctx.doc, a, |id| Operation::SetLocked { id, locked })
        },
    },
    Verb {
        name: "set_opacity",
        doc: "Set layers' opacity, 0 to 1",
        params: &[
            req("nodes", NODES, "the layers"),
            req("opacity", ParamKind::Number, "0 to 1"),
        ],
        run: |ctx, a| {
            let opacity = a.number("opacity").unwrap_or(1.0) as f32;
            if !build::valid_opacity(opacity) {
                return Err(a.invalid("opacity", "expected 0 to 1"));
            }
            each(ctx.doc, a, |id| Operation::SetOpacity { id, opacity })
        },
    },
    Verb {
        name: "set_blend",
        doc: "Set how layers blend with what is under them; Pass through is Normal on a layer that is not a frame or group",
        params: &[
            req(
                "nodes",
                NODES,
                "the layers; a member inside another is left to it",
            ),
            req("blend", ParamKind::Choice(BLENDS), "the mode"),
        ],
        run: |ctx, a| {
            let blend = blend_of(a.choice("blend").unwrap_or("Normal"));
            Ok(Done::edit(build::set_blend_all(
                ctx.doc,
                &a.layers(ctx.doc, "nodes")?,
                blend,
            )))
        },
    },
    Verb {
        name: "set_clip",
        doc: "Clip frames' contents to their boxes, or not",
        params: &[
            req("nodes", NODES, "the frames"),
            req("clip", BOOL, "clipped"),
        ],
        run: |ctx, a| {
            let clip = a.bool("clip").unwrap_or(true);
            each(ctx.doc, a, |id| Operation::SetClip { id, clip })
        },
    },
    Verb {
        name: "set_mask",
        doc: "Use layers as masks for the layers above each in its parent, or stop",
        params: &[
            req("nodes", NODES, "the layers"),
            req("mask", BOOL, "a mask"),
            opt(
                "mode",
                ParamKind::Choice(MASK_MODES),
                "what of the mask decides what survives: its outline (`shape`), its alpha, or its brightness (`luminance`); kept unless given",
            ),
        ],
        run: |ctx, a| {
            let mask = a.bool("mask").unwrap_or(true);
            let mode = match a.choice("mode") {
                Some(_) if !mask => return Err(a.invalid("mode", "only a mask has a mode")),
                Some(word) => Some(
                    MASK_MODES
                        .iter()
                        .position(|w| *w == word)
                        .map_or(crate::node::MaskMode::Shape, |i| {
                            crate::node::MaskMode::ALL[i]
                        }),
                ),
                None => None,
            };
            let mut ops = Vec::new();
            for id in a.layers(ctx.doc, "nodes")? {
                ops.push(Operation::SetMask { id, mask });
                if let Some(mode) = mode
                    && node(ctx, id)?.mask_mode() != mode
                {
                    ops.push(Operation::SetMaskMode { id, mode });
                }
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "set_element",
        doc: "Set the HTML element a code export writes a frame, group or text as, or Automatic; a variant writes every variant of its set",
        params: &[
            req("node", NODE, "the frame, group or text"),
            opt(
                "element",
                ParamKind::Choice(ELEMENTS),
                "its tag; absent for Automatic",
            ),
        ],
        run: |ctx, a| {
            use crate::element::{self, HtmlElement};
            let id = a.layer(ctx.doc, "node")?;
            let n = node(ctx, id)?;
            element_refusal(ctx, a, id)?;
            let element = match a.choice("element") {
                None => None,
                Some(tag) => {
                    let offered = element::offered(n.kind()).unwrap_or(&[]);
                    let e = offered
                        .iter()
                        .copied()
                        .find(|e| e.tag() == tag)
                        .ok_or_else(|| {
                            let tags: Vec<&str> = offered.iter().map(|e| e.tag()).collect();
                            a.invalid("element", format!("it takes {}", tags.join(", ")))
                        })?;
                    if let Some(why) = element::targets(ctx.doc, id)
                        .iter()
                        .find_map(|t| element::refusal(ctx.doc, *t, e))
                    {
                        return Err(a.invalid("element", why));
                    }
                    Some::<HtmlElement>(e)
                }
            };
            Ok(Done::ops(element::set_ops(ctx.doc, id, element)))
        },
    },
    Verb {
        name: "set_link",
        doc: "Set the address a layer written as a link (`a`) goes to; empty clears it",
        params: &[
            req("node", NODE, "the layer, its element `a`"),
            req(
                "href",
                TEXT,
                "an https:// or http:// address, a mailto: or tel: link, a /path or a #id",
            ),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            if node(ctx, id)?.element() != Some(crate::element::HtmlElement::Link) {
                return Err(a.invalid(
                    "node",
                    "only a layer written as a link takes an address: set its element to `a` first",
                ));
            }
            element_refusal(ctx, a, id)?;
            let href = a.text("href").unwrap_or_default().trim();
            if !crate::element::is_href(href) {
                return Err(a.invalid("href", "not an address a link can take"));
            }
            if node(ctx, id)?.href() == href {
                return Ok(Done::default());
            }
            Ok(Done::ops(vec![Operation::SetHref {
                id,
                href: href.to_owned(),
            }]))
        },
    },
    Verb {
        name: "set_description",
        doc: "Set the description of a main component or set, a style or a variable; empty clears it",
        params: &[
            opt("node", NODE, "a main component or set"),
            opt("style", ParamKind::Id("style"), "a style"),
            opt("variable", ParamKind::Id("variable"), "a variable"),
            req("text", TEXT, "the description"),
        ],
        run: |ctx, a| {
            use crate::op::Described;
            let of = match (a.get("node"), a.get("style"), a.get("variable")) {
                (Some(_), None, None) => Described::Node(component_owner(ctx, a)?),
                (None, Some(_), None) => Described::Style(own_style(ctx, a)?.id),
                (None, None, Some(_)) => Described::Variable(own_variable(ctx, a)?),
                _ => {
                    return Err(a.invalid("node", "give one of `node`, `style` and `variable`"));
                }
            };
            let text = a.text("text").unwrap_or_default().trim().to_owned();
            let now = match of {
                Described::Node(id) => node(ctx, id)?.description(),
                Described::Style(id) => ctx
                    .doc
                    .styles()
                    .get(id)
                    .map_or("", |s| s.description.as_str()),
                Described::Variable(id) => ctx
                    .doc
                    .variables()
                    .get(id)
                    .map_or("", |v| v.description.as_str()),
            };
            if now == text {
                return Ok(Done::default());
            }
            Ok(Done::ops(vec![Operation::SetDescription { of, text }]))
        },
    },
    Verb {
        name: "set_doc_link",
        doc: "Set the documentation address of a main component or set; empty clears it",
        params: &[
            req("node", NODE, "the main component or set"),
            req(
                "url",
                TEXT,
                "an http or https address; a bare host takes https://",
            ),
        ],
        run: |ctx, a| {
            let id = component_owner(ctx, a)?;
            let url = crate::node::web_link(a.text("url").unwrap_or_default())
                .map_err(|why| a.invalid("url", why))?;
            if node(ctx, id)?.doc_link() == url {
                return Ok(Done::default());
            }
            Ok(Done::ops(vec![Operation::SetDocLink { id, url }]))
        },
    },
    Verb {
        name: "convert_container",
        doc: "Turn a group into a frame of its own box, or a frame into a group keeping its look; answers what it became",
        params: &[req("node", NODE, "the group or frame")],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            if !matches!(
                node(ctx, id)?.kind(),
                NodeKind::Group | NodeKind::Artboard { .. }
            ) {
                return Err(a.invalid("node", "neither a group nor a frame"));
            }
            if let Some(why) = build::convert_container_refusal(ctx.doc, id) {
                return Err(a.invalid("node", why));
            }
            let (tx, new) = build::convert_container(ctx.doc, ctx.res, ctx.ids, id)?;
            Ok(Done::made(tx, vec![new]))
        },
    },
    Verb {
        name: "fit_to_content",
        doc: "Resize a frame without a layout to the box of what is inside it, everything inside staying where it is drawn",
        params: &[req("node", NODE, "the frame")],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let n = node(ctx, id)?;
            build::fit_frame(ctx.doc, ctx.res, id)
                .map(Done::edit)
                .ok_or_else(|| {
                    let why = match n.kind() {
                        NodeKind::Group => "a group always fits its content",
                        NodeKind::Artboard { .. } if n.display().is_some() => {
                            "a frame with a layout sizes itself: give its width and height fit-content with `set_layout_item`"
                        }
                        NodeKind::Artboard { .. } if n.children().is_empty() => {
                            "nothing inside to fit to"
                        }
                        NodeKind::Artboard { .. } => "already fits its content",
                        _ => "only a frame fits its content",
                    };
                    a.invalid("node", why)
                })
        },
    },
    Verb {
        name: "replace_font",
        doc: "Replace a font family everywhere the document sets it: layers, runs, text styles, charts and string variables",
        params: &[
            req("from", TEXT, "the family replaced, in any case"),
            req(
                "to",
                TEXT,
                "the family set instead, a name `list_fonts` answers",
            ),
        ],
        run: |ctx, a| {
            let (from, to) = (
                a.text("from").unwrap_or_default().trim(),
                a.text("to").unwrap_or_default().trim(),
            );
            if to.is_empty() {
                return Err(a.invalid("to", "a family needs a name"));
            }
            // A replacement within one family would still drop every match's
            // axis and feature settings, which go with the old typeface.
            if from.to_lowercase() == to.to_lowercase() {
                return Err(a.invalid("to", "it is the family replaced"));
            }
            let tx = crate::families::replace_family(ctx.doc, from, to);
            if tx.0.is_empty() {
                return Err(VerbError::Nothing(
                    "nothing of the document's own sets that family: no layer, run, text style, chart or variable, though a kit's may",
                ));
            }
            Ok(Done::edit(tx))
        },
    },
    // --- text ---------------------------------------------------------------
    Verb {
        name: "set_text",
        doc: "Replace a text layer's characters, dropping its runs of character styling",
        params: &[
            req("node", NODE, "the text layer"),
            req("text", TEXT, "the new text"),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetText {
                id: a.node("node")?,
                content: a.text("text").unwrap_or_default().to_owned(),
                spans: Default::default(),
                para_spans: Default::default(),
            }]))
        },
    },
    Verb {
        name: "replace_text",
        doc: "Replace what text layers say wherever `find` matches, each replacement styled as its match was; locked layers are left alone",
        params: &[
            req("find", TEXT, "what to look for"),
            req("replace", TEXT, "what takes its place"),
            opt(
                "nodes",
                NODES,
                "only the text in and under these layers; every page but drafts unless given",
            ),
            opt(
                "match_case",
                BOOL,
                "match capitals exactly; false unless given",
            ),
            opt(
                "whole_words",
                BOOL,
                "match only whole words; false unless given",
            ),
            opt(
                "pattern",
                BOOL,
                "`find` is a regular expression, and `[1]` in `replace` inserts its first group; false unless given",
            ),
            opt(
                "keep_case",
                BOOL,
                "give each replacement the capitals of what it replaces; false unless given",
            ),
            opt(
                "inside_instances",
                BOOL,
                "also override text inside instances; false unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::find::{self, Query, Scope};
            let flag = |name| a.bool(name).unwrap_or(false);
            let query = Query::new(
                a.text("find").unwrap_or_default(),
                a.text("replace").unwrap_or_default(),
                flag("match_case"),
                flag("whole_words"),
                flag("pattern"),
                flag("keep_case"),
            )
            .map_err(|why| a.invalid("find", why))?;
            let scope = match a.get("nodes") {
                Some(_) => Scope::Under(a.nodes("nodes")?),
                None => Scope::Pages {
                    here: ctx.doc.first_page(),
                },
            };
            let hits = find::find(ctx.doc, &scope, &query);
            let done = find::replace(ctx.doc, &hits, &query, flag("inside_instances"));
            if done.matches == 0 {
                return Err(VerbError::Nothing(match hits.is_empty() {
                    true => "nothing matches",
                    false => "every match is locked or inside an instance",
                }));
            }
            let alone = match hits.len() - done.matches {
                0 => String::new(),
                n => format!("; {n} locked or in an instance left alone"),
            };
            Ok(Done::edit(done.tx).saying(format!(
                "Replaced {} in {}{alone}",
                plural(done.matches, "match", "matches"),
                plural(done.layers, "layer", "layers"),
            )))
        },
    },
    Verb {
        name: "set_text_style",
        doc: "Set text layers' character style, and optionally one text's runs",
        params: &[
            req("nodes", NODES, "the text layers"),
            req(
                "style",
                ParamKind::Model("TextStyle"),
                "the fields to change; each text keeps the rest of its own",
            ),
            opt(
                "spans",
                ParamKind::Model("CharSpans"),
                "the runs over it, counted in one text's characters",
            ),
        ],
        run: |ctx, a| {
            let ids = texts(ctx, a)?;
            let mut patch = a.get("style").cloned().unwrap_or_default();
            loose::loosen("", &mut patch);
            let Value::Object(patch) = patch else {
                return Err(a.invalid("style", "expected an object"));
            };
            let spans = a.model_opt::<CharSpans>("spans")?;
            if spans.is_some() && ids.len() > 1 {
                return Err(a.invalid("spans", "runs are counted in one text's characters"));
            }
            let mut ops = Vec::new();
            for id in ids {
                let Some(NodeKind::Text { style, .. }) = ctx.doc.get(id).map(|n| n.kind()) else {
                    continue;
                };
                let mut whole = serde_json::to_value(style.as_ref()).unwrap_or_default();
                if let Value::Object(map) = &mut whole {
                    map.extend(patch.clone());
                }
                let style =
                    read_model::<TextStyle>(&whole).map_err(|why| a.invalid("style", why))?;
                ops.push(Operation::SetTextStyle {
                    id,
                    style,
                    spans: spans.clone(),
                });
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "set_text_spans",
        doc: "Set a text layer's runs of character styling",
        params: &[
            req("node", NODE, "the text layer"),
            req("spans", ParamKind::Model("CharSpans"), "the runs"),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetTextSpans {
                id: a.node("node")?,
                spans: a.model("spans")?,
            }]))
        },
    },
    Verb {
        name: "set_paragraph_style",
        doc: "Set text layers' paragraph style",
        params: &[
            req("nodes", NODES, "the text layers"),
            req("paragraph", ParamKind::Model("ParagraphStyle"), "the style"),
        ],
        run: |ctx, a| {
            let paragraph = a.model::<ParagraphStyle>("paragraph")?;
            Ok(Done::ops(
                texts(ctx, a)?
                    .into_iter()
                    .map(|id| Operation::SetParagraphStyle {
                        id,
                        paragraph: paragraph.clone(),
                        spans: None,
                    })
                    .collect(),
            ))
        },
    },
    Verb {
        name: "set_block_style",
        doc: "Set text layers' block style: the box, overflow and vertical alignment",
        params: &[
            req("nodes", NODES, "the text layers"),
            req("block", ParamKind::Model("BlockStyle"), "the style"),
        ],
        run: |ctx, a| {
            let block = a.model::<BlockStyle>("block")?;
            Ok(Done::ops(
                texts(ctx, a)?
                    .into_iter()
                    .map(|id| Operation::SetBlockStyle { id, block })
                    .collect(),
            ))
        },
    },
    Verb {
        name: "style_text",
        doc: "Set character attributes on a text layer, over all of it or a range of its characters",
        params: &[
            req("node", NODE, "the text layer"),
            req(
                "attrs",
                ParamKind::Model("[CharAttr]"),
                "the attributes to set",
            ),
            opt(
                "start",
                INDEX,
                "the first character styled, counted from 0; 0 unless given",
            ),
            opt(
                "end",
                INDEX,
                "the character after the last styled; the end unless given",
            ),
        ],
        run: |ctx, a| {
            let attrs = a.model::<Vec<CharAttr>>("attrs")?;
            let (t, content) = text_of(ctx, a)?;
            let (range, whole) = char_range(a, content)?;
            Ok(Done::edit(if whole {
                build::char_defaults(&t, range, attrs).0
            } else {
                build::char_runs(&t, range, attrs)
            }))
        },
    },
    Verb {
        name: "style_paragraphs",
        doc: "Set paragraph attributes on a text layer, over all of it or the paragraphs a range of its characters reaches",
        params: &[
            req("node", NODE, "the text layer"),
            req(
                "attrs",
                ParamKind::Model("[ParaAttr]"),
                "the attributes to set",
            ),
            opt(
                "start",
                INDEX,
                "the first character of the range, counted from 0; 0 unless given",
            ),
            opt(
                "end",
                INDEX,
                "the character after the range; the end unless given",
            ),
        ],
        run: |ctx, a| {
            let attrs = a.model::<Vec<ParaAttr>>("attrs")?;
            let (t, content) = text_of(ctx, a)?;
            let (range, whole) = char_range(a, content)?;
            Ok(Done::edit(if whole {
                build::para_defaults(&t, range, attrs).0
            } else {
                build::para_runs(&t, content, range, attrs)
            }))
        },
    },
    // --- paint --------------------------------------------------------------
    Verb {
        name: "set_fills",
        doc: "Replace layers' fills, bottom first, as the Fill card writes a selection: a group's reach its shapes",
        params: &[
            req("nodes", NODES, "the layers"),
            req("fills", ParamKind::Model("[Fill]"), "the fills"),
        ],
        run: |ctx, a| {
            let ids = painted(ctx, a)?;
            let fills = a.model::<Vec<Fill>>("fills")?;
            Ok(Done::edit(match build::shared_fills(ctx.doc, &ids) {
                build::PaintShown::List(anchor) => {
                    let edited = rekeyed(ctx, &anchor, fills);
                    build::retarget_fills_all(ctx.doc, &ids, &anchor, &edited, ctx.ids)
                }
                build::PaintShown::Mixed => build::set_fills_all(ctx.doc, &ids, &fills, ctx.ids),
            }))
        },
    },
    Verb {
        name: "set_strokes",
        doc: "Replace layers' strokes, bottom first, as the Stroke card writes a selection: a group's reach its shapes",
        params: &[
            req("nodes", NODES, "the layers"),
            req("strokes", ParamKind::Model("[Stroke]"), "the strokes"),
        ],
        run: |ctx, a| {
            let ids = painted(ctx, a)?;
            let strokes = a.model::<Vec<Stroke>>("strokes")?;
            Ok(Done::edit(match build::shared_strokes(ctx.doc, &ids) {
                build::PaintShown::List(anchor) => {
                    let edited = rekeyed(ctx, &anchor, strokes);
                    build::retarget_strokes_all(ctx.doc, &ids, &anchor, &edited, ctx.ids)
                }
                build::PaintShown::Mixed => {
                    build::set_strokes_all(ctx.doc, &ids, &strokes, ctx.ids)
                }
            }))
        },
    },
    Verb {
        name: "set_effects",
        doc: "Replace layers' effects, shadows and blurs in order, on the outermost of them",
        params: &[
            req("nodes", NODES, "the layers"),
            req("effects", ParamKind::Model("[Effect]"), "the effects"),
        ],
        run: |ctx, a| {
            let ids = existing_layers(ctx, a)?;
            let effects = a.model::<Vec<Effect>>("effects")?;
            Ok(Done::edit(build::set_effects_all(
                ctx.doc, &ids, &effects, ctx.ids,
            )))
        },
    },
    Verb {
        name: "merge_colours",
        doc: "Make near-identical colours one, as Colour review merges a group: every place painted in one of `colours` takes `into`",
        params: &[
            req(
                "colours",
                ParamKind::Model("[Hex]"),
                "the colours to replace, `#rrggbbaa` as `review_colours` writes them",
            ),
            req(
                "into",
                TEXT,
                "the colour they become, `#rrggbbaa`, one the document uses; a variable's colour binds them to it",
            ),
            opt(
                "nodes",
                NODES,
                "only the layers in and under these; every page unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::color_review::{self, Group};
            let parse = |param: &'static str, s: &str| {
                peniko::color::parse_color(s.trim())
                    .map(|c| {
                        c.to_alpha_color::<peniko::color::Srgb>()
                            .to_rgba8()
                            .to_u8_array()
                    })
                    .map_err(|_| a.invalid(param, format!("`{s}` is no colour")))
            };
            let into = parse("into", a.text("into").unwrap_or_default())?;
            let from = a
                .model::<Vec<String>>("colours")?
                .iter()
                .map(|s| parse("colours", s))
                .collect::<Result<Vec<_>, _>>()?;
            let scope = match a.get("nodes") {
                Some(_) => Some(a.nodes("nodes")?),
                None => None,
            };
            let roots = color_review::roots(ctx.doc, scope.as_deref());
            let census = color_review::census(ctx.doc, &roots);
            let raw = |i: usize| {
                census[i]
                    .is_raw()
                    .then(|| census[i].color.to_rgba8().to_u8_array())
            };
            // A variable that is this colour first, as Colour review's default
            // target: the raw paints are then bound to it.
            let bound: Vec<usize> = (0..census.len())
                .filter(|i| {
                    !census[*i].is_raw() && census[*i].color.to_rgba8().to_u8_array() == into
                })
                .collect();
            let target = match bound.as_slice() {
                [one] => *one,
                [] => (0..census.len())
                    .find(|i| raw(*i) == Some(into))
                    .ok_or_else(|| a.invalid("into", "no paint in scope is this colour"))?,
                _ => {
                    return Err(a.invalid(
                        "into",
                        "several variables are this colour; bind the paints with `bind_variable`",
                    ));
                }
            };
            let mut members = vec![target];
            members.extend(
                (0..census.len())
                    .filter(|i| *i != target && raw(*i).is_some_and(|b| from.contains(&b))),
            );
            if members.len() < 2 {
                return Err(VerbError::Nothing("none of the colours is used in scope"));
            }
            let merged = color_review::merge(ctx.doc, &roots, &census, &Group { members }, 0);
            if merged.uses == 0 {
                return Err(VerbError::Nothing("every place is on a locked layer"));
            }
            let locked = match merged.locked {
                0 => String::new(),
                n => format!(
                    "; {} on locked layers left alone",
                    plural(n, "place", "places")
                ),
            };
            Ok(Done::edit(merged.tx).saying(format!(
                "Merged {} on {}{locked}",
                plural(merged.uses, "place", "places"),
                plural(merged.layers, "layer", "layers"),
            )))
        },
    },
    Verb {
        name: "set_exports",
        doc: "Replace layers' export settings",
        params: &[
            req("nodes", NODES, "the layers"),
            req("exports", ParamKind::Model("[ExportSpec]"), "the settings"),
        ],
        run: |ctx, a| {
            let exports = a.model::<Vec<ExportSpec>>("exports")?;
            let mut ops = Vec::new();
            // One id for a row past any layer's end, shared across the layers, as
            // the Export card gives a row added over a selection.
            let mut fresh = PastTheEnd::default();
            for id in existing_layers(ctx, a)? {
                let own = node(ctx, id)?.exports().to_vec();
                let exports = rekey_by_position(&own, exports.clone(), &mut fresh, ctx.ids);
                ops.push(Operation::SetExports { id, exports });
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "add_image",
        doc: "Add a picture to the document for a fill to show; answers its image id",
        params: &[req(
            "bytes",
            TEXT,
            "the PNG, JPEG, WebP or GIF file, in base64",
        )],
        run: |ctx, a| {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(a.text("bytes").unwrap_or_default())
                .map_err(|e| a.invalid("bytes", e))?;
            let format = crate::foreign::sniff(&bytes)
                .ok_or_else(|| a.invalid("bytes", "not a PNG, JPEG, WebP or GIF file"))?;
            let sink = ctx
                .images
                .as_mut()
                .ok_or(VerbError::Nothing("this caller cannot read pictures"))?;
            let (id, entry) = sink
                .place(bytes, format)
                .ok_or_else(|| a.invalid("bytes", "the picture does not decode"))?;
            // A picture already in the table is the same bytes: one entry, two users.
            let ops = if ctx.doc.has_image(&id) {
                Vec::new()
            } else {
                vec![Operation::AddImage {
                    id: id.clone(),
                    entry,
                }]
            };
            Ok(Done::making(ops, id.0))
        },
    },
    Verb {
        name: "replace_image",
        doc: "Show another picture in a layer's fill, keeping how the old one was framed",
        params: &[
            req("node", NODE, "the layer"),
            req("image", TEXT, "the picture, by the id `add_image` answered"),
            opt(
                "index",
                INDEX,
                "the fill, 0 the bottom; its first picture fill unless given",
            ),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            let image = crate::image::ImageId(a.text("image").unwrap_or_default().to_owned());
            if !ctx.doc.has_image(&image) {
                return Err(a.invalid("image", "the document holds no such picture; add it first"));
            }
            let fills = &node(ctx, id)?.paint().fills;
            let index = a.index("index").unwrap_or_else(|| {
                fills
                    .iter()
                    .position(|f| matches!(f.brush.literal(), Some(crate::Brush::Image(_))))
                    .unwrap_or(0)
            });
            build::replace_image_fill(ctx.doc, id, index, &image, None)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("index", "the layer has no fill there"))
        },
    },
    Verb {
        name: "fix_contrast",
        doc: "Give layers whose contrast fails the nearest colour that passes, or a colour or variable you name",
        params: &[
            req("nodes", NODES, "text layers and small shapes"),
            opt(
                "level",
                ParamKind::Choice(LEVELS),
                "the standard: `aa` unless given, `aaa` or `apca`",
            ),
            opt(
                "colour",
                ParamKind::Model("Color"),
                "the colour the failing paints take; the nearest passing one unless given",
            ),
            opt(
                "variable",
                ParamKind::Id("variable"),
                "a colour variable the failing paints are bound to instead",
            ),
        ],
        run: |ctx, a| {
            use crate::contrast::{Fix, Level};
            let level = match a.choice("level") {
                Some("aaa") => Level::Aaa,
                Some("apca") => Level::Apca,
                _ => Level::Aa,
            };
            let named = match (
                a.model_opt::<peniko::Color>("colour")?,
                a.id_opt::<VarId>("variable")?,
            ) {
                (Some(_), Some(_)) => {
                    return Err(a.invalid("variable", "give a colour or a variable, not both"));
                }
                (Some(c), None) => Some(Fix::Colour(c)),
                (None, Some(v)) => {
                    let var = ctx
                        .doc
                        .variables()
                        .get(v)
                        .ok_or_else(|| a.invalid("variable", "no variable has that id"))?;
                    if var.kind != VarKind::Color {
                        return Err(a.invalid("variable", "not a colour variable"));
                    }
                    Some(Fix::Var(v))
                }
                (None, None) => None,
            };
            let nodes = a.nodes("nodes")?;
            for id in &nodes {
                node(ctx, *id)?;
            }
            let probe = ctx
                .contrast
                .as_mut()
                .ok_or(VerbError::Nothing("this caller cannot measure contrast"))?;
            let measured = probe.measure(ctx.doc, &nodes, level);
            // What was given and is left unfixed for a reason other than passing.
            let named_as = |id: NodeId, why: &str| {
                let name = ctx.doc.get(id).map_or("", |n| n.name());
                format!("{name} ({why})")
            };
            let mut unfixed: Vec<String> = measured
                .skipped
                .iter()
                .map(|(id, why)| named_as(*id, why))
                .collect();
            unfixed.extend(
                nodes
                    .iter()
                    .filter(|id| {
                        !measured.findings.iter().any(|f| f.id == **id)
                            && !measured.skipped.iter().any(|(s, _)| s == *id)
                    })
                    .map(|id| named_as(*id, "not a shown text layer or small shape")),
            );
            let mut ops = Vec::new();
            for f in measured.findings.iter().filter(|f| !f.passes()) {
                let fix = match named {
                    Some(fix) => fix,
                    None => match f
                        .worst()
                        .and_then(|w| crate::contrast::nearest(f, level, w.ink))
                    {
                        Some(c) => Fix::Colour(c),
                        None => {
                            unfixed.push(named_as(f.id, "no lightness at its hue passes"));
                            continue;
                        }
                    },
                };
                let new_fill = ctx.ids.mint_item();
                if let Some(tx) =
                    crate::contrast::fix_transaction(ctx.doc, f.id, f, level, fix, new_fill)
                {
                    ops.extend(tx.0);
                }
            }
            // Nothing changed is "they pass" only when every layer was measured.
            if ops.is_empty() && !unfixed.is_empty() {
                return Err(a.invalid("nodes", format!("could not fix {}", unfixed.join(", "))));
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "clean_up",
        doc: "Take away what draws nothing: hidden paint, covered fills, hidden and empty \
              layers, one-layer groups, guides; what would change a pixel is left",
        params: &[
            opt(
                "nodes",
                NODES,
                "the layers to clean, with everything under them; every page but drafts \
                 unless given",
            ),
            opt(
                "hidden_paint",
                ParamKind::Bool,
                "hidden fills, strokes, effects and grids; on unless given",
            ),
            opt(
                "covered_fills",
                ParamKind::Bool,
                "fills under an opaque solid fill; on unless given",
            ),
            opt(
                "hidden_layers",
                ParamKind::Bool,
                "hidden layers; off unless given",
            ),
            opt(
                "empty_layers",
                ParamKind::Bool,
                "empty layers; on unless given",
            ),
            opt(
                "one_layer_groups",
                ParamKind::Bool,
                "groups holding one layer; on unless given",
            ),
            opt(
                "guides",
                ParamKind::Bool,
                "ruler guides in scope; off unless given",
            ),
            opt(
                "skip_locked",
                ParamKind::Bool,
                "leave locked layers alone; on unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::clean_up::{Rule, Rules, Scope};
            let scope = match a.get("nodes") {
                Some(_) => Scope::Layers(a.nodes("nodes")?),
                None => Scope::Pages(ctx.doc.gathered_pages().collect()),
            };
            let mut rules = Rules::default();
            for rule in Rule::ALL {
                if let Some(on) = a.bool(rule.key()) {
                    rules.set(rule, on);
                }
            }
            if let Some(skip) = a.bool("skip_locked") {
                rules.skip_locked = skip;
            }
            Ok(Done::edit(
                crate::clean_up::plan(ctx.doc, ctx.res, &scope, &rules).tx,
            ))
        },
    },
    Verb {
        name: "fill_content",
        doc: "Fill text layers with realistic names, addresses, companies, text, numbers and \
              dates, and avatar shapes with initials or a pattern",
        params: &[
            req(
                "nodes",
                NODES,
                "the layers to fill, with every text layer under them",
            ),
            opt(
                "locale",
                TEXT,
                "a shipped locale's tag (en-US, en-GB, de, fr, es, it, pt-BR, nl); the \
                 layers' own Language when they agree, else en-US, unless given",
            ),
            opt(
                "kind",
                ParamKind::Choice(&crate::content_fill::KIND_KEYS),
                "one kind for every text layer; each read from its name, then its text, \
                 unless given",
            ),
            opt(
                "records",
                ParamKind::Bool,
                "keep each frame's values together; on unless given",
            ),
            opt(
                "avatars",
                ParamKind::Choice(&["initials", "pattern", "none"]),
                "how shapes named as avatars are drawn; initials unless given",
            ),
            opt(
                "seed",
                INDEX,
                "the draw: one seed always fills the same values; 0 unless given",
            ),
            opt(
                "example_emails",
                ParamKind::Bool,
                "every email at example.com, which reaches no one; off unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::content_fill::{self as fill, AvatarStyle, Kind, Options};
            let nodes = a.nodes("nodes")?;
            let locale = match a.text("locale") {
                Some(tag) => fill::locale_for(tag)
                    .ok_or_else(|| a.invalid("locale", "not one of the shipped locales"))?,
                None => fill::agreed_locale(ctx.doc, &nodes)
                    .or_else(|| fill::locales().first())
                    .ok_or(VerbError::Nothing("no locale is shipped"))?,
            };
            let opts = Options {
                locale: locale.tag.clone(),
                content: a.choice("kind").and_then(Kind::from_key),
                records: a.bool("records").unwrap_or(true),
                avatars: match a.choice("avatars") {
                    Some("pattern") => Some(AvatarStyle::Pattern),
                    Some("none") => None,
                    _ => Some(AvatarStyle::Initials),
                },
                seed: a.index("seed").unwrap_or(0) as u64,
                example_emails: a.bool("example_emails").unwrap_or(false),
                ..Options::default()
            };
            let plan = fill::plan(ctx.doc, ctx.res, &nodes, &opts);
            Ok(Done::edit(plan.transaction(
                ctx.doc,
                ctx.res,
                ctx.images.as_deref_mut(),
            )))
        },
    },
    // --- layout -------------------------------------------------------------
    Verb {
        name: "set_display",
        doc: "Make frames lay out their children as flex or grid, or stop",
        params: &[
            req("nodes", NODES, "the frames"),
            opt(
                "display",
                ParamKind::Model("Display"),
                "the layout; absent turns it off",
            ),
        ],
        run: |ctx, a| {
            let display = a.model_opt::<Display>("display")?;
            each(ctx.doc, a, |id| Operation::SetDisplay {
                id,
                display: display.clone(),
            })
        },
    },
    Verb {
        name: "set_layout_item",
        doc: "Set how layers size and place themselves in their parents' layouts",
        params: &[
            req("nodes", NODES, "the layers"),
            req(
                "item",
                ParamKind::Model("LayoutItem"),
                "their item properties",
            ),
        ],
        run: |ctx, a| {
            let item = a.model::<LayoutItem>("item")?;
            each(ctx.doc, a, |id| Operation::SetLayoutItem { id, item })
        },
    },
    Verb {
        name: "set_insets",
        doc: "Pin layers to their frames' sides, as CSS insets and auto margins",
        params: &[
            req("nodes", NODES, "the layers"),
            req("insets", ParamKind::Model("Insets"), "the four sides"),
        ],
        run: |ctx, a| {
            let insets = a.model::<Insets>("insets")?;
            each(ctx.doc, a, |id| Operation::SetInsets { id, insets })
        },
    },
    // --- charts -------------------------------------------------------------
    Verb {
        name: "insert_chart",
        doc: "Make a chart of `kind` in `parent`, on its own table or the kind's sample; answers its frame",
        params: &[
            req("parent", NODE, "the frame, group or page to make it in"),
            req("kind", ParamKind::Choice(CHART_KINDS), "what it draws"),
            opt(
                "table",
                ParamKind::Model("ChartTable"),
                "its data; the kind's sample table unless given",
            ),
            opt(
                "mapping",
                ParamKind::Model("ChartMapping"),
                "which columns it reads, by id; read off the column types unless given",
            ),
            opt(
                "x",
                ParamKind::Number,
                "its frame's left edge in the parent; 0 unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "its frame's top edge in the parent; 0 unless given",
            ),
            opt("width", ParamKind::Number, "its width; 480 unless given"),
            opt("height", ParamKind::Number, "its height; 280 unless given"),
            opt(
                "index",
                INDEX,
                "its place among the parent's children, 0 the bottom",
            ),
            opt("name", TEXT, "its name; Chart unless given"),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let kind = chart_kind(a.choice("kind").unwrap_or_default());
            let mut chart = crate::chart::Chart::sample(kind);
            if let Some(table) = a.model_opt::<crate::chart::Table>("table")? {
                chart.mapping = crate::chart::Mapping::guess(kind, &table);
                chart.table = table;
            }
            if let Some(mapping) = a.model_opt("mapping")? {
                chart.mapping = mapping;
            }
            chart_valid(a, &chart)?;
            let mut size = [480.0, 280.0];
            for (i, p) in ["width", "height"].into_iter().enumerate() {
                match a.number(p) {
                    Some(v) if v <= 0.0 || v > build::GEOMETRY_LIMIT => {
                        let why = format!(
                            "a side is more than 0 and at most {}",
                            build::GEOMETRY_LIMIT
                        );
                        return Err(a.invalid(p, why));
                    }
                    Some(v) => size[i] = v,
                    None => {}
                }
            }
            let at = kurbo::Vec2::new(a.number("x").unwrap_or(0.0), a.number("y").unwrap_or(0.0));
            let id = ctx.ids.mint();
            let tx = Transaction(vec![
                Operation::CreateNode {
                    id,
                    parent,
                    index: a.index("index").unwrap_or(count).min(count),
                    kind: NodeKind::Artboard {
                        size: kurbo::Size::new(size[0], size[1]),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: Some(Affine::translate(at)),
                    name: Some(a.text("name").unwrap_or("Chart").to_owned()),
                },
                Operation::SetChart {
                    id,
                    chart: Some(Box::new(chart)),
                },
            ]);
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "set_chart",
        doc: "Change a chart's kind, or patch its mapping or options; its parts follow. A switch to or from a Sankey re-reads the mapping off the table's columns before `mapping` patches it",
        params: &[
            req("node", NODE, "the chart"),
            opt("kind", ParamKind::Choice(CHART_KINDS), "what it draws"),
            opt(
                "mapping",
                ParamKind::Model("ChartMapping"),
                "the roles to change, by column id; a null clears one",
            ),
            opt(
                "options",
                ParamKind::Model("ChartOptions"),
                "the options to change, nested as the chart holds them; a null resets one",
            ),
        ],
        run: |ctx, a| {
            let (id, mut chart) = chart_of(ctx, a)?;
            if a.get("kind").is_none() && a.get("mapping").is_none() && a.get("options").is_none() {
                return Err(VerbError::Nothing(
                    "`set_chart` changes `kind`, `mapping` or `options`",
                ));
            }
            if let Some(word) = a.choice("kind") {
                chart = chart.with_kind(chart_kind(word));
            }
            if let Some(patch) = a.get("mapping") {
                chart.mapping = patched(a, "mapping", &chart.mapping, patch)?;
            }
            if let Some(patch) = a.get("options") {
                let (palette, dark) = (chart.options.palette, chart.options.dark);
                chart.options = patched(a, "options", &chart.options, patch)?;
                // A palette pick and a theme switch carry the colours with them,
                // as the card's do.
                let dark = std::mem::replace(&mut chart.options.dark, dark);
                if chart.options.palette != palette {
                    chart = crate::chart::with_palette(&chart, chart.options.palette);
                }
                chart = crate::chart::with_dark(&chart, dark);
            }
            chart_valid(a, &chart)?;
            Ok(Done::ops(vec![Operation::SetChart {
                id,
                chart: Some(Box::new(chart)),
            }]))
        },
    },
    Verb {
        name: "set_chart_data",
        doc: "Replace a chart's table, keeping the mapping and series colours of each column whose name survives; unlinks its file",
        params: &[
            req("node", NODE, "the chart"),
            req(
                "table",
                ParamKind::Model("ChartTable"),
                "its new data; column ids are its own",
            ),
        ],
        run: |ctx, a| {
            let (id, chart) = chart_of(ctx, a)?;
            let mut chart = chart.with_table(a.model("table")?);
            chart.source = None;
            chart_valid(a, &chart)?;
            Ok(Done::ops(vec![Operation::SetChart {
                id,
                chart: Some(Box::new(chart)),
            }]))
        },
    },
    Verb {
        name: "flatten_chart",
        doc: "Drop charts' descriptions, leaving their parts as ordinary layers where they are",
        params: &[req("nodes", NODES, "the charts")],
        run: |ctx, a| {
            let ops = a
                .layers(ctx.doc, "nodes")?
                .into_iter()
                .map(|id| {
                    crate::chart::flatten_ops(ctx.doc, id).ok_or_else(|| {
                        a.invalid("nodes", format!("`{}` is not a chart", id.to_wire()))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Done::ops(ops.into_iter().flatten().collect()))
        },
    },
];

/// The chart kinds by the word a verb takes, in [`crate::chart::ChartKind::ALL`]'s order.
const CHART_KINDS: &[&str] = &["column", "bar", "line", "area", "pie", "scatter", "sankey"];

/// The chart kind a [`CHART_KINDS`] word names; Column for none.
fn chart_kind(word: &str) -> crate::chart::ChartKind {
    CHART_KINDS
        .iter()
        .position(|w| *w == word)
        .map_or(crate::chart::ChartKind::Column, |i| {
            crate::chart::ChartKind::ALL[i]
        })
}

/// The chart the `node` argument names and a copy of its description.
fn chart_of(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<(NodeId, crate::chart::Chart), VerbError> {
    let id = a.node("node")?;
    let chart = node(ctx, id)?
        .chart()
        .ok_or_else(|| a.invalid("node", "not a chart"))?;
    Ok((id, chart.clone()))
}

/// `chart`, refused as `SetChart` would refuse it, the reason said as a fault of
/// the table, the mapping or the options a caller sent.
fn chart_valid(a: &Args<'_>, chart: &crate::chart::Chart) -> Result<(), VerbError> {
    use crate::chart::ChartRule as R;
    chart.validate().map_err(|rule| {
        let param = match rule {
            R::UnknownColumn => "mapping",
            R::BadOption => "options",
            _ => "table",
        };
        let param = if a.verb.params.iter().any(|p| p.name == param) {
            param
        } else {
            "node"
        };
        a.invalid(param, rule)
    })
}

/// `own` with `patch` merged over it as a JSON merge patch (RFC 7396): an object
/// merges key by key, a null sets the key back to its value in `T::default()`
/// (not a nested struct's own default, which may differ), anything else
/// replaces it.
fn patched<T: serde::Serialize + serde::de::DeserializeOwned + Default>(
    a: &Args<'_>,
    param: &'static str,
    own: &T,
    patch: &Value,
) -> Result<T, VerbError> {
    fn merge(into: &mut Value, patch: &Value, default: Option<&Value>) {
        match (into, patch) {
            (Value::Object(into), Value::Object(patch)) => {
                for (k, v) in patch {
                    let default = default.and_then(|d| d.get(k));
                    if v.is_null() {
                        match default {
                            Some(d) => into.insert(k.clone(), d.clone()),
                            None => into.remove(k),
                        };
                    } else {
                        merge(into.entry(k.clone()).or_insert(Value::Null), v, default);
                    }
                }
            }
            (into, patch) => *into = patch.clone(),
        }
    }
    let mut v = serde_json::to_value(own).map_err(|e| a.invalid(param, e))?;
    let default = serde_json::to_value(T::default()).map_err(|e| a.invalid(param, e))?;
    merge(&mut v, patch, Some(&default));
    serde_json::from_value(v).map_err(|e| a.invalid(param, e))
}

/// One layer of `insert_subtree`'s tree: what `create_node` takes, the properties
/// the single-layer verbs set, each in the same form, and the layers inside it.
/// Ids are minted as it is created, so a caller never writes one.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Tree {
    pub kind: NodeKind,
    pub name: Option<String>,
    pub transform: Option<Affine>,
    pub visible: Option<bool>,
    pub locked: Option<bool>,
    pub opacity: Option<f32>,
    /// A [`BLENDS`] word.
    pub blend: Option<String>,
    pub clip: Option<bool>,
    pub mask: Option<bool>,
    /// A [`MASK_MODES`] word.
    pub mask_mode: Option<String>,
    pub fills: Option<Vec<Fill>>,
    pub strokes: Option<Vec<Stroke>>,
    pub effects: Option<Vec<Effect>>,
    pub exports: Option<Vec<ExportSpec>>,
    pub display: Option<Display>,
    pub item: Option<LayoutItem>,
    pub insets: Option<Insets>,
    /// Bottom first.
    #[serde(default)]
    pub children: Vec<Tree>,
}

/// The ops creating `tree` at `index` in `parent`, then its children inside it,
/// appended to `done` with each layer it mints. A layer is created before its
/// properties are set, so each op meets the layer it names.
fn tree_ops(
    ctx: &mut Ctx<'_>,
    a: &Args<'_>,
    tree: &Tree,
    parent: NodeId,
    index: usize,
    done: &mut Done,
) -> Result<(), VerbError> {
    let id = ctx.ids.mint();
    done.created.push(id);
    let ops = &mut done.tx.0;
    ops.push(Operation::CreateNode {
        id,
        parent,
        index,
        kind: tree.kind.clone(),
        transform: tree.transform,
        name: tree.name.clone(),
    });
    if let Some(visible) = tree.visible {
        ops.push(Operation::SetVisible { id, visible });
    }
    if let Some(locked) = tree.locked {
        ops.push(Operation::SetLocked { id, locked });
    }
    if let Some(opacity) = tree.opacity {
        if !build::valid_opacity(opacity) {
            return Err(a.invalid("tree", "an opacity is 0 to 1"));
        }
        ops.push(Operation::SetOpacity { id, opacity });
    }
    if let Some(word) = &tree.blend {
        if !BLENDS.contains(&word.as_str()) {
            let why = format!("a blend is one of {}", BLENDS.join(", "));
            return Err(a.invalid("tree", why));
        }
        // As `build::set_blend_all` lands it, for a layer it cannot see yet.
        let blend = match blend_of(word) {
            m if m.allowed_on(&tree.kind) => m,
            _ => BlendMode::Normal,
        };
        ops.push(Operation::SetBlend { id, blend });
    }
    if let Some(clip) = tree.clip {
        ops.push(Operation::SetClip { id, clip });
    }
    if let Some(mask) = tree.mask {
        ops.push(Operation::SetMask { id, mask });
    }
    if let Some(word) = &tree.mask_mode {
        let at = MASK_MODES.iter().position(|w| w == word).ok_or_else(|| {
            a.invalid(
                "tree",
                format!("a mask mode is one of {}", MASK_MODES.join(", ")),
            )
        })?;
        let mode = crate::node::MaskMode::ALL[at];
        ops.push(Operation::SetMaskMode { id, mode });
    }
    if let Some(fills) = &tree.fills {
        let fills = rekeyed(ctx, &[], fills.clone());
        done.tx.0.push(Operation::SetFills { id, fills });
    }
    if let Some(strokes) = &tree.strokes {
        let strokes = rekeyed(ctx, &[], strokes.clone());
        done.tx.0.push(Operation::SetStrokes { id, strokes });
    }
    if let Some(effects) = &tree.effects {
        let effects = rekeyed(ctx, &[], effects.clone());
        done.tx.0.push(Operation::SetEffects { id, effects });
    }
    if let Some(exports) = &tree.exports {
        let exports = rekeyed(ctx, &[], exports.clone());
        done.tx.0.push(Operation::SetExports { id, exports });
    }
    let ops = &mut done.tx.0;
    if let Some(display) = &tree.display {
        ops.push(Operation::SetDisplay {
            id,
            display: Some(display.clone()),
        });
    }
    if let Some(insets) = tree.insets {
        ops.push(Operation::SetInsets { id, insets });
    }
    if let Some(item) = tree.item {
        ops.push(Operation::SetLayoutItem { id, item });
    }
    for (i, child) in tree.children.iter().enumerate() {
        tree_ops(ctx, a, child, id, i, done)?;
    }
    Ok(())
}

/// Layer `id` and the layers inside it as a [`Tree`], the shape `insert_subtree`
/// takes and the setters write: its kind with a text's runs, every fill and stroke
/// with the hidden ones, its effects and exports, each field only where it differs
/// from a new layer's, and no item ids. A binding a paint, stroke, effect or layout
/// item carries travels in it; any other bound field (size, corners, text, gaps,
/// padding, flow, visibility, opacity) is written as drawn, unbound. Only what
/// `Tree` holds is read: no styles, modes, width queries, pivot, element, link or
/// component state. `None` for no such layer. Every value in its full shape;
/// [`tree`] writes the short forms.
fn full_tree(doc: &Document, id: NodeId) -> Option<Value> {
    fn values<T: serde::Serialize>(list: &[Keyed<T>]) -> Value {
        serde_json::to_value(list.iter().map(|k| &k.value).collect::<Vec<_>>()).unwrap_or_default()
    }
    let n = doc.get(id)?;
    let mut out = Map::new();
    let mut put = |k: &str, v: Value| {
        out.insert(k.to_owned(), v);
    };
    // A bound field's stored literal is a placeholder, so each is written as drawn.
    let mut kind = doc
        .resolved_kind(id)
        .map_or_else(|| n.kind().clone(), std::borrow::Cow::into_owned);
    if let Some(radii) = kind.corner_radii().and_then(|r| doc.resolve_corners(id, r)) {
        kind.set_corner_radii(radii);
    }
    put("kind", serde_json::to_value(&kind).unwrap_or_default());
    put("name", Value::from(n.name()));
    if n.transform() != Affine::IDENTITY {
        put(
            "transform",
            serde_json::to_value(n.transform()).unwrap_or_default(),
        );
    }
    if !doc.shown(id) {
        put("visible", Value::Bool(false));
    }
    if n.locked() {
        put("locked", Value::Bool(true));
    }
    let opacity = doc.drawn_opacity(id);
    if opacity != 1.0 {
        put("opacity", Value::from(opacity));
    }
    if n.blend() != BlendMode::default_for(n.kind()) {
        put("blend", Value::from(n.blend().label()));
    }
    if n.clip() {
        put("clip", Value::Bool(true));
    }
    if n.mask() {
        put("mask", Value::Bool(true));
        let at = crate::node::MaskMode::ALL
            .iter()
            .position(|m| *m == n.mask_mode())
            .unwrap_or(0);
        put("mask_mode", Value::from(MASK_MODES[at]));
    }
    let paint = n.paint();
    if !paint.fills.is_empty() {
        put("fills", values(&paint.fills));
    }
    if !paint.strokes.is_empty() {
        put("strokes", values(&paint.strokes));
    }
    if !n.effects().is_empty() {
        put("effects", values(n.effects()));
    }
    if !n.exports().is_empty() {
        put("exports", values(n.exports()));
    }
    if let Some(d) = doc.resolved_display(id) {
        put("display", serde_json::to_value(&*d).unwrap_or_default());
    }
    if *n.item() != LayoutItem::default() {
        put("item", serde_json::to_value(n.item()).unwrap_or_default());
    }
    if *n.insets() != Insets::default() {
        put(
            "insets",
            serde_json::to_value(n.insets()).unwrap_or_default(),
        );
    }
    let children: Vec<Value> = n
        .children()
        .iter()
        .filter_map(|c| full_tree(doc, *c))
        .collect();
    if !children.is_empty() {
        put("children", Value::Array(children));
    }
    Some(Value::Object(out))
}

/// [`full_tree`] in the short forms `insert_subtree` reads back to the same
/// tree: hex colours where they are exact, paints without their default fields.
pub fn tree(doc: &Document, id: NodeId) -> Option<Value> {
    let mut v = full_tree(doc, id)?;
    loose::tighten("tree", &mut v);
    Some(v)
}

/// Fill in a tree's `like`s: a layer with `"like": "<id>"` takes that layer's
/// fills, strokes, effects, opacity and blend where it gives none, and a text
/// its text style field by field and its paragraph. Recurses into `children`.
fn expand_like(doc: &Document, v: &mut Value) -> Result<(), String> {
    let Value::Object(map) = v else {
        return Ok(());
    };
    if let Some(like) = map.remove("like") {
        let id = like
            .as_str()
            .and_then(NodeId::from_wire)
            .ok_or_else(|| format!("`like`: {like} is not a layer id"))?;
        let Some(Value::Object(src)) = full_tree(doc, id) else {
            return Err(format!("`like`: no layer `{}`", id.to_wire()));
        };
        for k in ["fills", "strokes", "effects", "opacity", "blend"] {
            if let Some(val) = src.get(k) {
                map.entry(k).or_insert_with(|| val.clone());
            }
        }
        let src_text = src.get("kind").and_then(|k| k.get("Text"));
        let text = map.get_mut("kind").and_then(|k| k.get_mut("Text"));
        if let (Some(Value::Object(from)), Some(Value::Object(to))) = (src_text, text) {
            if let Some(Value::Object(src_style)) = from.get("style") {
                let style = to
                    .entry("style")
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Value::Object(style) = style {
                    for (k, val) in src_style {
                        style.entry(k.clone()).or_insert_with(|| val.clone());
                    }
                }
            }
            if let Some(p) = from.get("paragraph") {
                to.entry("paragraph").or_insert_with(|| p.clone());
            }
        }
    }
    if let Some(Value::Array(children)) = map.get_mut("children") {
        for child in children {
            expand_like(doc, child)?;
        }
    }
    Ok(())
}

/// The blend mode a [`BLENDS`] label names; Normal for none.
fn blend_of(label: &str) -> BlendMode {
    BlendMode::ALL
        .into_iter()
        .find(|m| m.label() == label)
        .unwrap_or(BlendMode::Normal)
}

/// Why a script stopped: the step, counted from 1, and what went wrong there.
#[derive(thiserror::Error, Debug)]
#[error("step {step}: {why}")]
pub struct ScriptError {
    pub step: usize,
    pub why: String,
}

/// Run `script` against `doc`, each step committed before the next reads it: a list
/// of `{ "verb": name, "args": { … }, "as": label }`. A string argument `"@label"`
/// stands for the first thing an earlier step labelled `label` made, a node's wire
/// form or another thing's id ([`Done::made`]), and `"@page"` for the first page; a string
/// starting `@@` is written with one `@`. A step holding any other key is refused.
/// Answers what each step made, in those forms; on a refusal `doc` keeps the steps before it.
pub fn run_script(
    doc: &mut Document,
    ids: &mut IdSource,
    mut lent: Lent<'_>,
    script: &Value,
) -> Result<Vec<Vec<String>>, ScriptError> {
    let fail = |step: usize, why: String| ScriptError { step, why };
    let Value::Array(steps) = script else {
        return Err(fail(0, "a script is a list of steps".into()));
    };
    let mut labels: rustc_hash::FxHashMap<String, String> = rustc_hash::FxHashMap::default();
    labels.insert("page".into(), doc.first_page().to_wire());
    let mut out = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let n = i + 1;
        // A misspelled `args` would run the verb with none, at its widest.
        if let Some(extra) = step.as_object().and_then(|m| {
            m.keys()
                .find(|k| !["verb", "args", "as"].contains(&k.as_str()))
        }) {
            return Err(fail(n, format!("a step takes no `{extra}`")));
        }
        let verb = step
            .get("verb")
            .and_then(Value::as_str)
            .ok_or_else(|| fail(n, "a step names its `verb`".into()))?;
        let mut args = step.get("args").cloned().unwrap_or(Value::Null);
        substitute(&mut args, &labels).map_err(|why| fail(n, why))?;
        let res = Resolved::rebuild(doc);
        let done = call(
            &mut Ctx {
                doc,
                res: &res,
                ids,
                // A cast, not `as_deref_mut`: it shortens the object lifetime to
                // this step's borrow, which `&mut`'s invariance otherwise forbids.
                images: lent.images.as_mut().map(|s| &mut **s as &mut dyn ImageSink),
                contrast: lent
                    .contrast
                    .as_mut()
                    .map(|p| &mut **p as &mut dyn crate::contrast::Probe),
            },
            verb,
            &args,
        )
        .map_err(|e| fail(n, e.to_string()))?;
        // What the components owe the edit, as an import commits: instances follow
        // their mains and links settle.
        let mut tx = done.tx;
        crate::propagate::owed(doc, &mut tx, ids);
        doc.apply(&tx).map_err(|e| fail(n, e.to_string()))?;
        let made: Vec<String> = done
            .created
            .iter()
            .map(|id| id.to_wire())
            .chain(done.made)
            .collect();
        if let Some(label) = step.get("as").and_then(Value::as_str) {
            let first = made
                .first()
                .ok_or_else(|| fail(n, format!("`{verb}` makes nothing to call `{label}`")))?;
            labels.insert(label.to_owned(), first.clone());
        }
        out.push(made);
    }
    Ok(out)
}

/// Every `"@label"` string in `v` replaced by what its label stands for, and a
/// string starting `@@` by itself less one `@`.
pub fn substitute<S: std::hash::BuildHasher>(
    v: &mut Value,
    labels: &std::collections::HashMap<String, String, S>,
) -> Result<(), String> {
    match v {
        // `@@` escapes a literal `@`.
        Value::String(s) if s.starts_with("@@") => {
            s.remove(0);
        }
        Value::String(s) if s.starts_with('@') => {
            let made = labels
                .get(&s[1..])
                .ok_or_else(|| format!("no earlier step is called `{}`", &s[1..]))?;
            *s = made.clone();
        }
        Value::Array(list) => list.iter_mut().try_for_each(|v| substitute(v, labels))?,
        Value::Object(map) => map.values_mut().try_for_each(|v| substitute(v, labels))?,
        _ => {}
    }
    Ok(())
}
