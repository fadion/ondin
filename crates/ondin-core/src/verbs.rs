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

/// A verb's answer: the edit to commit, the nodes it creates, in order, and the
/// id of what else it made (a picture, a variable, a mode, a style) as a caller
/// names it.
#[derive(Debug, Default)]
pub struct Done {
    pub tx: Transaction,
    pub created: Vec<NodeId>,
    pub made: Option<String>,
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
            made: None,
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

impl<'a> Args<'a> {
    /// `value` checked against `verb`'s parameters: an object holding every required
    /// one, no other, each of its kind.
    fn check(verb: &'static Verb, value: &'a Value) -> Result<Self, VerbError> {
        let map = match value {
            Value::Object(map) => map,
            Value::Null if verb.params.iter().all(|p| !p.required) => {
                static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
                EMPTY.get_or_init(Map::new)
            }
            _ => return Err(VerbError::NotAnObject(verb.name)),
        };
        if let Some(extra) = map
            .keys()
            .find(|k| !verb.params.iter().any(|p| p.name == *k))
        {
            return Err(VerbError::Unexpected {
                verb: verb.name,
                param: extra.clone(),
            });
        }
        for p in verb.params {
            match map.get(p.name) {
                None | Some(Value::Null) if p.required => {
                    return Err(VerbError::Missing {
                        verb: verb.name,
                        param: p.name,
                    });
                }
                None | Some(Value::Null) => {}
                Some(v) => check_kind(p.kind, v).map_err(|why| VerbError::Invalid {
                    verb: verb.name,
                    param: p.name,
                    why,
                })?,
            }
        }
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

    /// The `Model` parameter `name`, deserialized.
    pub fn model<T: serde::de::DeserializeOwned>(
        &self,
        name: &'static str,
    ) -> Result<T, VerbError> {
        let v = self.get(name).ok_or(VerbError::Missing {
            verb: self.verb.name,
            param: name,
        })?;
        serde_json::from_value(v.clone()).map_err(|e| self.invalid(name, e))
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

/// Whether `v` reads as the model type `model` names (a [`ParamKind::Model`]'s
/// name), or why not.
pub fn check_model(model: &str, v: &Value) -> Result<(), String> {
    fn reads<T: serde::de::DeserializeOwned>(v: &Value) -> Result<(), String> {
        T::deserialize(v).map(|_| ()).map_err(|e| e.to_string())
    }
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
        "PropValue" => match v {
            Value::Bool(_) | Value::String(_) => Ok(()),
            _ => Err("expected true, false or a string".into()),
        },
        "Tree" => reads::<Tree>(v),
        "[Effect]" => reads::<Vec<Effect>>(v),
        "[ExportSpec]" => reads::<Vec<ExportSpec>>(v),
        "[Fill]" => reads::<Vec<Fill>>(v),
        "[Stroke]" => reads::<Vec<Stroke>>(v),
        _ => Err(format!("no model type is called `{model}`")),
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
    let text = NodeKind::Text {
        content: "Hello".into(),
        style: Box::default(),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: crate::node::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
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
        "PropValue" => Value::Bool(true),
        "[ParaAttr]" => json(&vec![
            ParaAttr::Spacing(Length::Px(12.0)),
            ParaAttr::IndentStart(Length::Px(24.0)),
        ]),
        "Tree" => serde_json::json!({
            "kind": json(&NodeKind::Artboard {
                size: Size::new(320.0, 200.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            }),
            "name": "Card",
            "fills": [json(&fill)],
            "display": json(&flex),
            "children": [
                { "kind": json(&rect), "name": "Picture", "fills": [json(&image_fill)] },
                {
                    "kind": json(&text),
                    "transform": json(&Affine::translate((16.0, 96.0))),
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
        _ => return None,
    })
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
const LEVELS: &[&str] = &["aa", "aaa", "apca"];
const PROP_KINDS: &[&str] = &["boolean", "text", "swap"];
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
        doc: "Create a whole tree of layers in `parent` in one edit; answers its layers, root first",
        params: &[
            req("parent", NODE, "the frame, group or page to create it in"),
            req(
                "tree",
                ParamKind::Model("Tree"),
                "the root layer, its properties and its `children`, bottom first",
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
            let tree = a.model::<Tree>("tree")?;
            let mut done = Done::default();
            tree_ops(ctx, a, &tree, parent, index, &mut done)?;
            Ok(done)
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
                "where its box is centred, across the parent; at the main's own place unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "where its box is centred, down the parent",
            ),
        ],
        run: |ctx, a| {
            let (main, parent) = (a.node("main")?, a.node("parent")?);
            let m = node(ctx, main)?;
            if !m.component() && m.set().is_none() {
                return Err(a.invalid("main", "not a main component or a set"));
            }
            node(ctx, parent)?;
            let centre = match (a.number("x"), a.number("y")) {
                (None, None) => None,
                (x, y) => {
                    let into = ctx.res.world_transform(parent).unwrap_or(Affine::IDENTITY);
                    Some(into * kurbo::Point::new(x.unwrap_or(0.0), y.unwrap_or(0.0)))
                }
            };
            let (tx, id) = build::place_instance(ctx.doc, ctx.ids, main, parent, centre)
                .ok_or(VerbError::Nothing("the set has no variant to place"))?;
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
            req(
                "mode",
                ParamKind::Id("mode"),
                "the mode, of the variable's collection",
            ),
            req(
                "value",
                ParamKind::Model("Cell"),
                "the value, or `{ \"alias\": <variable id> }`",
            ),
        ],
        run: |ctx, a| {
            let (id, mode): (VarId, ModeId) = (own_variable(ctx, a)?, a.id("mode")?);
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
            Ok(Done::edit(build::bind_field_ops(
                ctx.doc,
                &a.nodes("nodes")?,
                field,
                var,
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
                    a.nodes("nodes")?
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
            let nodes = a.nodes("nodes")?;
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
            let ops = nodes
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
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetGeometry {
                id: a.node("node")?,
                geometry: a.model::<GeometryPatch>("geometry")?,
            }]))
        },
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
        doc: "Clip a frame's contents to its box, or not",
        params: &[req("node", NODE, "the frame"), req("clip", BOOL, "clipped")],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetClip {
                id: a.node("node")?,
                clip: a.bool("clip").unwrap_or(true),
            }]))
        },
    },
    Verb {
        name: "set_mask",
        doc: "Use a layer as a mask for the layers above it in its parent, or stop",
        params: &[req("node", NODE, "the layer"), req("mask", BOOL, "a mask")],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetMask {
                id: a.node("node")?,
                mask: a.bool("mask").unwrap_or(true),
            }]))
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
        name: "set_text_style",
        doc: "Set a text layer's character style, and optionally its runs",
        params: &[
            req("node", NODE, "the text layer"),
            req("style", ParamKind::Model("TextStyle"), "the style"),
            opt("spans", ParamKind::Model("CharSpans"), "the runs over it"),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetTextStyle {
                id: a.node("node")?,
                style: a.model::<TextStyle>("style")?,
                spans: a.model_opt::<CharSpans>("spans")?,
            }]))
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
        doc: "Set a text layer's paragraph style",
        params: &[
            req("node", NODE, "the text layer"),
            req("paragraph", ParamKind::Model("ParagraphStyle"), "the style"),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetParagraphStyle {
                id: a.node("node")?,
                paragraph: a.model::<ParagraphStyle>("paragraph")?,
                spans: None,
            }]))
        },
    },
    Verb {
        name: "set_block_style",
        doc: "Set a text layer's block style: its box, overflow and vertical alignment",
        params: &[
            req("node", NODE, "the text layer"),
            req("block", ParamKind::Model("BlockStyle"), "the style"),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetBlockStyle {
                id: a.node("node")?,
                block: a.model::<BlockStyle>("block")?,
            }]))
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
        doc: "Replace a layer's fills, bottom first",
        params: &[
            req("node", NODE, "the layer"),
            req("fills", ParamKind::Model("[Fill]"), "the fills"),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            let fills = a.model::<Vec<Fill>>("fills")?;
            let own = node(ctx, id)?.paint().fills.clone();
            let fills = rekeyed(ctx, &own, fills);
            Ok(Done::ops(vec![Operation::SetFills { id, fills }]))
        },
    },
    Verb {
        name: "set_strokes",
        doc: "Replace a layer's strokes, bottom first",
        params: &[
            req("node", NODE, "the layer"),
            req("strokes", ParamKind::Model("[Stroke]"), "the strokes"),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            let strokes = a.model::<Vec<Stroke>>("strokes")?;
            let own = node(ctx, id)?.paint().strokes.clone();
            let strokes = rekeyed(ctx, &own, strokes);
            Ok(Done::ops(vec![Operation::SetStrokes { id, strokes }]))
        },
    },
    Verb {
        name: "set_effects",
        doc: "Replace a layer's effects: shadows and blurs, in order",
        params: &[
            req("node", NODE, "the layer"),
            req("effects", ParamKind::Model("[Effect]"), "the effects"),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            let effects = a.model::<Vec<Effect>>("effects")?;
            let own = node(ctx, id)?.effects().to_vec();
            let effects = rekeyed(ctx, &own, effects);
            Ok(Done::ops(vec![Operation::SetEffects { id, effects }]))
        },
    },
    Verb {
        name: "set_exports",
        doc: "Replace a layer's export settings",
        params: &[
            req("node", NODE, "the layer"),
            req("exports", ParamKind::Model("[ExportSpec]"), "the settings"),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let exports = a.model::<Vec<ExportSpec>>("exports")?;
            let own = node(ctx, id)?.exports().to_vec();
            let exports = rekeyed(ctx, &own, exports);
            Ok(Done::ops(vec![Operation::SetExports { id, exports }]))
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
            let probe = ctx
                .contrast
                .as_mut()
                .ok_or(VerbError::Nothing("this caller cannot measure contrast"))?;
            let findings = probe.measure(ctx.doc, &nodes, level);
            let mut ops = Vec::new();
            for f in findings.iter().filter(|f| !f.passes()) {
                let fix = match named {
                    Some(fix) => fix,
                    None => match f
                        .worst()
                        .and_then(|w| crate::contrast::nearest(f, level, w.ink))
                    {
                        Some(c) => Fix::Colour(c),
                        // No lightness at that hue passes: left as it is.
                        None => continue,
                    },
                };
                let new_fill = ctx.ids.mint_item();
                if let Some(tx) =
                    crate::contrast::fix_transaction(ctx.doc, f.id, f, level, fix, new_fill)
                {
                    ops.extend(tx.0);
                }
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
            let plan = fill::plan(ctx.doc, &nodes, &opts);
            Ok(Done::edit(
                plan.transaction(ctx.doc, ctx.images.as_deref_mut()),
            ))
        },
    },
    // --- layout -------------------------------------------------------------
    Verb {
        name: "set_display",
        doc: "Make a frame lay out its children as flex or grid, or stop",
        params: &[
            req("node", NODE, "the frame"),
            opt(
                "display",
                ParamKind::Model("Display"),
                "the layout; absent turns it off",
            ),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetDisplay {
                id: a.node("node")?,
                display: a.model_opt::<Display>("display")?,
            }]))
        },
    },
    Verb {
        name: "set_layout_item",
        doc: "Set how a layer sizes and places itself in its parent's layout",
        params: &[
            req("node", NODE, "the layer"),
            req(
                "item",
                ParamKind::Model("LayoutItem"),
                "its item properties",
            ),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetLayoutItem {
                id: a.node("node")?,
                item: a.model::<LayoutItem>("item")?,
            }]))
        },
    },
    Verb {
        name: "set_insets",
        doc: "Set a frame's padding",
        params: &[
            req("node", NODE, "the frame"),
            req("insets", ParamKind::Model("Insets"), "the four sides"),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetInsets {
                id: a.node("node")?,
                insets: a.model::<Insets>("insets")?,
            }]))
        },
    },
];

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
fn substitute(v: &mut Value, labels: &rustc_hash::FxHashMap<String, String>) -> Result<(), String> {
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
