//! The document's variables and styles as a design-tokens file in the W3C Design
//! Tokens Community Group format (2025.10): `tokens.json`, beside the
//! `tokens.css` code export writes.
//!
//! **Slash paths become groups**, so `brand/500` is `{"brand": {"500": …}}` and
//! an alias to it is `{brand.500}`. A token whose path is also a group (`brand`
//! beside `brand/500`) is that group's `$root`.
//!
//! **One mode is the `$value`** — the document's default unless asked for
//! another — and every mode's value rides in the token's `$extensions` under
//! [`EXTENSION`], with the variable's id, which is what an import matches on
//! first so a rename made in code is not a delete plus a create.
//!
//! **A document with more than one collection writes each as a top-level
//! group**, its mode list and `valueMode` in the group's extension; a document
//! with one writes as before collections, its modes in the root's, so existing
//! files round-trip unchanged.
//!
//! **A number's type is its scope's** when it has one: lengths only is a px
//! `dimension`, weight only a `fontWeight`, anything else a `number`. A number
//! that may bind every field is typed **by what it binds**: read only as lengths
//! (sizes, radii, gaps, stroke widths, shadow offsets, px type sizes) a
//! `dimension`, only as a font weight a `fontWeight`, anything else — an
//! opacity, an em length, both, or nothing — a `number`. An alias and its
//! target pool what reads them, since the format gives them one type. The scope
//! itself rides in the extension, so a round trip keeps it exactly.
//!
//! Strings and booleans have no DTCG type and ride in the root's — or their
//! collection's — `$extensions`.
//! Styles go under a `styles` group, one subgroup per slot, as the composite type
//! each maps to; what has no DTCG form is left out and said so in
//! [`Tokens::notes`].

use ondin_core::Document;
use ondin_core::kurbo::Vec2;
use ondin_core::peniko::Color;
use ondin_core::style::{StyleSlot, StyleValue};
use ondin_core::typography::{Length, TextStyle};
use ondin_core::variable::{self, Cell, CollectionId, ModeId, VarId, VarKind, Variable, Variables};
use ondin_core::{Brush, BrushSource, EffectKind, NodeKind};
use serde_json::{Map, Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};

/// The `$extensions` key Ondin's own data is filed under: the reverse domain of
/// the app id, as the format asks.
pub const EXTENSION: &str = "io.github.fadion.ondin";

/// A written token file and what did not make it into one.
#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    /// The file, pretty-printed, ending in a newline.
    pub json: String,
    /// One sentence per variable or style left out or changed on the way out.
    pub notes: Vec<String>,
}

/// How a number variable is read, gathered over every binding of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Read {
    Length,
    Weight,
    Other,
}

/// `doc`'s own variables and styles as a DTCG file, `mode`'s values the
/// `$value`s of its collection — each other collection's, and every one's when
/// `mode` is `None` or not one of the document's, its default.
pub fn tokens(doc: &Document, mode: Option<ModeId>) -> Tokens {
    let t = doc.variables();
    let value_modes = t
        .collections()
        .iter()
        .filter_map(|c| {
            let m = mode.filter(|m| c.has(*m)).or_else(|| c.default_mode())?;
            Some((c.id, m))
        })
        .collect();
    let mut w = Writer {
        t,
        value_modes,
        reads: number_reads(doc),
        paths: BTreeMap::new(),
        mode_keys: BTreeMap::new(),
        collection_keys: BTreeMap::new(),
        notes: Vec::new(),
    };
    w.name_collections();
    w.name_modes();
    w.name_variables();

    let mut root = Map::new();
    let mut loose: BTreeMap<CollectionId, Map<String, Json>> = BTreeMap::new();
    for v in t.local() {
        match v.kind {
            VarKind::Color | VarKind::Number => {
                let Some(path) = w.paths.get(&v.id).cloned() else {
                    continue;
                };
                let token = w.variable_token(v);
                insert(&mut root, &path, token, &mut w.notes, &v.name);
            }
            VarKind::String | VarKind::Boolean => {
                let Some(c) = t.collection_of(v.id) else {
                    continue;
                };
                let entry = w.loose_variable(v);
                loose.entry(c.id).or_default().insert(v.name.clone(), entry);
            }
        }
    }
    // Each collection's group carries its modes, when there are groups.
    for c in t.collections() {
        let Some(key) = w.collection_keys.get(&c.id).cloned() else {
            continue;
        };
        let mut ext = Map::new();
        let mut col = Map::new();
        col.insert("id".to_string(), Json::String(c.id.0.to_wire()));
        col.insert("name".to_string(), Json::String(c.name.clone()));
        col.insert("modes".to_string(), w.modes_list(c));
        if let Some(m) = w.value_modes.get(&c.id).and_then(|m| t.mode(*m)) {
            col.insert("valueMode".to_string(), Json::String(m.name.clone()));
        }
        ext.insert("collection".to_string(), Json::Object(col));
        if let Some(vars) = loose.remove(&c.id) {
            ext.insert("variables".to_string(), Json::Object(vars));
        }
        let group = root.entry(key).or_insert_with(|| Json::Object(Map::new()));
        if let Json::Object(g) = group {
            g.insert(
                "$extensions".to_string(),
                json!({ EXTENSION: Json::Object(ext) }),
            );
        }
    }

    let styles = w.styles(doc);
    let mut styles_key = None;
    if !styles.is_empty() {
        let key = std::iter::once("styles".to_string())
            .chain(std::iter::once("ondin-styles".to_string()))
            .chain((2..).map(|n| format!("ondin-styles-{n}")))
            .find(|k| !root.contains_key(k))
            .expect("an unbounded run of names has a free one");
        if key != "styles" {
            w.notes.push(format!(
                "The styles are under `{key}`: a variable group is already named `styles`."
            ));
        }
        styles_key = Some(key.clone());
        root.insert(key, Json::Object(styles));
    }

    let mut ext = Map::new();
    // Named so the import finds it, whatever it is called.
    if let Some(key) = styles_key {
        ext.insert("styles".to_string(), Json::String(key));
    }
    // One collection: its modes at the root, as before collections.
    if w.collection_keys.is_empty() {
        let c = t.first_collection();
        ext.insert("modes".to_string(), w.modes_list(c));
        if let Some(m) = w.value_modes.get(&c.id).and_then(|m| t.mode(*m)) {
            ext.insert("valueMode".to_string(), Json::String(m.name.clone()));
        }
        if let Some(vars) = loose.remove(&c.id) {
            ext.insert("variables".to_string(), Json::Object(vars));
        }
    }
    let mut exts = Map::new();
    exts.insert(EXTENSION.to_string(), Json::Object(ext));
    root.insert("$extensions".to_string(), Json::Object(exts));

    let mut json =
        serde_json::to_string_pretty(&Json::Object(root)).expect("a JSON value always serializes");
    json.push('\n');
    Tokens {
        json,
        notes: w.notes,
    }
}

struct Writer<'a> {
    t: &'a Variables,
    /// Each collection's `$value` mode.
    value_modes: BTreeMap<CollectionId, ModeId>,
    reads: BTreeMap<VarId, BTreeSet<Read>>,
    /// Each exported variable's token path, its segments already made legal, and
    /// ending in `$root` when the token is its group's.
    paths: BTreeMap<VarId, Vec<String>>,
    /// The key each mode's values are filed under in a token's extension: its
    /// name, numbered when an earlier mode of its collection has the same one.
    mode_keys: BTreeMap<ModeId, String>,
    /// Each collection's top-level group — none in a document with one.
    collection_keys: BTreeMap<CollectionId, String>,
    notes: Vec<String>,
}

impl Writer<'_> {
    /// `note` unless it is already said: a variable read in every mode would say
    /// it once per mode.
    fn note(&mut self, note: String) {
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    /// A top-level group for each collection when there are two or more: its
    /// name made legal, numbered when another holds it, never `styles`.
    fn name_collections(&mut self) {
        if !self.t.has_collections() {
            return;
        }
        let mut taken: BTreeSet<String> = ["styles".to_string()].into();
        for c in self.t.collections() {
            let legal = token_path(&c.name).join("_");
            let key = std::iter::once(legal.clone())
                .chain((2..).map(|n| format!("{legal}_{n}")))
                .find(|k| !taken.contains(k))
                .expect("an unbounded run of names has a free one");
            if key != c.name {
                self.notes
                    .push(format!("The collection {} is written as `{key}`.", c.name));
            }
            taken.insert(key.clone());
            self.collection_keys.insert(c.id, key);
        }
    }

    /// A key for every mode, unique in its collection although names need not
    /// be.
    fn name_modes(&mut self) {
        for c in self.t.collections() {
            let mut taken = BTreeSet::new();
            for m in &c.modes {
                let key = std::iter::once(m.name.clone())
                    .chain((2..).map(|n| format!("{} {n}", m.name)))
                    .find(|k| !taken.contains(k))
                    .expect("an unbounded run of names has a free one");
                if key != m.name {
                    self.notes.push(format!(
                        "Two modes are named {}: one's values are under `{key}`.",
                        m.name
                    ));
                }
                taken.insert(key.clone());
                self.mode_keys.insert(m.id, key);
            }
        }
    }

    /// A collection's modes as an extension lists them: id, name, and the key
    /// its values are under where that is not the name.
    fn modes_list(&self, c: &variable::Collection) -> Json {
        Json::Array(
            c.modes
                .iter()
                .map(|m| {
                    let mut out = json!({ "id": m.id.0.to_wire(), "name": m.name });
                    let key = &self.mode_keys[&m.id];
                    if *key != m.name {
                        out["key"] = Json::String(key.clone());
                    }
                    out
                })
                .collect(),
        )
    }

    /// The `$value` mode `v` is written in: its collection's.
    fn value_mode(&self, v: VarId) -> Option<ModeId> {
        self.value_modes.get(&self.t.collection_of(v)?.id).copied()
    }

    /// The modes of `v`'s collection, in order.
    fn modes_of(&self, v: &Variable) -> Vec<ModeId> {
        self.t.columns(v)
    }

    /// A legal path for every colour and number variable of the document's own,
    /// one each: a name that becomes another's path is numbered. A path another
    /// continues is that group's `$root`, which is how a reference has to name it.
    /// Under its collection's group, when there are groups.
    fn name_variables(&mut self) {
        let mut taken: BTreeSet<Vec<String>> = BTreeSet::new();
        for v in self.t.local() {
            if !matches!(v.kind, VarKind::Color | VarKind::Number) {
                continue;
            }
            let prefix: Vec<String> = self
                .t
                .collection_of(v.id)
                .and_then(|c| self.collection_keys.get(&c.id))
                .cloned()
                .into_iter()
                .collect();
            let grouped = prefix.len();
            let legal: Vec<String> = prefix.into_iter().chain(token_path(&v.name)).collect();
            let mut path = legal.clone();
            let mut n = 2;
            while taken.contains(&path) {
                let last = path.len() - 1;
                path[last] = format!("{}_{n}", legal[last]);
                n += 1;
            }
            if path != legal {
                self.notes.push(format!(
                    "{}: written as {}, since another variable is written at {}.",
                    v.name,
                    path.join("."),
                    legal.join(".")
                ));
            } else if path[grouped..].join("/") != v.name {
                self.notes.push(format!(
                    "{}: written as {}, since a token name cannot hold `.`, `{{`, `}}` or a leading `$`.",
                    v.name,
                    path.join(".")
                ));
            }
            taken.insert(path.clone());
            self.paths.insert(v.id, path);
        }
        for path in self.paths.values_mut() {
            if taken
                .iter()
                .any(|p| p.len() > path.len() && p.starts_with(path))
            {
                path.push("$root".to_string());
            }
        }
    }

    /// The `$type` a token of `v` declares: its scope's, else its reads'. An
    /// alias declares one too: its reads were handed to its target, so the two
    /// agree unless a scope says otherwise.
    fn type_of(&self, v: &Variable) -> &'static str {
        match v.kind {
            VarKind::Color => "color",
            _ if v.scope.is_some() => scope_type(v.scope),
            _ => {
                let reads = self.reads.get(&v.id).filter(|r| !r.is_empty());
                match reads {
                    Some(r) if r.iter().all(|x| *x == Read::Length) => "dimension",
                    Some(r) if r.iter().all(|x| *x == Read::Weight) => "fontWeight",
                    _ => "number",
                }
            }
        }
    }

    fn variable_token(&mut self, v: &Variable) -> Json {
        let ty = self.type_of(v);
        let mut token = Map::new();
        token.insert("$type".to_string(), Json::String(ty.to_string()));
        let value = self
            .value_mode(v.id)
            .and_then(|m| Some((m, v.cells.get(&m)?)))
            .map(|(m, c)| self.cell(v, c, ty, m));
        token.insert("$value".to_string(), value.unwrap_or(Json::Null));
        if !v.description.is_empty() {
            token.insert(
                "$description".to_string(),
                Json::String(v.description.clone()),
            );
        }
        let mut modes = Map::new();
        for m in self.modes_of(v) {
            if let Some(c) = v.cells.get(&m) {
                let value = self.cell(v, c, ty, m);
                modes.insert(self.mode_keys[&m].clone(), value);
            }
        }
        let mut ext = json!({ "id": v.id.0.to_wire(), "modes": modes });
        if let Some(s) = v.scope {
            ext["scope"] = serde_json::to_value(s).expect("a scope serializes");
        }
        token.insert("$extensions".to_string(), json!({ EXTENSION: ext }));
        Json::Object(token)
    }

    /// One cell, `mode`'s, as a `$value` of type `ty`: a literal, or a reference
    /// to the token it aliases. An alias to a variable with no token here (a
    /// kit's) is written as the value it resolves to in `mode`.
    fn cell(&mut self, v: &Variable, cell: &Cell, ty: &str, mode: ModeId) -> Json {
        match cell {
            Cell::Color(c) => color(*c),
            Cell::Number(n) => number(*n, ty),
            Cell::Alias(to) => match self.paths.get(to) {
                Some(path) => Json::String(reference(path)),
                None => {
                    let resolved = self.t.resolve(v.id, mode);
                    self.note(format!(
                        "{}: aliases a variable that is not exported (a kit's), so it is written resolved.",
                        v.name
                    ));
                    match resolved {
                        Some(variable::Value::Color(c)) => color(c),
                        Some(variable::Value::Number(n)) => number(n, ty),
                        _ => Json::Null,
                    }
                }
            },
            Cell::Boolean(b) => Json::Bool(*b),
            Cell::String(s) => Json::String(s.clone()),
        }
    }

    /// A string or boolean variable, for the root's `$extensions`.
    fn loose_variable(&mut self, v: &Variable) -> Json {
        let kind = match v.kind {
            VarKind::String => "string",
            _ => "boolean",
        };
        let mut modes = Map::new();
        for m in self.modes_of(v) {
            if let Some(c) = v.cells.get(&m) {
                let value = match c {
                    Cell::Alias(to) => match self.t.get(*to) {
                        // Named with its collection where that is another.
                        Some(target) => {
                            let theirs = self.t.collection_of(*to).map(|c| c.id);
                            let mine = self.t.collection_of(v.id).map(|c| c.id);
                            match theirs.filter(|c| Some(*c) != mine) {
                                Some(c) => json!({
                                    "alias": target.name,
                                    "collection": self.t.collection(c).map(|c| c.name.clone()),
                                }),
                                None => json!({ "alias": target.name }),
                            }
                        }
                        None => Json::Null,
                    },
                    Cell::Boolean(b) => Json::Bool(*b),
                    Cell::String(s) => Json::String(s.clone()),
                    _ => Json::Null,
                };
                modes.insert(self.mode_keys[&m].clone(), value);
            }
        }
        let mut out = json!({ "id": v.id.0.to_wire(), "kind": kind, "modes": modes });
        if !v.description.is_empty() {
            out["description"] = Json::String(v.description.clone());
        }
        if let Some(s) = v.scope {
            out["scope"] = serde_json::to_value(s).expect("a scope serializes");
        }
        out
    }

    /// The document's own styles, by slot, each as the composite token it maps
    /// to. A slot with nothing exported has no group.
    fn styles(&mut self, doc: &Document) -> Map<String, Json> {
        let mut out = Map::new();
        for slot in StyleSlot::ALL {
            let mut group = Map::new();
            for s in doc.styles().of_slot(slot).filter(|s| s.origin.is_none()) {
                let Some(token) = self.style_token(&s.name, &s.value) else {
                    continue;
                };
                let mut token = token;
                if !s.description.is_empty()
                    && let Json::Object(m) = &mut token
                {
                    m.insert(
                        "$description".to_string(),
                        Json::String(s.description.clone()),
                    );
                }
                if let Json::Object(m) = &mut token {
                    m.insert(
                        "$extensions".to_string(),
                        json!({ EXTENSION: { "id": s.id.0.to_wire() } }),
                    );
                }
                let label = format!("{} style {}", slot.title(), s.name);
                let path = token_path(&s.name);
                if path.join("/") != s.name {
                    self.notes.push(format!(
                        "{label}: written as {}, since a token name cannot hold `.`, `{{`, `}}` or a leading `$`.",
                        path.join(".")
                    ));
                }
                insert(&mut group, &path, token, &mut self.notes, &label);
            }
            if !group.is_empty() {
                let key = match slot {
                    StyleSlot::Fills => "fill",
                    StyleSlot::Strokes => "stroke",
                    StyleSlot::Effects => "effect",
                    StyleSlot::Text => "type",
                    StyleSlot::Layout => "layout",
                };
                out.insert(key.to_string(), Json::Object(group));
            }
        }
        out
    }

    fn style_token(&mut self, name: &str, value: &StyleValue) -> Option<Json> {
        let left_out = |notes: &mut Vec<String>, why: &str| {
            notes.push(format!("{name}: {why}, so the style is left out."));
            None
        };
        match value {
            StyleValue::Fills(fills) => {
                let shown: Vec<_> = fills.iter().filter(|f| f.visible).collect();
                let [fill] = shown.as_slice() else {
                    return left_out(
                        &mut self.notes,
                        "a fill style of other than one fill has no DTCG type",
                    );
                };
                match &fill.brush {
                    BrushSource::Bound { var, alpha } if *alpha >= 1.0 => {
                        let r = self.reference_to(*var)?;
                        Some(json!({ "$type": "color", "$value": r }))
                    }
                    BrushSource::Bound { .. } => left_out(
                        &mut self.notes,
                        "a colour variable at partial opacity cannot be a reference",
                    ),
                    BrushSource::Literal(Brush::Solid(c)) => {
                        Some(json!({ "$type": "color", "$value": color(*c) }))
                    }
                    BrushSource::Literal(Brush::Gradient(g)) => {
                        let stops: Vec<Json> = g
                            .gradient
                            .stops
                            .iter()
                            .map(|s| {
                                let c = s
                                    .color
                                    .to_alpha_color::<ondin_core::peniko::color::Srgb>()
                                    .multiply_alpha(g.opacity);
                                json!({ "color": color(c), "position": f64::from(s.offset) })
                            })
                            .collect();
                        self.notes.push(format!(
                            "{name}: a DTCG gradient holds the stops only, not the gradient's kind or direction."
                        ));
                        Some(json!({ "$type": "gradient", "$value": stops }))
                    }
                    BrushSource::Literal(Brush::Image(_)) => {
                        left_out(&mut self.notes, "an image fill has no DTCG type")
                    }
                }
            }
            StyleValue::Strokes(strokes) => {
                let shown: Vec<_> = strokes.iter().filter(|s| s.visible).collect();
                let [stroke] = shown.as_slice() else {
                    return left_out(
                        &mut self.notes,
                        "a stroke style of other than one stroke has no DTCG type",
                    );
                };
                let colour = match &stroke.brush {
                    BrushSource::Bound { var, alpha } if *alpha >= 1.0 => {
                        self.reference_to(*var)?
                    }
                    BrushSource::Literal(Brush::Solid(c)) => color(*c),
                    _ => {
                        return left_out(
                            &mut self.notes,
                            "a border's colour must be one solid colour",
                        );
                    }
                };
                let width = match stroke.width_var {
                    Some(v) => self.dimension_to(v, name)?,
                    None => px(stroke.width),
                };
                let style = if stroke.dashes.is_empty() {
                    "solid"
                } else {
                    "dashed"
                };
                Some(json!({
                    "$type": "border",
                    "$value": { "color": colour, "width": width, "style": style }
                }))
            }
            StyleValue::Effects(effects) => {
                let mut shadows = Vec::new();
                for e in effects.iter().filter(|e| e.visible) {
                    let (s, inset) = match &e.kind {
                        EffectKind::DropShadow(s) => (s, false),
                        EffectKind::InnerShadow(s) => (s, true),
                        EffectKind::LayerBlur { .. }
                        | EffectKind::BackgroundBlur { .. }
                        | EffectKind::Filters(_) => {
                            return left_out(
                                &mut self.notes,
                                "a blur or a colour filter has no DTCG type",
                            );
                        }
                    };
                    if s.blends() {
                        self.notes.push(format!(
                            "{name}: a shadow's blend mode has no DTCG field, so it is written without it."
                        ));
                    }
                    let mut slot = |k: usize, literal: f64| -> Option<Json> {
                        match e.vars[k] {
                            Some(v) => self.dimension_to(v, name),
                            None => Some(px(literal)),
                        }
                    };
                    let Vec2 { x, y } = s.offset;
                    shadows.push(json!({
                        "color": color(s.color),
                        "offsetX": slot(ondin_core::effect::EFFECT_X, x)?,
                        "offsetY": slot(ondin_core::effect::EFFECT_Y, y)?,
                        "blur": slot(ondin_core::effect::EFFECT_BLUR, s.blur)?,
                        "spread": slot(ondin_core::effect::EFFECT_SPREAD, s.spread)?,
                        "inset": inset,
                    }));
                }
                match shadows.len() {
                    0 => left_out(&mut self.notes, "it holds no visible effect"),
                    1 => Some(json!({ "$type": "shadow", "$value": shadows.remove(0) })),
                    _ => Some(json!({ "$type": "shadow", "$value": shadows })),
                }
            }
            StyleValue::Text(style) => Some(self.typography(name, style)),
            StyleValue::Layout(_) => left_out(&mut self.notes, "a layout has no DTCG type"),
        }
    }

    /// A text style as a `typography` token. An em letter spacing is turned into
    /// px at the style's size; an Auto line height, which is the font's own, has
    /// no number and is written as 1.2.
    fn typography(&mut self, name: &str, s: &TextStyle) -> Json {
        let line_height = match s.line_height {
            Some(Length::Em(m)) => m,
            Some(Length::Px(v)) if s.font_size > 0.0 => v / s.font_size,
            _ => {
                self.notes.push(format!(
                    "{name}: an Auto line height is the font's own, written as 1.2."
                ));
                1.2
            }
        };
        let letter_spacing = match s.letter_spacing {
            Length::Px(v) => v,
            Length::Em(m) => m * s.font_size,
        };
        let plain = TextStyle::default();
        let lost: Vec<&str> = [
            (s.italic, "italic"),
            (s.word_spacing != plain.word_spacing, "word spacing"),
            (
                s.underline.is_some() || s.strikethrough.is_some(),
                "decorations",
            ),
            (s.case != plain.case, "case"),
            (s.baseline_shift != plain.baseline_shift, "baseline shift"),
            (!s.features.is_empty(), "OpenType features"),
            (!s.variations.is_empty(), "variable axes"),
        ]
        .into_iter()
        .filter_map(|(set, what)| set.then_some(what))
        .collect();
        if !lost.is_empty() {
            self.notes.push(format!(
                "{name}: a typography token has no {}, so it is left out.",
                lost.join(", ")
            ));
        }
        json!({
            "$type": "typography",
            "$value": {
                "fontFamily": s.font_family,
                "fontSize": px(s.font_size),
                "fontWeight": s.weight,
                "letterSpacing": px(letter_spacing),
                "lineHeight": line_height,
            }
        })
    }

    /// [`Self::reference_to`] for a slot the format types `dimension` (a border's
    /// width, a shadow's offsets): a variable whose token is not a dimension is
    /// written as its value in px, noted.
    fn dimension_to(&mut self, var: VarId, style: &str) -> Option<Json> {
        let t = self.t;
        if let Some(target) = t.get(var)
            && self.paths.contains_key(&var)
            && self.type_of(target) != "dimension"
            && let Some(variable::Value::Number(n)) =
                self.value_mode(var).and_then(|m| t.resolve(var, m))
        {
            self.note(format!(
                "{style}: reads {}, a `number` token, so its value is written in px.",
                target.name
            ));
            return Some(px(n));
        }
        self.reference_to(var)
    }

    /// A reference to `var`'s token, or `None` (noted) when it has none.
    fn reference_to(&mut self, var: VarId) -> Option<Json> {
        match self.paths.get(&var) {
            Some(path) => Some(Json::String(reference(path))),
            None => {
                let name = self
                    .t
                    .get(var)
                    .map_or("a missing variable", |v| v.name.as_str());
                self.notes.push(format!(
                    "A style reads {name}, which is not exported (a kit's), so the style is left out."
                ));
                None
            }
        }
    }
}

/// How every number variable is read across `doc`, through aliases: what reads
/// an alias reads its target too.
fn number_reads(doc: &Document) -> BTreeMap<VarId, BTreeSet<Read>> {
    let mut reads: BTreeMap<VarId, BTreeSet<Read>> = BTreeMap::new();
    let mut add = |v: Option<VarId>, r: Read| {
        if let Some(v) = v {
            reads.entry(v).or_default().insert(r);
        }
    };
    let ids: Vec<_> = doc.pages().iter().map(|p| p.id).collect();
    for id in ondin_core::subtree_nodes(doc, &ids) {
        let Some(n) = doc.get(id) else { continue };
        for s in &n.paint().strokes {
            add(s.width_var, Read::Length);
        }
        for e in n.effects() {
            for v in e.vars {
                add(v, Read::Length);
            }
        }
        for v in n
            .item()
            .limit_vars
            .iter()
            .chain(&n.corner_vars())
            .chain(&n.spacing_vars())
            .chain(&n.size_vars())
        {
            add(*v, Read::Length);
        }
        add(n.opacity_var(), Read::Other);
        if let NodeKind::Text {
            style, paragraph, ..
        } = n.kind()
        {
            let tv = n.text_vars();
            let by_unit = |l: Length| match l {
                Length::Px(_) => Read::Length,
                Length::Em(_) => Read::Other,
            };
            add(tv[variable::TEXT_SIZE], Read::Length);
            add(tv[variable::TEXT_WEIGHT], Read::Weight);
            add(
                tv[variable::TEXT_LINE_HEIGHT],
                style.line_height.map_or(Read::Other, by_unit),
            );
            add(
                tv[variable::TEXT_LETTER_SPACING],
                by_unit(style.letter_spacing),
            );
            add(
                tv[variable::TEXT_PARAGRAPH_SPACING],
                by_unit(paragraph.spacing),
            );
        }
    }
    for s in doc.styles().iter() {
        match &s.value {
            StyleValue::Strokes(v) => {
                for k in v {
                    add(k.width_var, Read::Length);
                }
            }
            StyleValue::Effects(v) => {
                for e in v {
                    for var in e.vars {
                        add(var, Read::Length);
                    }
                }
            }
            StyleValue::Layout(l) => {
                for var in l.spacing_vars {
                    add(var, Read::Length);
                }
            }
            StyleValue::Fills(_) | StyleValue::Text(_) => {}
        }
    }
    // A scoped number reads as its scope says, whatever binds it now: what it
    // hands an unscoped alias or target is its type.
    let t = doc.variables();
    for v in t.iter() {
        if let Some(s) = v.scope.filter(|_| v.kind == VarKind::Number) {
            let mine: BTreeSet<Read> = s
                .fields()
                .map(|f| match f {
                    _ if f.is_length() => Read::Length,
                    variable::Field::Weight => Read::Weight,
                    _ => Read::Other,
                })
                .collect();
            reads.insert(v.id, mine);
        }
    }
    // An alias and its target are one type in the format, so each pair shares
    // its reads both ways, until nothing changes.
    loop {
        let mut changed = false;
        for v in t.iter() {
            for c in v.cells.values() {
                let Cell::Alias(to) = c else { continue };
                let union: BTreeSet<Read> = [v.id, *to]
                    .iter()
                    .filter_map(|id| reads.get(id))
                    .flatten()
                    .copied()
                    .collect();
                for id in [v.id, *to] {
                    let mine = reads.entry(id).or_default();
                    if mine.len() != union.len() {
                        mine.clone_from(&union);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    reads
}

/// A variable or style name as the segments of a token path: split at `/`,
/// trimmed, empty segments dropped, and the characters the format reserves
/// replaced — `.` (the reference separator) and the braces with `_`, a leading
/// `$` with `_`.
pub(crate) fn token_path(name: &str) -> Vec<String> {
    let mut out: Vec<String> = name
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let s = s.replace(['.', '{', '}'], "_");
            match s.strip_prefix('$') {
                Some(rest) => format!("_{rest}"),
                None => s,
            }
        })
        .collect();
    if out.is_empty() {
        out.push("_".to_string());
    }
    out
}

/// `{a.b.c}`, the format's reference to the token at `path`; a group's `$root`
/// token ends its path in `$root`, as the format requires.
fn reference(path: &[String]) -> String {
    format!("{{{}}}", path.join("."))
}

/// Put `token` at `path` under `root`, making groups on the way. Where a token
/// and a group want one name, the token becomes the group's `$root`; a second
/// token at one path is noted and dropped.
fn insert(
    root: &mut Map<String, Json>,
    path: &[String],
    token: Json,
    notes: &mut Vec<String>,
    label: &str,
) {
    let Some((last, groups)) = path.split_last() else {
        return;
    };
    let mut at = root;
    for g in groups {
        let slot = at
            .entry(g.clone())
            .or_insert_with(|| Json::Object(Map::new()));
        if is_token(slot) {
            // A token already holds the name: it moves down into `$root`.
            let held = std::mem::replace(slot, Json::Object(Map::new()));
            if let Json::Object(m) = slot {
                m.insert("$root".to_string(), held);
            }
        }
        let Json::Object(m) = slot else {
            unreachable!()
        };
        at = m;
    }
    match at.get_mut(last) {
        None => {
            at.insert(last.clone(), token);
        }
        Some(slot)
            if !is_token(slot) && !slot.as_object().is_some_and(|m| m.contains_key("$root")) =>
        {
            if let Json::Object(m) = slot {
                m.insert("$root".to_string(), token);
            }
        }
        Some(_) => notes.push(format!(
            "{label}: another one is written at {}, so this one is left out.",
            path.join(".")
        )),
    }
}

fn is_token(v: &Json) -> bool {
    v.as_object().is_some_and(|m| m.contains_key("$value"))
}

/// A colour as the format's colour value: sRGB components, alpha, and the hex
/// fallback it allows.
fn color(c: Color) -> Json {
    let [r, g, b, a] = c.components;
    let rgba = c.to_rgba8();
    json!({
        "colorSpace": "srgb",
        "components": [round(r), round(g), round(b)],
        "alpha": round(a),
        "hex": format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b),
    })
}

/// The DTCG type a number scoped to `scope` is written as: lengths only a
/// `dimension`, weight only a `fontWeight`, anything else — every field
/// included — a `number`.
pub fn scope_type(scope: Option<variable::Scope>) -> &'static str {
    let Some(s) = scope.filter(|s| !s.is_empty()) else {
        return "number";
    };
    if s.fields().all(variable::Field::is_length) {
        "dimension"
    } else if s.fields().all(|f| f == variable::Field::Weight) {
        "fontWeight"
    } else {
        "number"
    }
}

/// A number as a `$value` of type `ty`.
fn number(n: f64, ty: &str) -> Json {
    match ty {
        "dimension" => px(n),
        _ => json!(n),
    }
}

fn px(n: f64) -> Json {
    json!({ "value": n, "unit": "px" })
}

/// A colour component to six places: `f32` noise (`0.20000000298…`) is not a
/// value anybody set.
fn round(x: f32) -> f64 {
    (f64::from(x) * 1e6).round() / 1e6
}
