//! Variables as CSS custom properties: what a bound value is written as, and the
//! `tokens.css` that declares every variable the export reads.
//!
//! **Numbers are unitless in `tokens.css`; the unit is added where one is read**
//! — `calc(var(--x) * 1px)` for a length, `calc(var(--x) / 100)` for an opacity —
//! because one number variable may be bound to both, and an opacity reads its
//! number as a percentage.
//!
//! The default mode is declared on `:root` and on its own `[data-mode="…"]` (a
//! frame may set it inside one set to another), every other mode on its own,
//! **each block declaring every token, aliases included**: a custom property's
//! `var()` is substituted on the element that declares it, so an alias declared
//! only on `:root` keeps the default mode's value inside a `[data-mode]` subtree
//! (measured in Chrome).
//!
//! A collection picked by width is the window's instead: its default on `:root`,
//! each other mode in an `@media (min-width: …)` block, and each mode's
//! `[data-mode]` block after them all, so a frame pinned to a mode wins.

use super::dom::slug;
use crate::svg::fmt;
use ondin_core::Document;
use ondin_core::variable::{self, Cell, ModeId, VarId, Variables};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// The custom property names, the `data-mode` values, and which variables the
/// export has read so far.
pub(crate) struct Tokens {
    names: BTreeMap<VarId, String>,
    modes: BTreeMap<ModeId, String>,
    used: BTreeSet<VarId>,
}

impl Tokens {
    /// Names for every variable and mode in `doc`, fixed before the walk so a
    /// name never depends on what was read first.
    pub fn new(doc: &Document) -> Self {
        let t = doc.variables();
        let mut taken = HashSet::new();
        let mut unique = |base: String| {
            let base = if base.is_empty() {
                "token".to_string()
            } else {
                base
            };
            let mut name = base.clone();
            let mut n = 1;
            while !taken.insert(name.clone()) {
                n += 1;
                name = format!("{base}-{n}");
            }
            name
        };
        let kit_name = |kit: &str| {
            t.kits()
                .get(kit)
                .and_then(|k| k.name.clone())
                .unwrap_or_else(|| kit.to_string())
        };
        // The document's own first, so a kit's variable is the one suffixed when
        // the two slug alike.
        let mut names = BTreeMap::new();
        for v in t.local() {
            names.insert(v.id, unique(slug(&v.name)));
        }
        for v in t.iter().filter(|v| v.kit().is_some()) {
            let kit = kit_name(v.kit().unwrap_or_default());
            names.insert(v.id, unique(format!("{}-{}", slug(&kit), slug(&v.name))));
        }
        let mut taken = HashSet::new();
        let mut unique = |base: String| {
            let base = if base.is_empty() {
                "mode".to_string()
            } else {
                base
            };
            let mut name = base.clone();
            let mut n = 1;
            while !taken.insert(name.clone()) {
                n += 1;
                name = format!("{base}-{n}");
            }
            name
        };
        let mut modes = BTreeMap::new();
        for m in t.modes() {
            modes.insert(m.id, unique(slug(&m.name)));
        }
        // A word of `data-mode`, so never two words.
        for name in modes.values_mut() {
            *name = name.replace(' ', "-");
        }
        for (kit, km) in t.kits() {
            for m in &km.modes {
                modes.insert(
                    m.id,
                    unique(format!("{}-{}", slug(&kit_name(kit)), slug(&m.name))),
                );
            }
        }
        Tokens {
            names,
            modes,
            used: BTreeSet::new(),
        }
    }

    /// `var(--name)`, recording the variable as read.
    pub fn var(&mut self, id: VarId) -> String {
        self.used.insert(id);
        format!("var(--{})", self.name(id))
    }

    /// A number variable read as px.
    pub fn px(&mut self, id: VarId) -> String {
        format!("calc({} * 1px)", self.var(id))
    }

    /// A number variable read as px, floored at zero as the model floors it.
    pub fn px_floored(&mut self, id: VarId) -> String {
        format!("max(0px, {})", self.px(id))
    }

    /// A number variable read as an opacity: a percentage, which CSS clamps to
    /// `0..=1` as the model clamps it to `0..=100`.
    pub fn opacity(&mut self, id: VarId) -> String {
        format!("calc({} / 100)", self.var(id))
    }

    /// Whether string variable `id` is written bare, as CSS keywords: it is
    /// scoped to flex directions or wraps alone, so it holds only their words
    /// (`variable::holds_its_words`).
    pub fn keywords(&self, doc: &Document, id: VarId) -> bool {
        doc.variables().get(id).is_some_and(keywords)
    }

    /// A boolean variable read as a `display`: hidden where it is false, the
    /// layer's own display `own` where it is true.
    pub fn shown(&mut self, id: VarId, own: &str) -> String {
        self.used.insert(id);
        format!("var(--{}, {own})", self.name(id))
    }

    /// A colour variable times a fill's `%`.
    pub fn color(&mut self, id: VarId, alpha: f32) -> String {
        let v = self.var(id);
        if alpha >= 1.0 {
            v
        } else {
            format!(
                "color-mix(in srgb, {v} {}%, transparent)",
                fmt(f64::from(alpha.max(0.0)) * 100.0)
            )
        }
    }

    /// The `data-mode` value for `mode`.
    pub fn mode(&self, mode: ModeId) -> Option<&str> {
        self.modes.get(&mode).map(String::as_str)
    }

    /// The `data-mode` a frame setting `modes` writes: their names, one word
    /// each, `None` for none.
    pub fn modes_attr(&self, modes: impl IntoIterator<Item = ModeId>) -> Option<String> {
        let words: Vec<&str> = modes.into_iter().filter_map(|m| self.mode(m)).collect();
        (!words.is_empty()).then(|| words.join(" "))
    }

    fn name(&self, id: VarId) -> &str {
        self.names.get(&id).map_or("missing", String::as_str)
    }

    /// `tokens.css`, or `None` when the export read no variable.
    ///
    /// Every variable read and every alias target they reach in any mode,
    /// declared in each of the document's modes; a kit's variable in the column
    /// that mode maps to. A kit's own modes, which only a kit main's frame sets,
    /// get a block of that kit's variables.
    pub fn css(&self, doc: &Document) -> Option<String> {
        if self.used.is_empty() {
            return None;
        }
        let t = doc.variables();
        let shaken = self.reached(t);
        let mut out = String::new();
        // A frame names every mode it sets in one `data-mode`, so with two
        // collections a block matches one word of it.
        let eq = if t.has_collections() { "~=" } else { "=" };
        for c in t.collections() {
            // Picked by width: the default on `:root`, every other mode in a
            // media block of its minimum, then each mode's pin, after every
            // media block, so a frame set to a mode wins over the window.
            if c.by_width() && !c.modes.is_empty() {
                let default = c.modes[0].id;
                self.mode_block(t, &shaken, c, ":root".into(), Some(default), &mut out);
                for m in &c.modes[1..] {
                    let mut inner = String::new();
                    self.mode_block(t, &shaken, c, ":root".into(), Some(m.id), &mut inner);
                    let min = c.min_width(m.id).unwrap_or_default();
                    out.push_str(&format!("@media (min-width: {}px) {{\n", fmt(min)));
                    for line in inner.lines() {
                        out.push_str(&format!("  {line}\n"));
                    }
                    out.push_str("}\n");
                }
                for m in &c.modes {
                    let pin = format!("[data-mode{eq}\"{}\"]", self.mode(m.id).unwrap_or_default());
                    self.mode_block(t, &shaken, c, pin, Some(m.id), &mut out);
                }
                continue;
            }
            let modes: Vec<Option<ModeId>> = if c.modes.is_empty() {
                vec![None]
            } else {
                c.modes.iter().map(|m| Some(m.id)).collect()
            };
            // An empty collection after the first declares nothing.
            if c.modes.is_empty() && c.id != t.first_collection().id {
                continue;
            }
            for (i, mode) in modes.iter().enumerate() {
                // The default is a mode a frame can set too, inside one set to
                // another, so it matches `:root` and its name.
                let selector = match mode {
                    Some(m) if i > 0 => {
                        format!("[data-mode{eq}\"{}\"]", self.mode(*m).unwrap_or_default())
                    }
                    Some(m) => format!(
                        ":root, [data-mode{eq}\"{}\"]",
                        self.mode(*m).unwrap_or_default()
                    ),
                    None => ":root".to_string(),
                };
                self.mode_block(t, &shaken, c, selector, *mode, &mut out);
            }
        }
        for km in t.kits().values() {
            for m in &km.modes {
                let vars: Vec<VarId> = shaken
                    .iter()
                    .copied()
                    .filter(|id| t.get(*id).is_some_and(|v| v.cells.contains_key(&m.id)))
                    .filter(|id| t.get(*id).is_some_and(|v| v.kit().is_some()))
                    .collect();
                if vars.is_empty() {
                    continue;
                }
                out.push_str(&format!(
                    "[data-mode=\"{}\"] {{\n",
                    self.mode(m.id).unwrap_or_default()
                ));
                for id in vars {
                    if let Some(value) = t
                        .get(id)
                        .and_then(|v| Some((v, v.cells.get(&m.id)?)))
                        .and_then(|(v, c)| self.value(v, c))
                    {
                        out.push_str(&format!("  --{}: {value};\n", self.name(id)));
                    }
                }
                out.push_str("}\n");
            }
        }
        Some(out)
    }

    /// One mode's block of collection `c` under `selector`: its variables, and a
    /// kit's that map from it, in the column that mode maps to.
    fn mode_block(
        &self,
        t: &Variables,
        shaken: &BTreeSet<VarId>,
        c: &variable::Collection,
        selector: String,
        mode: Option<ModeId>,
        out: &mut String,
    ) {
        out.push_str(&selector);
        out.push_str(" {\n");
        for id in shaken {
            let Some(v) = t.get(*id) else { continue };
            let col = match v.kit() {
                Some(kit) => {
                    let kc = t.kit_collection_of(v).unwrap_or_default();
                    if t.kit_onto(kc).is_none_or(|o| o.id != c.id) {
                        continue;
                    }
                    t.kit_mode_at(kit, kc, mode.as_slice())
                }
                None if t.collection_of(*id).is_some_and(|x| x.id == c.id) => mode,
                None => continue,
            };
            if let Some(value) = col
                .and_then(|c| v.cells.get(&c))
                .and_then(|c| self.value(v, c))
            {
                out.push_str(&format!("  --{}: {value};\n", self.name(*id)));
            }
        }
        out.push_str("}\n");
    }

    /// The variables read and every alias target they reach, in any column.
    fn reached(&self, t: &Variables) -> BTreeSet<VarId> {
        let mut out = BTreeSet::new();
        let mut stack: Vec<VarId> = self.used.iter().copied().collect();
        while let Some(id) = stack.pop() {
            if !out.insert(id) {
                continue;
            }
            if let Some(v) = t.get(id) {
                for c in v.cells.values() {
                    if let Cell::Alias(next) = c {
                        stack.push(*next);
                    }
                }
            }
        }
        out
    }

    /// One cell of variable `v` as a custom property's value. A boolean is a
    /// visibility: `none` hides, and `initial` — the guaranteed-invalid value —
    /// makes [`Self::shown`]'s fallback, the layer's own `display`, win. A
    /// string of flex words is their CSS keyword, bare; any other is quoted.
    fn value(&self, v: &variable::Variable, cell: &Cell) -> Option<String> {
        match cell {
            Cell::Color(c) => Some(super::css::color(*c, 1.0)),
            Cell::Number(n) => Some(fmt(*n)),
            Cell::String(s) if keywords(v) => Some(keyword(s)),
            Cell::String(s) => Some(format!(
                "\"{}\"",
                s.replace('\\', "\\\\").replace('"', "\\\"")
            )),
            Cell::Boolean(b) => Some(if *b { "initial" } else { "none" }.into()),
            Cell::Alias(to) => Some(format!("var(--{})", self.name(*to))),
        }
    }
}

/// [`Tokens::keywords`] of a variable in hand.
fn keywords(v: &variable::Variable) -> bool {
    v.kind == variable::VarKind::String
        && v.scope
            .is_some_and(|s| !s.is_empty() && s.fields().all(variable::Field::needs_scope))
}

/// A word a keyword string holds as CSS writes it: a direction's or a wrap's
/// keyword, else the word itself.
fn keyword(s: &str) -> String {
    use ondin_core::container::{FlexDirection, FlexWrap};
    FlexDirection::from_word(s)
        .map(FlexDirection::css)
        .or_else(|| FlexWrap::from_word(s).map(FlexWrap::css))
        .unwrap_or(s)
        .to_string()
}

/// Whether `var` takes more than one value across the document's modes — the
/// question for a binding written resolved, which then holds in one mode only.
pub(crate) fn varies(doc: &Document, var: VarId) -> bool {
    doc.variables().every_value(var).len() > 1
}
