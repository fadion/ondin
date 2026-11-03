//! A DTCG `tokens.json` read back into a document's variables and styles — the
//! inverse of [`crate::dtcg`].
//!
//! [`parse`] reads the file, refusing one that is not DTCG, or whose values
//! cannot be read, with the first reason and its path in the file. [`review`]
//! compares it with a document: **a token is matched by the id the export wrote
//! under [`EXTENSION`], then by its path**, so a token renamed in code is a
//! rename here and keeps every binding; failing both, it is added. Nothing is
//! applied until [`Review::ops`] turns the rows a user ticked into one
//! transaction, through the same operations the Variables panel uses.
//!
//! **Only what has a home is read**: colour, dimension, number, font family and
//! weight, string and boolean become variables; typography, shadow and gradient
//! become styles, as do the export's own `styles` group's fills and borders.
//! Anything else is left out and named ([`File::left_out`]).

use crate::dtcg::{EXTENSION, token_path};
use ondin_core::effect::{EFFECT_BLUR, EFFECT_SPREAD, EFFECT_X, EFFECT_Y, MAX_BLUR, Shadow};
use ondin_core::image::GradientBrush;
use ondin_core::node::{Fill, Stroke};
use ondin_core::peniko::{self, Color};
use ondin_core::style::{self, Style, StyleId, StyleSlot, StyleValue};
use ondin_core::typography::{Length, TextStyle};
use ondin_core::variable::{
    self, Cell, Collection, CollectionId, Mode, ModeId, VarId, VarKind, Variable,
};
use ondin_core::{
    Brush, BrushSource, Described, Document, Effect, EffectKind, IdSource, Keyed, NodeId,
    Operation, Transaction,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Why a file was refused: the first thing that could not be read, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    pub reason: String,
    /// The token or group it was found at, dotted; empty for the file itself.
    pub path: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.path.is_empty() {
            true => write!(f, "{}", self.reason),
            false => write!(f, "{}: {}", self.path, self.reason),
        }
    }
}

fn refuse<T>(path: &[String], reason: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal {
        reason: reason.into(),
        path: path.join("."),
    })
}

/// A token that has no variable or style here, and its type.
#[derive(Clone, Debug, PartialEq)]
pub struct LeftOut {
    pub path: String,
    pub ty: String,
}

/// A read token file.
#[derive(Clone, Debug, PartialEq)]
pub struct File {
    /// The collections the file holds: first the root's — every token outside a
    /// collection's group — then one per top-level group carrying Ondin's
    /// collection extension.
    collections: Vec<FileCollection>,
    tokens: Vec<Token>,
    left_out: Vec<LeftOut>,
}

/// One collection of a token file.
#[derive(Clone, Debug, PartialEq)]
struct FileCollection {
    /// Its name, matched against the document's; `None` for the root's, which
    /// lands in the collection the import is made into.
    name: Option<String>,
    /// The top-level group it is, `None` for the root's.
    key: Option<String>,
    /// The modes it names, in its order; empty for a file without Ondin's
    /// extension, whose `$value`s are one unnamed mode.
    modes: Vec<String>,
}

impl File {
    /// The tokens read as neither a variable nor a style.
    pub fn left_out(&self) -> &[LeftOut] {
        &self.left_out
    }
    /// Whether the file names its collections — written from a document with
    /// two or more — so it lands in them by name rather than in one.
    pub fn has_collections(&self) -> bool {
        self.collections.len() > 1
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Token {
    /// The token's path, a group's `$root` ending at the group.
    path: Vec<String>,
    /// Its path within its collection's group: the variable's name.
    name: Vec<String>,
    /// Which of [`File::collections`] it is in.
    collection: usize,
    id: Option<NodeId>,
    description: Option<String>,
    /// The scope Ondin's extension gives it, when it gives one.
    scope: Option<variable::Scope>,
    /// The scope its `$type` makes for a new variable: the length families
    /// for a `dimension`, weight for a `fontWeight`.
    type_scope: Option<variable::Scope>,
    body: Body,
}

#[derive(Clone, Debug, PartialEq)]
enum Body {
    /// A variable: its value per file mode (by name; `""` for a file without
    /// modes), `$value` already put in for the mode it holds.
    Var {
        kind: VarKind,
        values: Vec<(String, V)>,
    },
    /// A style of `slot`, named by `name`'s slash path.
    Style {
        slot: StyleSlot,
        name: Vec<String>,
        value: Composite,
    },
}

/// A value as the file wrote it: a literal, a `{reference}` to another token's
/// path, or (for the export's strings and booleans) another variable by name —
/// and by its collection's name when that is another.
#[derive(Clone, Debug, PartialEq)]
enum V {
    Lit(Cell),
    Ref(Vec<String>),
    Named(Option<String>, String),
}

#[derive(Clone, Debug, PartialEq)]
enum Composite {
    /// The whole value is another token's.
    Ref(Vec<String>),
    Color(V),
    Gradient(Vec<(V, f32)>),
    Border {
        color: V,
        width: V,
        dashed: bool,
    },
    Shadow(Vec<Layer>),
    Type(Box<Type>),
}

#[derive(Clone, Debug, PartialEq)]
struct Layer {
    color: V,
    x: V,
    y: V,
    blur: V,
    spread: V,
    inset: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Type {
    family: Option<V>,
    size: Option<V>,
    weight: Option<V>,
    letter: Option<V>,
    line: Option<V>,
}

// ---------------------------------------------------------------------------
// The file, read in its own order.

/// JSON kept in the order the file wrote it: `serde_json`'s map sorts its keys,
/// and the review lists groups as the file has them.
#[derive(Clone, Debug, PartialEq)]
enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl<'de> serde::Deserialize<'de> for J {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<J, D::Error> {
        struct Visit;
        impl<'de> serde::de::Visitor<'de> for Visit {
            type Value = J;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_bool<E>(self, v: bool) -> Result<J, E> {
                Ok(J::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<J, E> {
                Ok(J::Num(v as f64))
            }
            fn visit_u64<E>(self, v: u64) -> Result<J, E> {
                Ok(J::Num(v as f64))
            }
            fn visit_f64<E>(self, v: f64) -> Result<J, E> {
                Ok(J::Num(v))
            }
            fn visit_str<E>(self, v: &str) -> Result<J, E> {
                Ok(J::Str(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> Result<J, E> {
                Ok(J::Str(v))
            }
            fn visit_unit<E>(self) -> Result<J, E> {
                Ok(J::Null)
            }
            fn visit_none<E>(self) -> Result<J, E> {
                Ok(J::Null)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<J, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element()? {
                    out.push(v);
                }
                Ok(J::Arr(out))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> Result<J, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = a.next_entry::<String, J>()? {
                    out.push((k, v));
                }
                Ok(J::Obj(out))
            }
        }
        d.deserialize_any(Visit)
    }
}

impl J {
    fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            J::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn entries(&self) -> &[(String, J)] {
        match self {
            J::Obj(m) => m,
            _ => &[],
        }
    }
    /// The reference a `{a.b}` string names, a trailing `$root` dropped.
    fn reference(&self) -> Option<Vec<String>> {
        let s = self.str()?.trim();
        let inner = s.strip_prefix('{')?.strip_suffix('}')?;
        let mut path: Vec<String> = inner.split('.').map(|p| p.trim().to_string()).collect();
        if path.last().is_some_and(|p| p == "$root") {
            path.pop();
        }
        (!path.is_empty() && path.iter().all(|p| !p.is_empty())).then_some(path)
    }
}

/// A token as found, before its type is known for certain.
struct Raw<'a> {
    path: Vec<String>,
    ty: Option<String>,
    body: &'a J,
}

/// Read a `tokens.json`. Refused when it is not JSON, holds no token, or a
/// value cannot be read; a token of a type with no home here is kept in
/// [`File::left_out`] rather than refused.
pub fn parse(text: &str) -> Result<File, Refusal> {
    let root: J = serde_json::from_str(text).map_err(|e| Refusal {
        reason: format!("this is not JSON ({e})"),
        path: String::new(),
    })?;
    if !matches!(root, J::Obj(_)) {
        return refuse(&[], "a token file is a JSON object");
    }
    let ext = root.get("$extensions").and_then(|e| e.get(EXTENSION));

    // The collections: the root's, then each top-level group carrying the
    // collection extension. For each, its modes, the key each one's values are
    // filed under, and the mode its `$value`s hold.
    struct ModesRead {
        key: Option<String>,
        key_to_name: HashMap<String, String>,
        value_mode: String,
    }
    let read_modes = |at: &[String], e: Option<&J>| -> Result<(Vec<String>, ModesRead), Refusal> {
        let mut modes = Vec::new();
        let mut key_to_name = HashMap::new();
        if let Some(J::Arr(list)) = e.and_then(|e| e.get("modes")) {
            for m in list {
                let Some(name) = m.get("name").and_then(J::str) else {
                    return refuse(at, "a mode in the Ondin extension has no name");
                };
                let key = m.get("key").and_then(J::str).unwrap_or(name);
                key_to_name.insert(key.to_string(), name.to_string());
                modes.push(name.to_string());
            }
        }
        let value_mode = e
            .and_then(|e| e.get("valueMode"))
            .and_then(J::str)
            .map(str::to_string)
            .or_else(|| modes.first().cloned())
            .unwrap_or_default();
        Ok((
            modes,
            ModesRead {
                key: None,
                key_to_name,
                value_mode,
            },
        ))
    };
    let (modes, first) = read_modes(&[], ext)?;
    let mut collections = vec![FileCollection {
        name: None,
        key: None,
        modes,
    }];
    let mut reads = vec![first];
    let mut loose_lists: Vec<(usize, &J)> = Vec::new();
    if let Some(vars) = ext.and_then(|e| e.get("variables")) {
        loose_lists.push((0, vars));
    }
    for (k, g) in root.entries() {
        let Some(c) = g
            .get("$extensions")
            .and_then(|e| e.get(EXTENSION))
            .and_then(|e| e.get("collection"))
        else {
            continue;
        };
        let at = [k.clone()];
        let (modes, mut read) = read_modes(&at, Some(c))?;
        read.key = Some(k.clone());
        let name = c.get("name").and_then(J::str).unwrap_or(k).to_string();
        if let Some(vars) = g
            .get("$extensions")
            .and_then(|e| e.get(EXTENSION))
            .and_then(|e| e.get("variables"))
        {
            loose_lists.push((collections.len(), vars));
        }
        collections.push(FileCollection {
            name: Some(name),
            key: Some(k.clone()),
            modes,
        });
        reads.push(read);
    }
    // The collection a path is in, and its path within it.
    let place = |path: &[String]| -> (usize, Vec<String>) {
        match reads.iter().position(|r| {
            r.key
                .as_deref()
                .is_some_and(|k| path.first().is_some_and(|p| p == k))
        }) {
            Some(i) if path.len() > 1 => (i, path[1..].to_vec()),
            _ => (0, path.to_vec()),
        }
    };
    let scope_of = |e: Option<&J>| -> Option<variable::Scope> {
        let J::Arr(list) = e?.get("scope")? else {
            return None;
        };
        Some(variable::Scope::of(
            list.iter()
                .filter_map(J::str)
                .filter_map(variable::Field::from_wire),
        ))
    };
    // The export's styles group; a file without the extension has none.
    let styles_key = ext.map(|e| {
        e.get("styles")
            .and_then(J::str)
            .unwrap_or("styles")
            .to_string()
    });

    let mut raws = Vec::new();
    collect(&root, &mut Vec::new(), None, &mut raws)?;
    // A token without `$type` takes its reference's.
    let types: HashMap<Vec<String>, Option<String>> = raws
        .iter()
        .map(|r| (r.path.clone(), r.ty.clone()))
        .collect();
    let mut tokens = Vec::new();
    let mut left_out = Vec::new();
    for r in &raws {
        let ty = match &r.ty {
            Some(t) => t.clone(),
            None => {
                let mut at = r.body.get("$value").and_then(J::reference);
                let mut found = None;
                for _ in 0..16 {
                    let Some(p) = at.take() else { break };
                    match types.get(&p) {
                        Some(Some(t)) => {
                            found = Some(t.clone());
                            break;
                        }
                        _ => {
                            at = raws
                                .iter()
                                .find(|x| x.path == p)
                                .and_then(|x| x.body.get("$value"))
                                .and_then(J::reference);
                        }
                    }
                }
                match found {
                    Some(t) => t,
                    None => return refuse(&r.path, "the token has no $type"),
                }
            }
        };
        let id = r
            .body
            .get("$extensions")
            .and_then(|e| e.get(EXTENSION))
            .and_then(|e| e.get("id"))
            .and_then(J::str)
            .and_then(NodeId::from_wire);
        let description = r
            .body
            .get("$description")
            .and_then(J::str)
            .map(str::to_string);
        let value = r.body.get("$value").unwrap_or(&J::Null);
        let in_styles = styles_key
            .as_deref()
            .is_some_and(|k| r.path.first().is_some_and(|p| p == k));
        let body = if in_styles {
            let slot = match r.path.get(1).map(String::as_str) {
                Some("fill") => Some(StyleSlot::Fills),
                Some("stroke") => Some(StyleSlot::Strokes),
                Some("effect") => Some(StyleSlot::Effects),
                Some("type") => Some(StyleSlot::Text),
                _ => None,
            };
            let fits = matches!(
                (slot, ty.as_str()),
                (Some(StyleSlot::Fills), "color" | "gradient")
                    | (Some(StyleSlot::Strokes), "border")
                    | (Some(StyleSlot::Effects), "shadow")
                    | (Some(StyleSlot::Text), "typography")
            );
            match slot {
                Some(slot) if fits && r.path.len() > 2 => Some(Body::Style {
                    slot,
                    name: r.path[2..].to_vec(),
                    value: composite(&r.path, &ty, value)?,
                }),
                _ => None,
            }
        } else {
            match ty.as_str() {
                "typography" | "shadow" | "gradient" => Some(Body::Style {
                    slot: match ty.as_str() {
                        "typography" => StyleSlot::Text,
                        "shadow" => StyleSlot::Effects,
                        _ => StyleSlot::Fills,
                    },
                    name: r.path.clone(),
                    value: composite(&r.path, &ty, value)?,
                }),
                _ => match var_kind(&ty) {
                    Some(kind) => {
                        let ModesRead {
                            key_to_name,
                            value_mode,
                            ..
                        } = &reads[place(&r.path).0];
                        let mut values = Vec::new();
                        let ext_modes = r
                            .body
                            .get("$extensions")
                            .and_then(|e| e.get(EXTENSION))
                            .and_then(|e| e.get("modes"));
                        for (key, v) in ext_modes.map_or(&[][..], J::entries) {
                            let name = key_to_name.get(key).cloned().unwrap_or(key.clone());
                            values.push((name, read_v(&r.path, &ty, v)?));
                        }
                        if !matches!(value, J::Null) || values.is_empty() {
                            let v = read_v(&r.path, &ty, value)?;
                            values.retain(|(m, _)| m != value_mode);
                            values.push((value_mode.clone(), v));
                        }
                        // First, as what a mode the token has no value for takes.
                        if let Some(i) = values.iter().position(|(m, _)| m == value_mode) {
                            let held = values.remove(i);
                            values.insert(0, held);
                        }
                        Some(Body::Var { kind, values })
                    }
                    None => None,
                },
            }
        };
        match body {
            Some(body) => {
                let (collection, name) = place(&r.path);
                let scope = scope_of(r.body.get("$extensions").and_then(|e| e.get(EXTENSION)));
                let type_scope = match ty.as_str() {
                    "dimension" => Some(variable::Scope::of(
                        variable::Field::of_kind(VarKind::Number).filter(|f| f.is_length()),
                    )),
                    "fontWeight" => Some(variable::Scope::of([variable::Field::Weight])),
                    _ => None,
                };
                tokens.push(Token {
                    path: r.path.clone(),
                    name,
                    collection,
                    id,
                    description,
                    scope,
                    type_scope,
                    body,
                })
            }
            None => left_out.push(LeftOut {
                path: r.path.join("."),
                ty,
            }),
        }
    }

    // The export's strings and booleans, which have no DTCG type: the root's,
    // and each collection's.
    for (c, vars) in loose_lists {
        let ModesRead {
            key, key_to_name, ..
        } = &reads[c];
        for (name, v) in vars.entries() {
            let name: Vec<String> = name.split('/').map(str::to_string).collect();
            let path: Vec<String> = key.iter().cloned().chain(name.iter().cloned()).collect();
            let kind = match v.get("kind").and_then(J::str) {
                Some("string") => VarKind::String,
                Some("boolean") => VarKind::Boolean,
                _ => return refuse(&path, "a variable in the Ondin extension has no kind"),
            };
            let mut values = Vec::new();
            for (key, cell) in v.get("modes").map_or(&[][..], J::entries) {
                let mode = key_to_name.get(key).cloned().unwrap_or(key.clone());
                let read = match (cell, kind) {
                    (J::Str(s), VarKind::String) => V::Lit(Cell::String(s.clone())),
                    (J::Bool(b), VarKind::Boolean) => V::Lit(Cell::Boolean(*b)),
                    (c, _) => match c.get("alias").and_then(J::str) {
                        Some(to) => V::Named(
                            c.get("collection").and_then(J::str).map(str::to_string),
                            to.to_string(),
                        ),
                        None => return refuse(&path, "a value of the wrong kind"),
                    },
                };
                values.push((mode, read));
            }
            tokens.push(Token {
                id: v.get("id").and_then(J::str).and_then(NodeId::from_wire),
                description: v.get("description").and_then(J::str).map(str::to_string),
                scope: scope_of(Some(v)),
                type_scope: None,
                path,
                name,
                collection: c,
                body: Body::Var { kind, values },
            });
        }
    }
    if tokens.is_empty() && left_out.is_empty() {
        return refuse(&[], "the file holds no design token");
    }
    Ok(File {
        collections,
        tokens,
        left_out,
    })
}

/// Every token under `group`, depth first in the file's order, a group's `$type`
/// handed down to what it holds.
fn collect<'a>(
    group: &'a J,
    path: &mut Vec<String>,
    ty: Option<&str>,
    out: &mut Vec<Raw<'a>>,
) -> Result<(), Refusal> {
    let ty = group.get("$type").and_then(J::str).or(ty);
    for (k, v) in group.entries() {
        if k == "$root" {
            if !matches!(v, J::Obj(_)) || v.get("$value").is_none() {
                return refuse(path, "$root must be a token");
            }
            out.push(Raw {
                path: path.clone(),
                ty: v.get("$type").and_then(J::str).or(ty).map(str::to_string),
                body: v,
            });
            continue;
        }
        if k.starts_with('$') {
            continue;
        }
        path.push(k.clone());
        if !matches!(v, J::Obj(_)) {
            return refuse(path, "a group or token must be an object");
        }
        if v.get("$value").is_some() {
            out.push(Raw {
                path: path.clone(),
                ty: v.get("$type").and_then(J::str).or(ty).map(str::to_string),
                body: v,
            });
        } else {
            collect(v, path, ty, out)?;
        }
        path.pop();
    }
    Ok(())
}

/// The variable kind a token type becomes, `None` for a type with no variable.
fn var_kind(ty: &str) -> Option<VarKind> {
    match ty {
        "color" => Some(VarKind::Color),
        "dimension" | "number" | "fontWeight" => Some(VarKind::Number),
        "fontFamily" | "string" => Some(VarKind::String),
        "boolean" => Some(VarKind::Boolean),
        _ => None,
    }
}

/// One variable value of type `ty`, or a reference to another token.
fn read_v(path: &[String], ty: &str, j: &J) -> Result<V, Refusal> {
    if let Some(r) = j.reference() {
        return Ok(V::Ref(r));
    }
    let cell = match ty {
        "color" => Cell::Color(read_color(path, j)?),
        "dimension" => Cell::Number(read_dimension(path, j)?),
        "number" => match j.num() {
            Some(n) if n.is_finite() => Cell::Number(n),
            _ => return refuse(path, "a number token's value must be a number"),
        },
        "fontWeight" => Cell::Number(read_weight(path, j)?),
        "fontFamily" => Cell::String(read_family(path, j)?),
        "string" => match j.str() {
            Some(s) => Cell::String(s.to_string()),
            None => return refuse(path, "a string token's value must be a string"),
        },
        "boolean" => match j {
            J::Bool(b) => Cell::Boolean(*b),
            _ => return refuse(path, "a boolean token's value must be true or false"),
        },
        _ => return refuse(path, format!("a {ty} is not a variable")),
    };
    Ok(V::Lit(cell))
}

fn read_color(path: &[String], j: &J) -> Result<Color, Refusal> {
    let css = |s: &str| {
        peniko::color::parse_color(s.trim())
            .ok()
            .map(|c| c.to_alpha_color::<peniko::color::Srgb>())
    };
    match j {
        J::Str(s) => css(s).map_or_else(|| refuse(path, format!("{s:?} is not a colour")), Ok),
        J::Obj(_) => {
            let space = j.get("colorSpace").and_then(J::str).unwrap_or("srgb");
            let alpha = j.get("alpha").and_then(J::num).unwrap_or(1.0);
            let parts: Vec<String> = match j.get("components") {
                Some(J::Arr(c)) if c.len() == 3 => c
                    .iter()
                    .map(|x| match x {
                        J::Num(n) => format!("{n}"),
                        _ => "none".to_string(),
                    })
                    .collect(),
                _ => Vec::new(),
            };
            if space == "srgb"
                && let [Some(r), Some(g), Some(b)] = [0, 1, 2].map(|i| match j.get("components") {
                    Some(J::Arr(c)) => c.get(i).and_then(J::num),
                    _ => None,
                })
            {
                return Ok(Color::new([r as f32, g as f32, b as f32, alpha as f32]));
            }
            let written = match (space, parts.as_slice()) {
                (_, [a, b, c]) => match space {
                    "hsl" | "hwb" => Some(format!("{space}({a} {b}% {c}% / {alpha})")),
                    "lab" | "lch" | "oklab" | "oklch" => {
                        Some(format!("{space}({a} {b} {c} / {alpha})"))
                    }
                    _ => Some(format!("color({space} {a} {b} {c} / {alpha})")),
                },
                _ => None,
            };
            if let Some(c) = written.as_deref().and_then(css) {
                return Ok(c);
            }
            match j.get("hex").and_then(J::str).and_then(css) {
                Some(c) => Ok(c.with_alpha(alpha as f32)),
                None => refuse(path, format!("a colour in {space} cannot be read")),
            }
        }
        _ => refuse(path, "a colour must be an object or a string"),
    }
}

/// A dimension in px: `{value, unit}` with `px`, or `rem` at 16px.
fn read_dimension(path: &[String], j: &J) -> Result<f64, Refusal> {
    let (value, unit) = match j {
        J::Num(n) => (*n, "px".to_string()),
        J::Str(s) => {
            let s = s.trim();
            let at = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
            match s[..at].trim().parse::<f64>() {
                Ok(n) => (n, s[at..].to_string()),
                Err(_) => return refuse(path, format!("{s:?} is not a dimension")),
            }
        }
        J::Obj(_) => match (
            j.get("value").and_then(J::num),
            j.get("unit").and_then(J::str),
        ) {
            (Some(v), Some(u)) => (v, u.to_string()),
            _ => return refuse(path, "a dimension is a value and a unit"),
        },
        _ => return refuse(path, "a dimension is a value and a unit"),
    };
    let px = match unit.as_str() {
        "px" | "" => value,
        "rem" => value * 16.0,
        u => {
            return refuse(
                path,
                format!("a dimension in {u} cannot be read; px or rem"),
            );
        }
    };
    if !px.is_finite() {
        return refuse(path, "the dimension is not a finite number");
    }
    Ok(px)
}

fn read_weight(path: &[String], j: &J) -> Result<f64, Refusal> {
    if let Some(n) = j.num() {
        return Ok(n);
    }
    let w = match j.str().map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("thin" | "hairline") => 100.0,
        Some("extra-light" | "ultra-light") => 200.0,
        Some("light") => 300.0,
        Some("normal" | "regular" | "book") => 400.0,
        Some("medium") => 500.0,
        Some("semi-bold" | "demi-bold") => 600.0,
        Some("bold") => 700.0,
        Some("extra-bold" | "ultra-bold") => 800.0,
        Some("black" | "heavy") => 900.0,
        Some("extra-black" | "ultra-black") => 950.0,
        _ => return refuse(path, "a font weight is a number or a weight's name"),
    };
    Ok(w)
}

fn read_family(path: &[String], j: &J) -> Result<String, Refusal> {
    match j {
        J::Str(s) => Ok(s.clone()),
        J::Arr(list) => match list.first().and_then(J::str) {
            Some(s) => Ok(s.to_string()),
            None => refuse(path, "a font family list is empty"),
        },
        _ => refuse(path, "a font family is a name or a list of names"),
    }
}

/// A style token's value of type `ty`.
fn composite(path: &[String], ty: &str, j: &J) -> Result<Composite, Refusal> {
    if let Some(r) = j.reference() {
        return Ok(Composite::Ref(r));
    }
    let field = |k: &str| j.get(k).unwrap_or(&J::Null);
    let part = |k: &str, t: &str| -> Result<V, Refusal> {
        let mut p = path.to_vec();
        p.push(k.to_string());
        read_v(&p, t, field(k))
    };
    match ty {
        "color" => Ok(Composite::Color(read_v(path, "color", j)?)),
        "gradient" => {
            let J::Arr(stops) = j else {
                return refuse(path, "a gradient is a list of stops");
            };
            let mut out = Vec::new();
            for s in stops {
                let color = match s.get("color") {
                    Some(c) => read_v(path, "color", c)?,
                    None => return refuse(path, "a gradient stop has no colour"),
                };
                let at = s.get("position").and_then(J::num).unwrap_or(0.0);
                out.push((color, at.clamp(0.0, 1.0) as f32));
            }
            Ok(Composite::Gradient(out))
        }
        "border" => {
            let dashed = match field("style") {
                J::Str(s) => s != "solid",
                J::Obj(_) => true,
                _ => false,
            };
            Ok(Composite::Border {
                color: part("color", "color")?,
                width: part("width", "dimension")?,
                dashed,
            })
        }
        "shadow" => {
            let one = |l: &J| -> Result<Layer, Refusal> {
                let p = |k: &str, t: &str| {
                    let mut at = path.to_vec();
                    at.push(k.to_string());
                    read_v(&at, t, l.get(k).unwrap_or(&J::Num(0.0)))
                };
                Ok(Layer {
                    color: p("color", "color")?,
                    x: p("offsetX", "dimension")?,
                    y: p("offsetY", "dimension")?,
                    blur: p("blur", "dimension")?,
                    spread: p("spread", "dimension")?,
                    inset: matches!(l.get("inset"), Some(J::Bool(true))),
                })
            };
            match j {
                J::Arr(list) => Ok(Composite::Shadow(
                    list.iter().map(one).collect::<Result<_, _>>()?,
                )),
                J::Obj(_) => Ok(Composite::Shadow(vec![one(j)?])),
                _ => refuse(path, "a shadow is an object or a list of them"),
            }
        }
        "typography" => {
            let opt = |k: &str, t: &str| -> Result<Option<V>, Refusal> {
                match j.get(k) {
                    None | Some(J::Null) => Ok(None),
                    Some(_) => part(k, t).map(Some),
                }
            };
            if !matches!(j, J::Obj(_)) {
                return refuse(path, "a typography token's value is an object");
            }
            Ok(Composite::Type(Box::new(Type {
                family: opt("fontFamily", "fontFamily")?,
                size: opt("fontSize", "dimension")?,
                weight: opt("fontWeight", "fontWeight")?,
                letter: opt("letterSpacing", "dimension")?,
                line: opt("lineHeight", "number")?,
            })))
        }
        _ => refuse(path, format!("a {ty} is not a style")),
    }
}

// ---------------------------------------------------------------------------
// The review.

/// What a row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Changed,
    Renamed,
    Removed,
    /// A kit's variable or style: listed, never applied — a kit changes through
    /// its own update.
    Kit,
}

/// A value as a row shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub text: String,
    pub swatch: Option<Color>,
}

/// One mode column of a row: what it holds now when that changes, and what it
/// will hold.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Column {
    pub old: Option<Shown>,
    pub new: Option<Shown>,
}

/// One row of the review.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub change: Change,
    /// The header it sits under: the token's group path, or *Modes*.
    pub group: String,
    pub name: String,
    /// A second line: a rename's old path, what reads a removal.
    pub sub: Option<String>,
    /// One per [`Review::columns`]; a style's value is in the first.
    pub cells: Vec<Column>,
    /// Whether the row starts ticked: all but a removal and a kit's.
    pub ticked: bool,
    item: Item,
}

#[derive(Clone, Debug, PartialEq)]
enum Item {
    Mode(usize),
    Var { plan: usize, token: usize },
    Remove { var: VarId, path: Vec<String> },
    Style { plan: usize, token: usize },
    Kit { token: usize },
}

/// A token file compared with a document.
#[derive(Clone, Debug)]
pub struct Review {
    /// The mode columns of each collection the file lands in: the document's,
    /// then those the file adds, each named *Collection · Mode* when there are
    /// two collections; one *Value* column when there is no mode.
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    /// Tokens with no variable or style here, and why.
    pub left_out: Vec<LeftOut>,
    /// The document's modes the file lacks, which keep their values.
    pub kept_modes: Vec<String>,
    /// The modes the file adds, each to its collection.
    added_modes: Vec<(CollectionId, Mode)>,
    /// The collections the file adds — one of the file's no collection here is
    /// named as — made when one of their modes is ticked.
    added_collections: Vec<Collection>,
    /// Every variable the file reaches, changed or not: an unchanged one still
    /// takes its values for an added mode.
    vars: Vec<Wanted>,
    styles: Vec<WantedStyle>,
    /// The new variables' ids, and their paths, for the refusal that names one
    /// left unticked.
    fresh: BTreeMap<VarId, String>,
}

#[derive(Clone, Debug)]
struct Wanted {
    var: Variable,
    new: bool,
}

#[derive(Clone, Debug)]
struct WantedStyle {
    style: Style,
    new: bool,
}

/// A token's place among the file's variables: the variable it is (matched or
/// to be added) and its kind.
#[derive(Clone, Copy)]
struct Target {
    id: VarId,
    kind: VarKind,
}

/// Compare `file` with `doc`: what applying it would add, change, rename and
/// remove. New variables, styles, modes and collections are minted from `ids`
/// here, so an alias between two new tokens names a real id.
///
/// **A file's collection lands in the document's of its name**, else in a new
/// one; a file that names none — written from a document with one collection,
/// or by another tool — lands in `into`, the collection on show. A removal is
/// offered only among the collections the file lands in.
pub fn review(
    doc: &Document,
    file: &File,
    into: CollectionId,
    ids: &mut IdSource,
) -> Result<Review, Refusal> {
    let t = doc.variables();
    let into = t.collection(into).map_or(t.first_collection().id, |c| c.id);
    let has_vars = |k: usize| {
        file.tokens
            .iter()
            .any(|tk| tk.collection == k && matches!(tk.body, Body::Var { .. }))
    };
    // The collection each of the file's lands in, minted for a new one. The
    // root's is `into` — unless it holds no variable while the file names its
    // collections.
    let mut added_collections: Vec<Collection> = Vec::new();
    let landing: Vec<CollectionId> = file
        .collections
        .iter()
        .map(|fc| match &fc.name {
            None => into,
            Some(name) => match t.collections().iter().find(|c| c.name == *name) {
                Some(c) => c.id,
                None => {
                    let id = ids.mint_collection();
                    added_collections.push(Collection {
                        id,
                        name: name.clone(),
                        modes: Vec::new(),
                    });
                    id
                }
            },
        })
        .collect();
    let touched: Vec<CollectionId> = {
        let mut out = Vec::new();
        for (k, c) in landing.iter().enumerate() {
            if (has_vars(k) || !file.collections[k].modes.is_empty()) && !out.contains(c) {
                out.push(*c);
            }
        }
        out
    };
    let held_modes = |c: CollectionId| -> Vec<Mode> {
        t.collection(c).map(|c| c.modes.clone()).unwrap_or_default()
    };
    let mut added_modes: Vec<(CollectionId, Mode)> = Vec::new();
    let mut mode_of: HashMap<(usize, String), ModeId> = HashMap::new();
    for (k, fc) in file.collections.iter().enumerate() {
        let c = landing[k];
        let add_mode =
            |name: &str, added_modes: &mut Vec<(CollectionId, Mode)>, ids: &mut IdSource| {
                let id = ids.mint_mode();
                added_modes.push((
                    c,
                    Mode {
                        id,
                        name: name.to_string(),
                    },
                ));
                id
            };
        for name in &fc.modes {
            let known = held_modes(c)
                .into_iter()
                .chain(
                    added_modes
                        .iter()
                        .filter(|(x, _)| *x == c)
                        .map(|(_, m)| m.clone()),
                )
                .find(|m| m.name == *name);
            let id = match known {
                Some(m) => m.id,
                None => add_mode(name, &mut added_modes, ids),
            };
            mode_of.insert((k, name.clone()), id);
        }
        if fc.modes.is_empty() {
            let default = held_modes(c)
                .first()
                .map(|m| m.id)
                .or_else(|| added_modes.iter().find(|(x, _)| *x == c).map(|(_, m)| m.id));
            let id = match default {
                Some(m) => m,
                None if has_vars(k) => add_mode(variable::FIRST_MODE_NAME, &mut added_modes, ids),
                None => ModeId(NodeId::NOWHERE),
            };
            mode_of.insert((k, String::new()), id);
        }
    }
    // The columns: each collection landed in, its modes then the added ones.
    let labelled = touched.len() > 1 || t.has_collections();
    let mut columns: Vec<(String, ModeId)> = Vec::new();
    for c in &touched {
        let cname = t
            .collection(*c)
            .map(|x| x.name.clone())
            .or_else(|| {
                added_collections
                    .iter()
                    .find(|x| x.id == *c)
                    .map(|x| x.name.clone())
            })
            .unwrap_or_default();
        let modes = held_modes(*c).into_iter().chain(
            added_modes
                .iter()
                .filter(|(x, _)| x == c)
                .map(|(_, m)| m.clone()),
        );
        for m in modes {
            let label = match labelled {
                true => format!("{cname} \u{b7} {}", m.name),
                false => m.name.clone(),
            };
            columns.push((label, m.id));
        }
    }
    let column_of = |m: ModeId| -> Option<CollectionId> {
        t.collection_of_mode(m)
            .map(|c| c.id)
            .or_else(|| added_modes.iter().find(|(_, x)| x.id == m).map(|(c, _)| *c))
    };
    let mut kept_modes: Vec<String> = Vec::new();
    for (k, fc) in file.collections.iter().enumerate() {
        if fc.modes.is_empty() {
            continue;
        }
        for m in held_modes(landing[k]) {
            if !fc.modes.contains(&m.name) && !kept_modes.contains(&m.name) {
                kept_modes.push(m.name);
            }
        }
    }

    // Each variable token's variable: by id, then by path in the collection it
    // lands in, else a new one.
    let mut by_path: HashMap<(CollectionId, Vec<String>), VarId> = HashMap::new();
    let mut kit_by_path: HashMap<Vec<String>, VarId> = HashMap::new();
    for v in t.iter() {
        match (v.origin.as_ref(), t.collection_of(v.id)) {
            (None, Some(c)) => {
                by_path.entry((c.id, token_path(&v.name))).or_insert(v.id);
            }
            _ => {
                kit_by_path.entry(token_path(&v.name)).or_insert(v.id);
            }
        }
    }
    let mut taken: BTreeSet<VarId> = BTreeSet::new();
    let mut target: Vec<Option<Target>> = vec![None; file.tokens.len()];
    let mut fresh = BTreeMap::new();
    let mut left_out = file.left_out.clone();
    for (i, tk) in file.tokens.iter().enumerate() {
        let Body::Var { kind, .. } = tk.body else {
            continue;
        };
        let by_id = tk
            .id
            .map(VarId)
            .filter(|id| !taken.contains(id) && t.get(*id).is_some());
        let found = by_id
            .or_else(|| {
                by_path
                    .get(&(landing[tk.collection], tk.name.clone()))
                    .copied()
                    .filter(|id| !taken.contains(id))
            })
            .or_else(|| kit_by_path.get(&tk.name).copied());
        let id = match found.and_then(|id| t.get(id)) {
            Some(v) if v.kind != kind => {
                // Still the file's, so not offered for removal as well.
                taken.insert(v.id);
                left_out.push(LeftOut {
                    path: tk.path.join("."),
                    ty: format!("a {} here", kind_word(v.kind)),
                });
                continue;
            }
            Some(v) => v.id,
            None => {
                let id = ids.mint_var();
                fresh.insert(id, tk.path.join("."));
                id
            }
        };
        taken.insert(id);
        target[i] = Some(Target { id, kind });
    }
    let at_path: HashMap<&[String], Target> = file
        .tokens
        .iter()
        .zip(&target)
        .filter_map(|(tk, t)| Some((tk.path.as_slice(), (*t)?)))
        .collect();
    // A reference into the document rather than the file keeps that variable.
    let mut referenced: BTreeSet<VarId> = BTreeSet::new();
    // A reference to the document: under a collection's group, its variable of
    // that path; else `into`'s, else any collection's, else a kit's.
    let in_doc = |to: &[String]| -> Option<&VarId> {
        let grouped = file.collections.iter().enumerate().find(|(_, fc)| {
            fc.key
                .as_deref()
                .is_some_and(|k| to.first().is_some_and(|p| p == k))
        });
        match grouped {
            Some((k, _)) if to.len() > 1 => by_path.get(&(landing[k], to[1..].to_vec())),
            _ => by_path
                .get(&(into, to.to_vec()))
                .or_else(|| {
                    t.collections()
                        .iter()
                        .find_map(|c| by_path.get(&(c.id, to.to_vec())))
                })
                .or_else(|| kit_by_path.get(to)),
        }
    };
    let resolve = |path: &[String],
                   to: &[String],
                   kind: VarKind,
                   referenced: &mut BTreeSet<VarId>|
     -> Result<VarId, Refusal> {
        let (id, k) = match at_path.get(to) {
            Some(x) => (x.id, x.kind),
            None => match in_doc(to) {
                Some(id) => {
                    referenced.insert(*id);
                    (*id, t.get(*id).map_or(kind, |v| v.kind))
                }
                None => {
                    return refuse(
                        path,
                        format!("refers to {{{}}}, which is not in the file", to.join(".")),
                    );
                }
            },
        };
        if k != kind {
            return refuse(
                path,
                format!("refers to {{{}}}, a {}", to.join("."), kind_word(k)),
            );
        }
        Ok(id)
    };
    // A string's or a boolean's alias by name: in the referrer's collection,
    // or the one it names.
    let named = |path: &[String],
                 from: usize,
                 collection: Option<&str>,
                 name: &str,
                 kind: VarKind|
     -> Result<VarId, Refusal> {
        let k = match collection {
            Some(c) => file
                .collections
                .iter()
                .position(|fc| fc.name.as_deref() == Some(c)),
            None => Some(from),
        };
        let in_file = file
            .tokens
            .iter()
            .zip(&target)
            .find(|(tk, _)| Some(tk.collection) == k && tk.name.join("/") == name)
            .and_then(|(_, t)| *t);
        let here = match collection {
            Some(c) => t
                .collections()
                .iter()
                .find(|x| x.name == c)
                .and_then(|x| t.by_name_in(x.id, name)),
            None => t.by_name_in(landing[from], name),
        };
        match in_file
            .map(|t| t.id)
            .or_else(|| here.or_else(|| t.by_name(name)).map(|v| v.id))
        {
            Some(id) if t.get(id).is_none_or(|v| v.kind == kind) => Ok(id),
            _ => refuse(path, format!("refers to {name}, which is not in the file")),
        }
    };

    // The variables as the file wants them.
    let mut vars: Vec<Wanted> = Vec::new();
    let mut plan_of: Vec<Option<usize>> = vec![None; file.tokens.len()];
    for (i, tk) in file.tokens.iter().enumerate() {
        let (Some(tg), Body::Var { kind, values }) = (target[i], &tk.body) else {
            continue;
        };
        let held = t.get(tg.id);
        if held.is_some_and(|v| v.origin.is_some()) {
            continue;
        }
        let cell_of = |v: &V, referenced: &mut BTreeSet<VarId>| -> Result<Cell, Refusal> {
            Ok(match v {
                V::Lit(c) => c.clone(),
                V::Ref(to) => Cell::Alias(resolve(&tk.path, to, *kind, referenced)?),
                V::Named(c, name) => {
                    Cell::Alias(named(&tk.path, tk.collection, c.as_deref(), name, *kind)?)
                }
            })
        };
        let fallback = values.first().map(|(_, v)| v);
        // Its cells in the collection it lands in: a variable matched in
        // another moves there, keeping none of the cells it had.
        let lands = landing[tk.collection];
        let stays = held.is_some_and(|v| t.collection_of(v.id).map(|c| c.id) == Some(lands));
        let mut cells = match stays {
            true => held.map(|v| v.cells.clone()).unwrap_or_default(),
            false => BTreeMap::new(),
        };
        let held_here = held.filter(|_| stays);
        for (_, m) in columns.iter().filter(|(_, m)| column_of(*m) == Some(lands)) {
            let given = values
                .iter()
                .find(|(name, _)| mode_of.get(&(tk.collection, name.clone())) == Some(m))
                .map(|(_, v)| v);
            let added = added_modes.iter().any(|(_, a)| a.id == *m);
            let v = match (given, held_here) {
                (Some(v), _) => v,
                (None, Some(_)) if !added => continue,
                (None, _) => match fallback {
                    Some(v) => v,
                    None => continue,
                },
            };
            let cell = cell_of(v, &mut referenced)?;
            // The export rounds a colour's components; a value it would write
            // the same is the value held.
            let same = cells.get(m).is_some_and(|c| same_cell(c, &cell));
            if !same {
                cells.insert(*m, cell);
            }
        }
        if cells.values().any(|c| *c == Cell::Alias(tg.id)) {
            return refuse(&tk.path, "the token refers to itself");
        }
        let name = match held {
            Some(v) if token_path(&v.name) == tk.name => v.name.clone(),
            _ => tk.name.join("/"),
        };
        let description = match (&tk.description, held) {
            (Some(d), _) => d.clone(),
            (None, Some(v)) => v.description.clone(),
            (None, None) => String::new(),
        };
        // The extension's scope, else the one held, else its type's for a new
        // number; a boolean has none.
        let scope = match held {
            Some(v) => tk.scope.or(v.scope),
            None => tk.scope.or(tk.type_scope),
        }
        .filter(|_| *kind != VarKind::Boolean);
        plan_of[i] = Some(vars.len());
        vars.push(Wanted {
            var: Variable {
                id: tg.id,
                name,
                kind: *kind,
                cells,
                scope,
                origin: None,
                forked: held.and_then(|v| v.forked.clone()),
                description,
            },
            new: held.is_none(),
        });
    }

    // References that loop, followed through what the file wants.
    let wanted: HashMap<VarId, &Variable> = vars.iter().map(|w| (w.var.id, &w.var)).collect();
    for w in &vars {
        for (m, c) in &w.var.cells {
            let mut at = c.clone();
            let mut steps = 0;
            while let Cell::Alias(to) = at {
                steps += 1;
                if steps > wanted.len() + t.iter().count() + 1 {
                    return refuse(&token_path(&w.var.name), "its references loop");
                }
                let next = wanted
                    .get(&to)
                    .map(|v| v.cells.get(m).cloned())
                    .unwrap_or_else(|| t.get(to).and_then(|v| v.cells.get(m).cloned()));
                match next {
                    Some(n) => at = n,
                    None => break,
                }
            }
        }
    }

    // The styles as the file wants them.
    let mut style_by_path: HashMap<(StyleSlot, Vec<String>), StyleId> = HashMap::new();
    let mut kit_style: HashMap<(StyleSlot, Vec<String>), StyleId> = HashMap::new();
    for s in doc.styles().iter() {
        let at = match s.origin {
            None => &mut style_by_path,
            Some(_) => &mut kit_style,
        };
        at.entry((s.slot(), token_path(&s.name))).or_insert(s.id);
    }
    let value_mode = file.collections.first().and_then(|fc| {
        mode_of
            .get(&(0, fc.modes.first().cloned().unwrap_or_default()))
            .copied()
    });
    let ctx = Ctx {
        doc,
        file,
        at_path: &at_path,
        wanted: &wanted,
        mode: value_mode,
    };
    let mut styles: Vec<WantedStyle> = Vec::new();
    let mut style_plan: Vec<Option<usize>> = vec![None; file.tokens.len()];
    let mut kit_tokens: BTreeSet<usize> = BTreeSet::new();
    let mut style_taken: BTreeSet<StyleId> = BTreeSet::new();
    for (i, tk) in file.tokens.iter().enumerate() {
        let Body::Style { slot, name, value } = &tk.body else {
            continue;
        };
        let key = (*slot, name.clone());
        let by_id = tk
            .id
            .map(StyleId)
            .filter(|id| !style_taken.contains(id))
            .and_then(|id| doc.styles().get(id))
            .filter(|s| s.slot() == *slot);
        let held = by_id.or_else(|| {
            style_by_path
                .get(&key)
                .filter(|id| !style_taken.contains(id))
                .and_then(|id| doc.styles().get(*id))
        });
        if held.is_some_and(|s| s.origin.is_some())
            || (held.is_none() && kit_style.contains_key(&key))
        {
            kit_tokens.insert(i);
            continue;
        }
        let value = ctx.style_value(&tk.path, *slot, value, held.map(|s| &s.value), ids, 0)?;
        let Some(value) = value else {
            left_out.push(LeftOut {
                path: tk.path.join("."),
                ty: "a style of another kind here".to_string(),
            });
            continue;
        };
        let style = Style {
            id: held.map_or_else(|| ids.mint_style(), |s| s.id),
            name: match held {
                Some(s) if token_path(&s.name) == *name => s.name.clone(),
                _ => name.join("/"),
            },
            value,
            origin: None,
            forked: held.and_then(|s| s.forked.clone()),
            description: match (&tk.description, held) {
                (Some(d), _) => d.clone(),
                (None, Some(s)) => s.description.clone(),
                (None, None) => String::new(),
            },
        };
        style_taken.insert(style.id);
        style_plan[i] = Some(styles.len());
        styles.push(WantedStyle {
            style,
            new: held.is_none(),
        });
    }

    // The rows.
    let shown = Shower {
        doc,
        wanted: &wanted,
    };
    let mut rows: Vec<Row> = Vec::new();
    // A document without modes still shows a style's value in one column.
    let blank = || vec![Column::default(); columns.len().max(1)];
    let collection_name = |c: CollectionId| {
        t.collection(c)
            .or_else(|| added_collections.iter().find(|x| x.id == c))
            .map(|x| x.name.clone())
            .unwrap_or_default()
    };
    for (i, (c, m)) in added_modes.iter().enumerate() {
        let name = match labelled {
            true => format!(
                "Adds mode \u{201c}{}\u{201d} to {}",
                m.name,
                collection_name(*c)
            ),
            false => format!("Adds mode \u{201c}{}\u{201d}", m.name),
        };
        rows.push(Row {
            change: Change::Added,
            group: "Modes".to_string(),
            name,
            sub: Some("every token\u{2019}s value for it comes from the file".to_string()),
            cells: blank(),
            ticked: true,
            item: Item::Mode(i),
        });
    }
    for (i, tk) in file.tokens.iter().enumerate() {
        let group = group_of(&tk.path);
        let name = tk.path.join(".");
        if let Some(p) = plan_of[i] {
            let w = &vars[p];
            let held = t.get(w.var.id);
            let cells: Vec<Column> = columns
                .iter()
                .map(|(_, m)| {
                    let new = w.var.cells.get(m).map(|c| shown.cell(c, *m));
                    let old = held
                        .and_then(|h| h.cells.get(m))
                        .filter(|c| w.var.cells.get(m) != Some(*c))
                        .map(|c| shown.cell(c, *m));
                    Column { old, new }
                })
                .collect();
            let (change, sub) = match held {
                None => (Change::Added, None),
                Some(h) if h.name != w.var.name => (
                    Change::Renamed,
                    Some(format!(
                        "was {} \u{b7} {} kept",
                        token_path(&h.name).join("."),
                        count(readers(doc, h.id), "binding")
                    )),
                ),
                // A cell for an added mode alone is the mode's row: `AddMode`
                // writes it whether this one is ticked or not.
                Some(h)
                    if h.cells.iter().any(|(m, c)| w.var.cells.get(m) != Some(c))
                        || h.description != w.var.description
                        || h.scope != w.var.scope =>
                {
                    let same = h.cells.iter().all(|(m, c)| w.var.cells.get(m) == Some(c));
                    let sub = match (same, h.scope != w.var.scope) {
                        (false, _) => None,
                        (true, true) => Some("scope changed".to_string()),
                        (true, false) => Some("description changed".to_string()),
                    };
                    (Change::Changed, sub)
                }
                Some(_) => continue,
            };
            rows.push(Row {
                change,
                group,
                name,
                sub,
                cells,
                ticked: true,
                item: Item::Var { plan: p, token: i },
            });
        } else if let Some(p) = style_plan[i] {
            let w = &styles[p];
            let held = doc.styles().get(w.style.id);
            let mut cells = blank();
            if let Some(c) = cells.first_mut() {
                c.new = Some(shown.style(&w.style.value));
                c.old = held
                    .filter(|h| h.value != w.style.value)
                    .map(|h| shown.style(&h.value));
            }
            let (change, sub) = match held {
                None => (Change::Added, None),
                Some(h) if h.name != w.style.name => (
                    Change::Renamed,
                    Some(format!(
                        "was {} \u{b7} {} kept",
                        token_path(&h.name).join("."),
                        count(style::users(doc, h.id).len(), "layer")
                    )),
                ),
                Some(h) if h.value != w.style.value || h.description != w.style.description => {
                    (Change::Changed, None)
                }
                Some(_) => continue,
            };
            rows.push(Row {
                change,
                group,
                name,
                sub,
                cells,
                ticked: true,
                item: Item::Style { plan: p, token: i },
            });
        } else if kit_tokens.contains(&i)
            || target[i].is_some_and(|tg| t.get(tg.id).is_some_and(|v| v.origin.is_some()))
        {
            rows.push(Row {
                change: Change::Kit,
                group,
                name,
                sub: Some(
                    "Kit\u{2019}s \u{b7} changes through the kit\u{2019}s update".to_string(),
                ),
                cells: blank(),
                ticked: false,
                item: Item::Kit { token: i },
            });
        }
    }
    for v in t.local() {
        if taken.contains(&v.id)
            || referenced.contains(&v.id)
            || t.collection_of(v.id)
                .is_none_or(|c| !touched.contains(&c.id))
        {
            continue;
        }
        let path = token_path(&v.name);
        let layers = variable::users(doc, v.id).len();
        let styled = doc
            .styles()
            .iter()
            .filter(|s| style_reads(&s.value, v.id))
            .count();
        let sub = match (layers, styled) {
            (0, 0) => "read by nothing".to_string(),
            (l, 0) => format!("read by {}", count(l, "layer")),
            (0, s) => format!("read by {}", count(s, "style")),
            (l, s) => format!("read by {} and {}", count(l, "layer"), count(s, "style")),
        };
        rows.push(Row {
            change: Change::Removed,
            group: group_of(&path),
            name: path.join("."),
            sub: Some(sub),
            cells: columns
                .iter()
                .map(|(_, m)| Column {
                    old: None,
                    new: v.cells.get(m).map(|c| shown.cell(c, *m)),
                })
                .collect(),
            ticked: false,
            item: Item::Remove { var: v.id, path },
        });
    }
    // Rows under their group headers, the groups in the order they first come.
    let mut order: Vec<String> = Vec::new();
    for r in &rows {
        if !order.contains(&r.group) {
            order.push(r.group.clone());
        }
    }
    rows.sort_by_key(|r| order.iter().position(|g| *g == r.group));

    let mut columns: Vec<String> = columns.into_iter().map(|(n, _)| n).collect();
    if columns.is_empty() {
        columns.push("Value".to_string());
    }
    Ok(Review {
        columns,
        rows,
        left_out,
        kept_modes,
        added_modes,
        added_collections,
        vars,
        styles,
        fresh,
    })
}

/// What a style token's references are read against.
struct Ctx<'a> {
    doc: &'a Document,
    file: &'a File,
    at_path: &'a HashMap<&'a [String], Target>,
    wanted: &'a HashMap<VarId, &'a Variable>,
    /// The mode a style's literal values are read in: the file's first.
    mode: Option<ModeId>,
}

impl Ctx<'_> {
    /// The variable a reference names, when it is one of `kind`.
    fn var(&self, to: &[String], kind: VarKind) -> Option<VarId> {
        let t = self.doc.variables();
        match self.at_path.get(to) {
            Some(x) if x.kind == kind => Some(x.id),
            Some(_) => None,
            // The document's, by its path — under its collection's group too.
            None => t
                .iter()
                .find(|v| {
                    let path = token_path(&v.name);
                    let grouped = t.collection_of(v.id).is_some_and(|c| {
                        to.first() == Some(&token_path(&c.name).join("_")) && to[1..] == path[..]
                    });
                    v.kind == kind && (path == to || grouped)
                })
                .map(|v| v.id),
        }
    }

    /// The literal a value comes to in the file's first mode, following
    /// references through the file and then the document.
    fn literal(&self, path: &[String], v: &V, kind: VarKind) -> Result<Cell, Refusal> {
        let start = match v {
            V::Lit(c) => return Ok(c.clone()),
            V::Ref(to) => match self.var(to, kind) {
                Some(id) => id,
                None => {
                    return refuse(
                        path,
                        format!("refers to {{{}}}, which is not in the file", to.join(".")),
                    );
                }
            },
            V::Named(..) => return refuse(path, "a style cannot read a string variable"),
        };
        let mut at = start;
        for _ in 0..64 {
            let cell = self
                .wanted
                .get(&at)
                .and_then(|v| {
                    self.mode
                        .and_then(|m| v.cells.get(&m))
                        .or(v.cells.values().next())
                })
                .cloned()
                .or_else(|| {
                    let t = self.doc.variables();
                    let v = t.get(at)?;
                    self.mode
                        .and_then(|m| v.cells.get(&m))
                        .or(v.cells.values().next())
                        .cloned()
                });
            match cell {
                Some(Cell::Alias(to)) => at = to,
                Some(c) => return Ok(c),
                None => break,
            }
        }
        refuse(path, "a reference that comes to no value")
    }

    fn number(&self, path: &[String], v: &V) -> Result<f64, Refusal> {
        match self.literal(path, v, VarKind::Number)? {
            Cell::Number(n) => Ok(n),
            _ => refuse(path, "a number was expected"),
        }
    }

    fn colour(&self, path: &[String], v: &V) -> Result<Color, Refusal> {
        match self.literal(path, v, VarKind::Color)? {
            Cell::Color(c) => Ok(c),
            _ => refuse(path, "a colour was expected"),
        }
    }

    /// A dimension slot that a number variable may bind: its variable when it
    /// is a reference to one, else its px.
    fn bindable(&self, path: &[String], v: &V) -> Result<(f64, Option<VarId>), Refusal> {
        if let V::Ref(to) = v
            && let Some(id) = self.var(to, VarKind::Number)
        {
            return Ok((0.0, Some(id)));
        }
        Ok((self.number(path, v)?, None))
    }

    /// A colour slot a colour variable may bind whole.
    fn brush(&self, path: &[String], v: &V) -> Result<BrushSource, Refusal> {
        if let V::Ref(to) = v
            && let Some(var) = self.var(to, VarKind::Color)
        {
            return Ok(BrushSource::Bound { var, alpha: 1.0 });
        }
        Ok(BrushSource::Literal(Brush::Solid(self.colour(path, v)?)))
    }

    /// The style value `c` asks for, built over the one held where it has one
    /// so item ids and everything the token does not carry are kept. `None`
    /// when a reference names a style of another slot.
    fn style_value(
        &self,
        path: &[String],
        slot: StyleSlot,
        c: &Composite,
        held: Option<&StyleValue>,
        ids: &mut IdSource,
        depth: usize,
    ) -> Result<Option<StyleValue>, Refusal> {
        match c {
            Composite::Ref(to) => {
                let found = self.file.tokens.iter().find(|tk| tk.path == *to);
                match found.map(|tk| &tk.body) {
                    Some(Body::Style { slot: s, value, .. }) if *s == slot && depth < 16 => {
                        self.style_value(path, slot, value, held, ids, depth + 1)
                    }
                    Some(Body::Var {
                        kind: VarKind::Color,
                        ..
                    }) if slot == StyleSlot::Fills => self.style_value(
                        path,
                        slot,
                        &Composite::Color(V::Ref(to.clone())),
                        held,
                        ids,
                        depth + 1,
                    ),
                    Some(_) => Ok(None),
                    None => refuse(
                        path,
                        format!("refers to {{{}}}, which is not in the file", to.join(".")),
                    ),
                }
            }
            Composite::Color(v) => {
                let brush = self.brush(path, v)?;
                Ok(Some(StyleValue::Fills(one_fill(held, ids, |old| {
                    match (old.map(|f| &f.brush), &brush) {
                        (
                            Some(BrushSource::Literal(Brush::Solid(a))),
                            BrushSource::Literal(Brush::Solid(b)),
                        ) if same_colour(*a, *b) => old.map_or(brush.clone(), |f| f.brush.clone()),
                        _ => brush.clone(),
                    }
                }))))
            }
            Composite::Gradient(stops) => {
                let mut read = Vec::new();
                for (v, at) in stops {
                    read.push((self.colour(path, v)?, *at));
                }
                Ok(Some(StyleValue::Fills(one_fill(held, ids, |old| {
                    let held_g = match old.map(|f| &f.brush) {
                        Some(BrushSource::Literal(Brush::Gradient(g))) => Some(g),
                        _ => None,
                    };
                    let alike = held_g.is_some_and(|g| {
                        g.gradient.stops.len() == read.len()
                            && g.gradient.stops.iter().zip(&read).all(|(s, (c, at))| {
                                let drawn = s
                                    .color
                                    .to_alpha_color::<peniko::color::Srgb>()
                                    .multiply_alpha(g.opacity);
                                same_colour(drawn, *c) && (s.offset - at).abs() < 1e-6
                            })
                    });
                    if alike {
                        return old.map_or_else(|| unreachable!(), |f| f.brush.clone());
                    }
                    let stops: Vec<peniko::ColorStop> = read
                        .iter()
                        .map(|(c, at)| peniko::ColorStop {
                            offset: *at,
                            color: peniko::color::DynamicColor::from_alpha_color(*c),
                        })
                        .collect();
                    let mut g = held_g.cloned().unwrap_or_else(|| {
                        GradientBrush::from(peniko::Gradient::new_linear((0.0, 0.0), (0.0, 100.0)))
                    });
                    g.gradient = g.gradient.with_stops(stops.as_slice());
                    g.opacity = 1.0;
                    BrushSource::Literal(Brush::Gradient(g))
                }))))
            }
            Composite::Border {
                color,
                width,
                dashed,
            } => {
                let brush = self.brush(path, color)?;
                let (w, width_var) = self.bindable(path, width)?;
                let list = match held {
                    Some(StyleValue::Strokes(l)) => l.clone(),
                    _ => Vec::new(),
                };
                let at = list.iter().position(|s| s.visible);
                let mut out = list;
                let mut stroke = at.map_or_else(Stroke::default, |i| out[i].value.clone());
                stroke.brush = match (&stroke.brush, &brush) {
                    (
                        BrushSource::Literal(Brush::Solid(a)),
                        BrushSource::Literal(Brush::Solid(b)),
                    ) if at.is_some() && same_colour(*a, *b) => stroke.brush.clone(),
                    _ => brush,
                };
                if width_var.is_some() || !near(stroke.width, w) || stroke.width_var.is_some() {
                    stroke.width = w;
                    stroke.width_var = width_var;
                }
                if !*dashed {
                    stroke.dashes.clear();
                } else if stroke.dashes.is_empty() {
                    stroke.dashes = vec![4.0, 4.0];
                }
                match at {
                    Some(i) => out[i].value = stroke,
                    None => out.push(Keyed::new(ids.mint_item(), stroke)),
                }
                Ok(Some(StyleValue::Strokes(out)))
            }
            Composite::Shadow(layers) => {
                let held = match held {
                    Some(StyleValue::Effects(l)) => l.clone(),
                    _ => Vec::new(),
                };
                let mut shown = held.iter().filter(|e| e.visible);
                let mut out: Vec<Keyed<Effect>> =
                    held.iter().filter(|e| !e.visible).cloned().collect();
                let mut made = Vec::new();
                for l in layers {
                    let old = shown.next();
                    let mut sh = match old.map(|e| &e.kind) {
                        Some(EffectKind::DropShadow(s) | EffectKind::InnerShadow(s)) => s.clone(),
                        _ => Shadow::default(),
                    };
                    let mut vars = old.map_or([None; 4], |e| e.vars);
                    let colour = self.colour(path, &l.color)?;
                    if !same_colour(sh.color, colour) {
                        sh.color = colour;
                    }
                    for (k, v) in [
                        (EFFECT_X, &l.x),
                        (EFFECT_Y, &l.y),
                        (EFFECT_BLUR, &l.blur),
                        (EFFECT_SPREAD, &l.spread),
                    ] {
                        let (n, var) = self.bindable(path, v)?;
                        let n = if k == EFFECT_BLUR {
                            n.clamp(0.0, MAX_BLUR)
                        } else {
                            n
                        };
                        let slot = match k {
                            EFFECT_X => &mut sh.offset.x,
                            EFFECT_Y => &mut sh.offset.y,
                            EFFECT_BLUR => &mut sh.blur,
                            _ => &mut sh.spread,
                        };
                        if var.is_some() || vars[k].is_some() || !near(*slot, n) {
                            *slot = n;
                            vars[k] = var;
                        }
                    }
                    let kind = match l.inset {
                        true => EffectKind::InnerShadow(sh),
                        false => EffectKind::DropShadow(sh),
                    };
                    let effect = Effect {
                        kind,
                        visible: true,
                        vars,
                    };
                    made.push(match old {
                        Some(e) => Keyed::new(e.id, effect),
                        None => Keyed::new(ids.mint_item(), effect),
                    });
                }
                // The shown effects in the file's order, the hidden ones after.
                made.append(&mut out);
                // Held lists keep their order where nothing changed.
                let same: BTreeSet<_> = made.iter().map(|e| e.id).collect();
                if held.len() == made.len() && held.iter().all(|e| same.contains(&e.id)) {
                    let by_id: HashMap<_, _> = made.iter().map(|e| (e.id, e.clone())).collect();
                    made = held.iter().map(|e| by_id[&e.id].clone()).collect();
                }
                Ok(Some(StyleValue::Effects(made)))
            }
            Composite::Type(ty) => {
                let mut s = match held {
                    Some(StyleValue::Text(t)) => (**t).clone(),
                    _ => style::type_part(&TextStyle::default()),
                };
                if let Some(v) = &ty.family
                    && let Cell::String(f) = self.literal(path, v, VarKind::String)?
                {
                    s.font_family = f;
                }
                if let Some(v) = &ty.size {
                    let n = self.number(path, v)?;
                    if !near(s.font_size, n) {
                        s.font_size = n.max(ondin_core::typography::MIN_FONT_SIZE);
                    }
                }
                if let Some(v) = &ty.weight {
                    s.weight = self.number(path, v)?.round().clamp(1.0, 1000.0) as u16;
                }
                if let Some(v) = &ty.letter {
                    let px = self.number(path, v)?;
                    let held_px = match s.letter_spacing {
                        Length::Px(p) => p,
                        Length::Em(m) => m * s.font_size,
                    };
                    if !near(held_px, px) {
                        s.letter_spacing = Length::Px(px);
                    }
                }
                if let Some(v) = &ty.line {
                    let m = self.number(path, v)?;
                    let held_m = match s.line_height {
                        Some(Length::Em(e)) => e,
                        Some(Length::Px(p)) if s.font_size > 0.0 => p / s.font_size,
                        _ => 1.2,
                    };
                    if !near(held_m, m) {
                        s.line_height = Some(Length::Em(m.max(0.0)));
                    }
                }
                Ok(Some(StyleValue::Text(Box::new(style::type_part(&s)))))
            }
        }
    }
}

/// A fills list whose one shown fill takes `brush(held one)`, the rest kept.
fn one_fill(
    held: Option<&StyleValue>,
    ids: &mut IdSource,
    brush: impl FnOnce(Option<&Fill>) -> BrushSource,
) -> Vec<Keyed<Fill>> {
    let mut list = match held {
        Some(StyleValue::Fills(l)) => l.clone(),
        _ => Vec::new(),
    };
    match list.iter().position(|f| f.visible) {
        Some(i) => {
            let b = brush(Some(&list[i].value));
            list[i].value.brush = b;
        }
        None => {
            let b = brush(None);
            list.push(Keyed::new(
                ids.mint_item(),
                Fill {
                    brush: b,
                    visible: true,
                },
            ));
        }
    }
    list
}

impl Review {
    /// Keep only what changed between the file as last applied, `old`, and
    /// `file` as it is now — a linked file's review, which leaves alone what the
    /// document changed since.
    pub fn since(&mut self, file: &File, old: &File) {
        let before = |tk: &Token| {
            tk.id
                .and_then(|id| old.tokens.iter().find(|o| o.id == Some(id)))
                .or_else(|| old.tokens.iter().find(|o| o.path == tk.path))
        };
        let modes = &self.added_modes;
        self.rows.retain(|r| match &r.item {
            Item::Mode(i) => !old
                .collections
                .iter()
                .any(|c| c.modes.contains(&modes[*i].1.name)),
            Item::Var { token, .. } | Item::Style { token, .. } | Item::Kit { token } => {
                let tk = &file.tokens[*token];
                before(tk) != Some(tk)
            }
            Item::Remove { var, path } => old
                .tokens
                .iter()
                .any(|o| o.id == Some(var.0) || o.path == *path),
        });
    }

    /// The rows that can be applied: all but a kit's.
    pub fn changes(&self) -> usize {
        self.rows.iter().filter(|r| r.change != Change::Kit).count()
    }

    /// One transaction applying the rows `ticked` marks (one flag per row), or
    /// the sentence that says why it cannot be applied.
    pub fn ops(&self, doc: &Document, ticked: &[bool]) -> Result<Vec<Operation>, String> {
        let on = |i: usize| ticked.get(i).copied().unwrap_or(false);
        let t = doc.variables();
        let mut modes_on = vec![false; self.added_modes.len()];
        let mut vars_on = BTreeSet::new();
        let mut styles_on = BTreeSet::new();
        let mut removed = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            if !on(i) {
                continue;
            }
            match &r.item {
                Item::Mode(m) => modes_on[*m] = true,
                Item::Var { plan, .. } => {
                    vars_on.insert(*plan);
                }
                Item::Style { plan, .. } => {
                    styles_on.insert(*plan);
                }
                Item::Remove { var, .. } => removed.push(*var),
                Item::Kit { .. } => {}
            }
        }
        let dropped: BTreeSet<ModeId> = self
            .added_modes
            .iter()
            .zip(&modes_on)
            .filter(|(_, on)| !**on)
            .map(|((_, m), _)| m.id)
            .collect();
        let shaped = |v: &Variable| {
            let mut v = v.clone();
            v.cells.retain(|m, _| !dropped.contains(m));
            v
        };
        let mut ops = Vec::new();
        if !removed.is_empty() {
            ops.extend(variable::remove_ops(doc, &removed));
        }
        let by_id: HashMap<VarId, &Wanted> = self.vars.iter().map(|w| (w.var.id, w)).collect();
        // A new collection is made when one of its modes is ticked.
        for c in &self.added_collections {
            let wanted = self
                .added_modes
                .iter()
                .zip(&modes_on)
                .any(|((x, _), on)| *x == c.id && *on);
            if wanted {
                ops.push(Operation::AddCollection {
                    collection: c.clone(),
                    index: t.collections().len(),
                });
            }
        }
        let mut index: HashMap<CollectionId, usize> = HashMap::new();
        for ((c, m), _) in self
            .added_modes
            .iter()
            .zip(&modes_on)
            .filter(|(_, on)| **on)
        {
            let at = index
                .entry(*c)
                .or_insert_with(|| t.collection(*c).map_or(0, |x| x.modes.len()));
            let default = t
                .collection(*c)
                .and_then(variable::Collection::default_mode);
            let mut cells = Vec::new();
            for v in t.of_collection(*c).filter(|v| !removed.contains(&v.id)) {
                let cell = by_id
                    .get(&v.id)
                    .and_then(|w| w.var.cells.get(&m.id))
                    .or_else(|| default.and_then(|d| v.cells.get(&d)))
                    .cloned()
                    .map(|cell| match cell {
                        // A removal turns aliases to it into values; so does this.
                        Cell::Alias(to) if removed.contains(&to) => default
                            .and_then(|d| t.resolve(v.id, d))
                            .map_or(cell, Cell::from),
                        cell => cell,
                    });
                if let Some(cell) = cell {
                    cells.push((v.id, cell));
                }
            }
            ops.push(Operation::AddMode {
                collection: *c,
                mode: m.clone(),
                index: *at,
                cells,
            });
            *at += 1;
        }
        for p in &vars_on {
            let w = &self.vars[*p];
            let var = shaped(&w.var);
            if var.cells.is_empty() {
                return Err("Tick a mode: a variable needs one to hold its value".to_string());
            }
            // A scope that drops a field detaches what it held there.
            if !w.new && t.get(var.id).is_some_and(|h| h.scope != var.scope) {
                ops.extend(
                    variable::scope_ops(doc, var.id, var.scope)
                        .into_iter()
                        .filter(|op| !matches!(op, Operation::SetVariable { .. })),
                );
            }
            ops.push(match w.new {
                true => Operation::AddVariable { var },
                false => Operation::SetVariable { var },
            });
        }
        for p in &styles_on {
            let w = &self.styles[*p];
            match (w.new, doc.styles().get(w.style.id)) {
                (false, Some(held)) => {
                    if held.name != w.style.name {
                        ops.push(Operation::SetStyleName {
                            id: held.id,
                            name: w.style.name.clone(),
                        });
                    }
                    if held.value != w.style.value {
                        ops.push(Operation::SetStyleValue {
                            id: held.id,
                            value: w.style.value.clone(),
                        });
                    }
                    if held.description != w.style.description {
                        ops.push(Operation::SetDescription {
                            of: Described::Style(held.id),
                            text: w.style.description.clone(),
                        });
                    }
                }
                _ => ops.push(Operation::AddStyle {
                    style: w.style.clone(),
                }),
            }
        }
        // A new variable something ticked reads, left unticked.
        let added: BTreeSet<VarId> = vars_on
            .iter()
            .filter(|p| self.vars[**p].new)
            .map(|p| self.vars[*p].var.id)
            .collect();
        for op in &ops {
            let (who, reads): (String, Vec<VarId>) = match op {
                Operation::AddVariable { var } | Operation::SetVariable { var } => (
                    token_path(&var.name).join("."),
                    var.cells
                        .values()
                        .filter_map(|c| match c {
                            Cell::Alias(to) => Some(*to),
                            _ => None,
                        })
                        .collect(),
                ),
                Operation::AddMode { cells, .. } => (
                    String::new(),
                    cells
                        .iter()
                        .filter_map(|(_, c)| match c {
                            Cell::Alias(to) => Some(*to),
                            _ => None,
                        })
                        .collect(),
                ),
                Operation::AddStyle { style: s } => {
                    (token_path(&s.name).join("."), style_vars(&s.value))
                }
                Operation::SetStyleValue { value, id } => (
                    doc.styles()
                        .get(*id)
                        .map_or(String::new(), |s| token_path(&s.name).join(".")),
                    style_vars(value),
                ),
                _ => continue,
            };
            if let Some(to) = reads
                .iter()
                .find(|to| self.fresh.contains_key(to) && !added.contains(to))
            {
                let target = &self.fresh[to];
                return Err(match who.is_empty() {
                    true => format!("The added mode reads {target}, which is not ticked"),
                    false => format!("{who} reads {target}, which is not ticked"),
                });
            }
            if let Some(to) = reads.iter().find(|to| removed.contains(to)) {
                let target = t
                    .get(*to)
                    .map_or(String::new(), |v| token_path(&v.name).join("."));
                return Err(format!("{who} reads {target}, which is ticked for removal"));
            }
        }
        let mut scratch = doc.clone();
        if let Err(e) = scratch.apply(&Transaction(ops.clone())) {
            return Err(format!("This cannot be applied: {e}"));
        }
        Ok(ops)
    }
}

/// Every variable a style's value binds.
fn style_vars(v: &StyleValue) -> Vec<VarId> {
    let mut out = Vec::new();
    match v {
        StyleValue::Fills(l) => out.extend(l.iter().filter_map(|f| f.brush.var())),
        StyleValue::Strokes(l) => {
            for s in l {
                out.extend(s.brush.var());
                out.extend(s.width_var);
            }
        }
        StyleValue::Effects(l) => out.extend(l.iter().flat_map(|e| e.vars.into_iter().flatten())),
        StyleValue::Layout(l) => out.extend(l.spacing_vars.into_iter().flatten()),
        StyleValue::Text(_) => {}
    }
    out
}

fn style_reads(v: &StyleValue, var: VarId) -> bool {
    style_vars(v).contains(&var)
}

/// How many layers and styles read `var`.
fn readers(doc: &Document, var: VarId) -> usize {
    variable::users(doc, var).len()
        + doc
            .styles()
            .iter()
            .filter(|s| style_reads(&s.value, var))
            .count()
}

fn group_of(path: &[String]) -> String {
    match path.split_last() {
        Some((_, [])) | None => String::new(),
        Some((_, g)) => g.join(" / "),
    }
}

fn count(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}

fn kind_word(k: VarKind) -> &'static str {
    match k {
        VarKind::Color => "colour",
        VarKind::Number => "number",
        VarKind::Boolean => "boolean",
        VarKind::String => "string",
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

/// Two colours the export writes alike: every component within its rounding.
fn same_colour(a: Color, b: Color) -> bool {
    a.components
        .iter()
        .zip(b.components)
        .all(|(x, y)| (f64::from(*x) - f64::from(y)).abs() <= 1e-6)
}

fn same_cell(a: &Cell, b: &Cell) -> bool {
    match (a, b) {
        (Cell::Color(x), Cell::Color(y)) => same_colour(*x, *y),
        _ => a == b,
    }
}

/// How a row draws values: literals, and aliases by their target's path.
struct Shower<'a> {
    doc: &'a Document,
    wanted: &'a HashMap<VarId, &'a Variable>,
}

impl Shower<'_> {
    fn var(&self, id: VarId) -> Option<&Variable> {
        self.wanted
            .get(&id)
            .copied()
            .or_else(|| self.doc.variables().get(id))
    }

    fn cell(&self, c: &Cell, mode: ModeId) -> Shown {
        match c {
            Cell::Alias(to) => {
                let name = self
                    .var(*to)
                    .map_or("?".to_string(), |v| token_path(&v.name).join("."));
                // The colour it comes to, for the swatch.
                let mut at = *to;
                let mut swatch = None;
                for _ in 0..64 {
                    match self
                        .var(at)
                        .and_then(|v| v.cells.get(&mode).or(v.cells.values().next()))
                    {
                        Some(Cell::Alias(n)) => at = *n,
                        Some(Cell::Color(c)) => {
                            swatch = Some(*c);
                            break;
                        }
                        _ => break,
                    }
                }
                Shown {
                    text: format!("\u{2192} {name}"),
                    swatch,
                }
            }
            Cell::Color(c) => Shown {
                text: hex(*c),
                swatch: Some(*c),
            },
            Cell::Number(n) => Shown {
                text: number(*n),
                swatch: None,
            },
            Cell::String(s) => Shown {
                text: format!("\u{201c}{}\u{201d}", s.replace('\n', " ")),
                swatch: None,
            },
            Cell::Boolean(b) => Shown {
                text: b.to_string(),
                swatch: None,
            },
        }
    }

    fn style(&self, v: &StyleValue) -> Shown {
        let name = |id: VarId| {
            self.var(id)
                .map_or("?".to_string(), |v| token_path(&v.name).join("."))
        };
        match v {
            StyleValue::Fills(l) => match l.iter().find(|f| f.visible).map(|f| &f.brush) {
                Some(BrushSource::Bound { var, .. }) => Shown {
                    text: format!("\u{2192} {}", name(*var)),
                    swatch: None,
                },
                Some(BrushSource::Literal(Brush::Solid(c))) => Shown {
                    text: hex(*c),
                    swatch: Some(*c),
                },
                Some(BrushSource::Literal(Brush::Gradient(g))) => Shown {
                    text: format!("Gradient \u{b7} {}", count(g.gradient.stops.len(), "stop")),
                    swatch: g
                        .gradient
                        .stops
                        .first()
                        .map(|s| s.color.to_alpha_color::<peniko::color::Srgb>()),
                },
                _ => Shown {
                    text: "Fill".to_string(),
                    swatch: None,
                },
            },
            StyleValue::Strokes(l) => match l.iter().find(|s| s.visible) {
                Some(s) => {
                    let width = s.width_var.map_or_else(|| number(s.width), name);
                    let (colour, swatch) = match &s.brush {
                        BrushSource::Literal(Brush::Solid(c)) => (hex(*c), Some(*c)),
                        BrushSource::Bound { var, .. } => (name(*var), None),
                        _ => ("paint".to_string(), None),
                    };
                    Shown {
                        text: format!("{width} \u{b7} {colour}"),
                        swatch,
                    }
                }
                None => Shown {
                    text: "Stroke".to_string(),
                    swatch: None,
                },
            },
            StyleValue::Effects(l) => {
                let shown: Vec<_> = l.iter().filter(|e| e.visible).collect();
                match shown.as_slice() {
                    [e] => match &e.kind {
                        EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => {
                            let n = |k: usize, v: f64| e.vars[k].map_or_else(|| number(v), name);
                            Shown {
                                text: format!(
                                    "{} {} {} {}",
                                    n(EFFECT_X, s.offset.x),
                                    n(EFFECT_Y, s.offset.y),
                                    n(EFFECT_BLUR, s.blur),
                                    n(EFFECT_SPREAD, s.spread)
                                ),
                                swatch: Some(s.color),
                            }
                        }
                        _ => Shown {
                            text: "Effect".to_string(),
                            swatch: None,
                        },
                    },
                    list => Shown {
                        text: count(list.len(), "shadow"),
                        swatch: None,
                    },
                }
            }
            StyleValue::Text(s) => Shown {
                text: format!(
                    "{} \u{b7} {} \u{b7} {}",
                    s.font_family,
                    number(s.font_size),
                    s.weight
                ),
                swatch: None,
            },
            StyleValue::Layout(_) => Shown {
                text: "Layout".to_string(),
                swatch: None,
            },
        }
    }
}

fn hex(c: Color) -> String {
    let rgba = c.to_rgba8();
    let mut s = format!("#{:02X}{:02X}{:02X}", rgba.r, rgba.g, rgba.b);
    if rgba.a < 255 {
        s.push_str(&format!(
            " \u{b7} {}%",
            (f64::from(c.components[3]) * 100.0).round()
        ));
    }
    s
}

fn number(n: f64) -> String {
    let s = format!("{:.4}", n);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}
