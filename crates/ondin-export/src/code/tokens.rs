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
//! (measured in Chrome). An alias of another collection reaching a block's
//! collection is redeclared there too when it names one target in every mode.
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
    /// Every custom property name given out, so a hidden token takes a free one.
    taken: HashSet<String>,
    /// The hidden tokens [`Self::shown_in`] made: each one's variable, name,
    /// collection, and the modes it shows in.
    hidden: Vec<(VarId, String, variable::CollectionId, Vec<ModeId>)>,
    /// The width queries [`Self::query_attr`] named: each `data-query` word,
    /// whose modes it picks and its widths — one container query apiece.
    queries: Vec<(String, QueryOf, variable::Breakpoints)>,
}

/// Whose modes a width query picks: a collection of the document's, or a kit's
/// collection, by kit id and collection name.
#[derive(Clone, PartialEq)]
enum QueryOf {
    Collection(variable::CollectionId),
    Kit(String, String),
}

/// A kit's name for the export's words: its recorded name, else its id.
fn kit_name(t: &Variables, kit: &str) -> String {
    t.kits()
        .get(kit)
        .and_then(|k| k.name.clone())
        .unwrap_or_else(|| kit.to_string())
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
        let kit_name = |kit: &str| kit_name(t, kit);
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
        let names_taken: HashSet<String> = names.values().cloned().collect();
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
            taken: names_taken,
            hidden: Vec::new(),
            queries: Vec::new(),
        }
    }

    /// The `data-query` a frame with width `queries` writes, `None` for none:
    /// one word per query, named after `scope` (the component it belongs to) and
    /// the collection — a kit's by its collection's name, or the kit's for its
    /// only one — and shared by every frame querying that collection at the
    /// same widths.
    pub fn query_attr(
        &mut self,
        t: &Variables,
        scope: &str,
        queries: &[variable::Breakpoints],
    ) -> Option<String> {
        let mut words = Vec::new();
        for q in queries {
            let Some((of, name)) = q.keys().find_map(|m| match t.owner_of(*m)? {
                variable::ModeOwner::Collection(c) => {
                    Some((QueryOf::Collection(c), t.collection(c)?.name.clone()))
                }
                variable::ModeOwner::Kit(k, c) => Some((
                    QueryOf::Kit(k.to_string(), c.to_string()),
                    if c.is_empty() {
                        kit_name(t, k)
                    } else {
                        c.to_string()
                    },
                )),
            }) else {
                continue;
            };
            if let Some((word, ..)) = self
                .queries
                .iter()
                .find(|(_, at, widths)| *at == of && widths == q)
            {
                words.push(word.clone());
                continue;
            }
            let base = format!("{}-{}", slug(scope), slug(&name)).replace(' ', "-");
            let mut word = base.clone();
            let mut n = 1;
            while self.queries.iter().any(|(w, ..)| *w == word) {
                n += 1;
                word = format!("{base}-{n}");
            }
            self.queries.push((word.clone(), of, q.clone()));
            words.push(word);
        }
        (!words.is_empty()).then(|| words.join(" "))
    }

    /// Each width query's blocks (§5.3f, CSS's container query): the frame a
    /// container of its inline size, and its children — never itself, which a
    /// container cannot query — given its narrowest mode, then each other mode
    /// under `@container (min-width: …)`, then each mode pinned on the frame or
    /// on a component use's mode wrapper (`data-pin`), after them all so that
    /// pin wins — a pin on any other element around loses, as on the canvas,
    /// where a frame's width beats what it inherits. Every block is `:where()`,
    /// of no weight, so source order alone ranks them and a child pinning a mode
    /// of its own keeps it, the nearer pin winning as on the canvas.
    fn query_blocks(&self, t: &Variables, shaken: &BTreeSet<VarId>, eq: &str, out: &mut String) {
        // Kit queries after the document's, so a kit's own pick beats the
        // mapping from a document query on the same frame, as on the canvas.
        let (kits, ours): (Vec<_>, Vec<_>) = self
            .queries
            .iter()
            .partition(|(_, of, _)| matches!(of, QueryOf::Kit(..)));
        for (word, of, widths) in ours.into_iter().chain(kits) {
            let (col, modes): (Option<&variable::Collection>, Vec<ModeId>) = match of {
                QueryOf::Collection(c) => {
                    let Some(col) = t.collection(*c) else {
                        continue;
                    };
                    (Some(col), col.modes.iter().map(|m| m.id).collect())
                }
                QueryOf::Kit(k, kc) => {
                    let Some(km) = t.kits().get(k) else {
                        continue;
                    };
                    (None, km.modes_in(kc).map(|m| m.id).collect())
                }
            };
            let block = |selector: String, m: ModeId, out: &mut String| match col {
                Some(col) => self.mode_block(t, shaken, col, selector, Some(m), out),
                None => self.kit_block(t, shaken, selector, m, out),
            };
            let at = format!("[data-query~=\"{word}\"]");
            out.push_str(&format!("{at} {{\n  container-type: inline-size;\n}}\n"));
            let kids = format!(":where({at}) > *");
            let mut held: Vec<(ModeId, f64)> = modes
                .iter()
                .filter_map(|m| Some((*m, *widths.get(m)?)))
                .collect();
            held.sort_by(|a, b| a.1.total_cmp(&b.1));
            for (i, (m, min)) in held.iter().enumerate() {
                if i == 0 {
                    block(kids.clone(), *m, out);
                    continue;
                }
                let mut inner = String::new();
                block(kids.clone(), *m, &mut inner);
                if inner.is_empty() {
                    continue;
                }
                out.push_str(&format!("@container (min-width: {}px) {{\n", fmt(*min)));
                for line in inner.lines() {
                    out.push_str(&format!("  {line}\n"));
                }
                out.push_str("}\n");
            }
            // A kit mode may share `data-mode` with a document mode, so `~=`.
            let eq = if col.is_some() { eq } else { "~=" };
            for m in &modes {
                let word = self.mode(*m).unwrap_or_default();
                let pin = format!("[data-mode{eq}\"{word}\"]");
                let own = format!("[data-pin~=\"{word}\"]");
                let selector = format!(":where({own} > {at}) > *, :where({pin}{at}) > *");
                block(selector, *m, out);
            }
        }
    }

    /// Kit mode `mode`'s block under `selector`: every kit variable read with a
    /// value in that mode, and every alias of the document's reaching one,
    /// redeclared as `mode_block` redeclares them. Nothing for none.
    fn kit_block(
        &self,
        t: &Variables,
        shaken: &BTreeSet<VarId>,
        selector: String,
        mode: ModeId,
        out: &mut String,
    ) {
        let in_mode = |v: &variable::Variable| v.kit().is_some() && v.cells.contains_key(&mode);
        let mut body = String::new();
        for id in shaken {
            let Some(v) = t.get(*id) else { continue };
            if v.kit().is_none() {
                if let Some(to) = one_alias(t, v).filter(|to| reaches_by(t, *to, in_mode)) {
                    body.push_str(&format!(
                        "  --{}: var(--{});\n",
                        self.name(*id),
                        self.name(to)
                    ));
                }
                continue;
            }
            if let Some(value) = v.cells.get(&mode).and_then(|c| self.value(t, v, c)) {
                body.push_str(&format!("  --{}: {value};\n", self.name(*id)));
            }
        }
        if !body.is_empty() {
            out.push_str(&format!("{selector} {{\n{body}}}\n"));
        }
    }

    /// A hidden token read as a `display`: the element's own display `own` in
    /// `modes` of `var`'s collection, hidden in its others — what shows one
    /// mode's text of a content that varies by mode. Named after `var` and the
    /// first of `modes`, and shared by every text of that variable.
    pub fn shown_in(
        &mut self,
        t: &Variables,
        var: VarId,
        modes: Vec<ModeId>,
        own: &str,
    ) -> Option<String> {
        if let Some((_, name, _, _)) = self
            .hidden
            .iter()
            .find(|(v, _, _, m)| *v == var && *m == modes)
        {
            return Some(format!("var(--{name}, {own})"));
        }
        let c = t.collection_of(var)?.id;
        let mode = modes.first().and_then(|m| self.mode(*m)).unwrap_or("mode");
        let base = format!("{}-{mode}", self.name(var));
        let mut name = base.clone();
        let mut n = 1;
        while !self.taken.insert(name.clone()) {
            n += 1;
            name = format!("{base}-{n}");
        }
        let out = format!("var(--{name}, {own})");
        self.hidden.push((var, name, c, modes));
        Some(out)
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
        if self.used.is_empty() && self.hidden.is_empty() {
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
        // After every collection's blocks, so a container's pick beats the
        // modes its children inherit from around it.
        self.query_blocks(t, &shaken, eq, &mut out);
        for km in t.kits().values() {
            // `~=`: a frame setting a kit mode may name a document mode beside it.
            for m in &km.modes {
                let selector = format!("[data-mode~=\"{}\"]", self.mode(m.id).unwrap_or_default());
                self.kit_block(t, &shaken, selector, m.id, &mut out);
            }
        }
        Some(out)
    }

    /// One mode's block of collection `c` under `selector`: its variables, other
    /// collections' aliases reaching it, and a kit's that map from it, in the
    /// column that mode maps to.
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
                // An alias into this collection is redeclared here too, or the
                // root's substitution would stand.
                None => match one_alias(t, v) {
                    Some(to) if reaches(t, to, c.id) => {
                        out.push_str(&format!(
                            "  --{}: var(--{});\n",
                            self.name(*id),
                            self.name(to)
                        ));
                        continue;
                    }
                    _ => continue,
                },
            };
            if let Some(value) = col
                .and_then(|c| v.cells.get(&c))
                .and_then(|c| self.value(t, v, c))
            {
                out.push_str(&format!("  --{}: {value};\n", self.name(*id)));
            }
        }
        // `initial` lets the fallback, the element's own display, win.
        for (_, name, of, modes) in &self.hidden {
            if let Some(m) = mode.filter(|_| *of == c.id) {
                let value = if modes.contains(&m) {
                    "initial"
                } else {
                    "none"
                };
                out.push_str(&format!("  --{name}: {value};\n"));
            }
        }
        out.push_str("}\n");
    }

    /// The variables read that alias into another collection of several modes
    /// by a different target per mode of their own: no block of that collection
    /// can redeclare them, so a frame setting its mode reads the root's value.
    pub fn unfollowed<'a>(&self, doc: &'a Document) -> Vec<&'a str> {
        let t = doc.variables();
        let mut out = Vec::new();
        for id in self.reached(t) {
            let Some(v) = t.get(id).filter(|v| v.kit().is_none()) else {
                continue;
            };
            let Some(own) = t.collection_of(id).map(|c| c.id) else {
                continue;
            };
            if one_alias(t, v).is_some() {
                continue;
            }
            let into_other = v.cells.values().any(|cell| match cell {
                Cell::Alias(to) => t
                    .collections()
                    .iter()
                    .any(|c| c.id != own && c.modes.len() > 1 && reaches(t, *to, c.id)),
                _ => false,
            });
            if into_other {
                out.push(v.name.as_str());
            }
        }
        out
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
    /// string of flex words is their CSS keyword, bare, and so is one a keyword
    /// string aliases, which `flex-direction` reads through it; any other is
    /// quoted.
    fn value(&self, t: &Variables, v: &variable::Variable, cell: &Cell) -> Option<String> {
        match cell {
            Cell::Color(c) => Some(super::css::color(*c, 1.0)),
            Cell::Number(n) => Some(fmt(*n)),
            Cell::String(s) if keywords(v) || self.aliased_by_keywords(t, v.id) => Some(keyword(s)),
            Cell::String(s) => Some(format!(
                "\"{}\"",
                s.replace('\\', "\\\\").replace('"', "\\\"")
            )),
            Cell::Boolean(b) => Some(if *b { "initial" } else { "none" }.into()),
            Cell::Alias(to) => Some(format!("var(--{})", self.name(*to))),
        }
    }

    /// Whether `id` is reached through aliases from a keyword string the export
    /// reads ([`keywords`]).
    fn aliased_by_keywords(&self, t: &Variables, id: VarId) -> bool {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<VarId> = self
            .used
            .iter()
            .copied()
            .filter(|u| t.get(*u).is_some_and(keywords))
            .collect();
        while let Some(at) = stack.pop() {
            if !seen.insert(at) {
                continue;
            }
            for c in t.get(at).into_iter().flat_map(|v| v.cells.values()) {
                if let Cell::Alias(next) = c {
                    if *next == id {
                        return true;
                    }
                    stack.push(*next);
                }
            }
        }
        false
    }
}

/// The variable a document variable aliases in every mode of its collection,
/// or `None` when its modes differ or one holds a value.
fn one_alias(t: &Variables, v: &variable::Variable) -> Option<VarId> {
    let modes = &t.collection_of(v.id)?.modes;
    let mut to = None;
    for m in modes {
        match v.cells.get(&m.id) {
            Some(Cell::Alias(x)) if to.is_none_or(|y| y == *x) => to = Some(*x),
            _ => return None,
        }
    }
    to
}

/// Whether `from`, or a variable it aliases in any mode, is in collection `c`.
fn reaches(t: &Variables, from: VarId, c: variable::CollectionId) -> bool {
    reaches_by(t, from, |v| {
        v.kit().is_none() && t.collection_of(v.id).is_some_and(|x| x.id == c)
    })
}

/// Whether `from`, or a variable it aliases in any mode, is one `hit` takes.
fn reaches_by(t: &Variables, from: VarId, hit: impl Fn(&variable::Variable) -> bool) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(at) = stack.pop() {
        if !seen.insert(at) {
            continue;
        }
        let Some(v) = t.get(at) else { continue };
        if hit(v) {
            return true;
        }
        for cell in v.cells.values() {
            if let Cell::Alias(next) = cell {
                stack.push(*next);
            }
        }
    }
    false
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

/// String variable `var`'s values, each with the modes of its collection that
/// hold it, in mode order — `None` for a kit's variable or one with an alias,
/// whose value another collection's mode can decide.
pub(crate) fn strings_by_mode(doc: &Document, var: VarId) -> Option<Vec<(String, Vec<ModeId>)>> {
    let t = doc.variables();
    let v = t.get(var).filter(|v| v.kit().is_none())?;
    let mut out: Vec<(String, Vec<ModeId>)> = Vec::new();
    for m in &t.collection_of(var)?.modes {
        let Some(Cell::String(s)) = v.cells.get(&m.id) else {
            return None;
        };
        match out.iter_mut().find(|(x, _)| x == s) {
            Some((_, modes)) => modes.push(m.id),
            None => out.push((s.clone(), vec![m.id])),
        }
    }
    Some(out)
}
