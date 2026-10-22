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

use super::dom::slug;
use crate::svg::fmt;
use ondin_core::Document;
use ondin_core::variable::{Cell, ModeId, VarId, Variables};
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
        let doc_modes: Vec<Option<ModeId>> = if t.modes().is_empty() {
            vec![None]
        } else {
            t.modes().iter().map(|m| Some(m.id)).collect()
        };
        for (i, mode) in doc_modes.iter().enumerate() {
            // The default is a mode a frame can set too, inside one set to another.
            let selector = match mode {
                Some(m) if i > 0 => {
                    format!("[data-mode=\"{}\"]", self.mode(*m).unwrap_or_default())
                }
                Some(m) => format!(
                    ":root, [data-mode=\"{}\"]",
                    self.mode(*m).unwrap_or_default()
                ),
                None => ":root".to_string(),
            };
            out.push_str(&selector);
            out.push_str(" {\n");
            for id in &shaken {
                let Some(v) = t.get(*id) else { continue };
                let col = match v.kit() {
                    Some(kit) => t.kit_mode(kit, *mode),
                    None => *mode,
                };
                if let Some(value) = col
                    .and_then(|c| v.cells.get(&c))
                    .and_then(|c| self.value(c))
                {
                    out.push_str(&format!("  --{}: {value};\n", self.name(*id)));
                }
            }
            out.push_str("}\n");
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
                        .and_then(|v| v.cells.get(&m.id))
                        .and_then(|c| self.value(c))
                    {
                        out.push_str(&format!("  --{}: {value};\n", self.name(id)));
                    }
                }
                out.push_str("}\n");
            }
        }
        Some(out)
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

    /// One cell as a custom property's value. Booleans are never read through a
    /// token: they are written resolved.
    fn value(&self, cell: &Cell) -> Option<String> {
        match cell {
            Cell::Color(c) => Some(super::css::color(*c, 1.0)),
            Cell::Number(n) => Some(fmt(*n)),
            Cell::String(s) => Some(format!(
                "\"{}\"",
                s.replace('\\', "\\\\").replace('"', "\\\"")
            )),
            Cell::Boolean(_) => None,
            Cell::Alias(to) => Some(format!("var(--{})", self.name(*to))),
        }
    }
}

/// Whether `var` takes more than one value across the document's modes — the
/// question for a binding written resolved, which then holds in one mode only.
pub(crate) fn varies(doc: &Document, var: VarId) -> bool {
    doc.variables().every_value(var).len() > 1
}
