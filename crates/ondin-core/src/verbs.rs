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
use crate::image::ImageId;
use crate::item::{Keyed, PastTheEnd, rekey_by_position};
use crate::node::{
    BlendMode, BlockStyle, BoolOp, CharSpans, Fill, NodeKind, ParagraphStyle, Pivot, Stroke,
    TextStyle,
};
use crate::op::{GeometryPatch, OpError, Operation, Transaction};
use crate::resolve::Resolved;
use crate::svg_in::ImageSink;
use crate::typography::{CharAttr, Length, ParaAttr};
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
}

/// A verb's answer: the edit to commit, the nodes it creates, in order, and the
/// picture it adds to the image table.
#[derive(Debug, Default)]
pub struct Done {
    pub tx: Transaction,
    pub created: Vec<NodeId>,
    pub image: Option<ImageId>,
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
            image: None,
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

/// One op per node of the `nodes` argument, built by `op`.
fn each(args: &Args<'_>, op: impl Fn(NodeId) -> Operation) -> Result<Done, VerbError> {
    Ok(Done::ops(
        args.nodes("nodes")?.into_iter().map(op).collect(),
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
            let name = a.text("name").map(str::to_owned);
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
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetTransform {
                id: a.node("node")?,
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
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetPivot {
                id: a.node("node")?,
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
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetName {
                id: a.node("node")?,
                name: a.text("name").unwrap_or_default().to_owned(),
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
        run: |_, a| {
            let visible = a.bool("visible").unwrap_or(true);
            each(a, |id| Operation::SetVisible { id, visible })
        },
    },
    Verb {
        name: "set_locked",
        doc: "Lock or unlock layers",
        params: &[
            req("nodes", NODES, "the layers"),
            req("locked", BOOL, "locked"),
        ],
        run: |_, a| {
            let locked = a.bool("locked").unwrap_or(false);
            each(a, |id| Operation::SetLocked { id, locked })
        },
    },
    Verb {
        name: "set_opacity",
        doc: "Set layers' opacity, 0 to 1",
        params: &[
            req("nodes", NODES, "the layers"),
            req("opacity", ParamKind::Number, "0 to 1"),
        ],
        run: |_, a| {
            let opacity = a.number("opacity").unwrap_or(1.0) as f32;
            if !build::valid_opacity(opacity) {
                return Err(a.invalid("opacity", "expected 0 to 1"));
            }
            each(a, |id| Operation::SetOpacity { id, opacity })
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
                &a.nodes("nodes")?,
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
            let id = a.node("node")?;
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
            Ok(Done {
                image: Some(id),
                ..Done::ops(ops)
            })
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
/// form or a picture's image id, and `"@page"` for the first page. Answers what each
/// step made, in those forms; on a refusal `doc` keeps the steps before it.
pub fn run_script(
    doc: &mut Document,
    ids: &mut IdSource,
    mut images: Option<&mut dyn ImageSink>,
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
                // A cast, not `as_deref_mut`: it shortens the sink's object lifetime
                // to this step's borrow, which `&mut`'s invariance otherwise forbids.
                images: images.as_mut().map(|s| &mut **s as &mut dyn ImageSink),
            },
            verb,
            &args,
        )
        .map_err(|e| fail(n, e.to_string()))?;
        doc.apply(&done.tx).map_err(|e| fail(n, e.to_string()))?;
        let made: Vec<String> = done
            .created
            .iter()
            .map(|id| id.to_wire())
            .chain(done.image.map(|id| id.0))
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

/// Every `"@label"` string in `v` replaced by what its label stands for.
fn substitute(v: &mut Value, labels: &rustc_hash::FxHashMap<String, String>) -> Result<(), String> {
    match v {
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
