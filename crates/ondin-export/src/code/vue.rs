//! The Vue emitter: single-file components in TypeScript with scoped styles.
//!
//! `App.vue` is the page; each main an instance on it uses, or the set it is a
//! variant of, becomes `components/<Name>.vue` (a kit's in `components/<Kit>/`):
//! its variant properties string
//! unions choosing a variant's tree by `v-if`, its boolean and text properties
//! typed props, each defaulting to the default variant's value. A class on a
//! component falls through to its root, and the user's scoped rule reaches that
//! root, so the user places it. `base.css` holds the page reset every file
//! imports, and `tokens.css` the variables.

use super::dom::{Arg, Child, Element, Syntax, condition, escape};
use super::jsx::js_string;
use super::walk::{self, Component, Ty};
use super::{BASE_CSS, CodeExport, CodeFile};
use ondin_core::{Document, NodeId, Resolved};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// `origins` as `App`, in the order given (later on top).
pub fn vue_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> CodeExport {
    let page = walk::page(doc, res, origins, true);
    let tokens = page.tokens.is_some();
    let app = Component::page(page.root.clone());
    let all: Vec<&Component> = std::iter::once(&app).chain(&page.components).collect();

    // A used component's root carries its user's scope as well as its own, and
    // its user's class as well as its own, so each file's classes are settled
    // against the files it uses in two passes.
    let roots: HashMap<&str, Vec<&str>> = all
        .iter()
        .map(|c| {
            let classes = c
                .bodies
                .iter()
                .flat_map(|(_, e)| e.classes())
                .map(String::as_str);
            let classes = classes.collect();
            (c.name.as_str(), classes)
        })
        .collect();
    let mut trees: Vec<Vec<Element>> = all
        .iter()
        .map(|c| {
            let mut bodies: Vec<Element> = c.bodies.iter().map(|(_, e)| e.clone()).collect();
            let (uses, _) = super::references(bodies.iter());
            let taken: HashSet<String> = uses
                .iter()
                .filter_map(|u| roots.get(u.as_str()))
                .flatten()
                .map(|c| c.to_string())
                .collect();
            avoid(&mut bodies, &taken);
            bodies
        })
        .collect();
    let classes: HashMap<String, HashSet<String>> = all
        .iter()
        .zip(&trees)
        .map(|(c, bodies)| (c.name.clone(), classes_in(bodies)))
        .collect();
    // A slot may draw any component, so its class keeps off all of theirs.
    let any: HashSet<String> = classes
        .iter()
        .filter(|(name, _)| *name != "App")
        .flat_map(|(_, c)| c.iter().cloned())
        .collect();
    for bodies in &mut trees {
        let mut mine = classes_in(bodies);
        for body in bodies.iter_mut() {
            keep_uses_apart(body, &classes, &any, &roots, &mut mine);
        }
    }

    let dirs = super::dirs(&page.components);
    let mut files = Vec::new();
    for (i, (c, bodies)) in all.iter().zip(trees).enumerate() {
        let path = match i {
            0 => "App.vue".to_string(),
            _ => format!("{}/{}.vue", c.dir, c.name),
        };
        files.push(CodeFile {
            path,
            bytes: sfc(c, bodies, tokens, &dirs).into_bytes(),
        });
        if i == 0 {
            files.push(CodeFile {
                path: "base.css".into(),
                bytes: BASE_CSS.as_bytes().to_vec(),
            });
        }
    }
    super::finish(files, page)
}

/// Every class in a file's trees.
fn classes_in(bodies: &[Element]) -> HashSet<String> {
    let mut all = HashSet::new();
    for body in bodies {
        body.each(&mut |e| all.extend(e.classes().cloned()));
    }
    all
}

/// Renames each use in `el`'s tree whose class is also a class of the file it
/// uses, where that file's own rule would reach the root the class lands on.
/// Its root's own class is harmless on a component of one tree, which wears it
/// anyway. A slot's class keeps off `any`, every component's classes. `mine` is
/// the file's classes, which a new name joins.
fn keep_uses_apart(
    el: &mut Element,
    classes: &HashMap<String, HashSet<String>>,
    any: &HashSet<String>,
    roots: &HashMap<&str, Vec<&str>>,
    mine: &mut HashSet<String>,
) {
    if let Some(i) = el.instance.clone() {
        let theirs = match i.slot {
            Some(_) => Some(any),
            None => classes.get(&i.component),
        };
        for class in el.classes_mut() {
            let own_root = i.slot.is_none()
                && roots
                    .get(i.component.as_str())
                    .is_some_and(|r| r.len() == 1 && r[0] == class);
            if let Some(theirs) = theirs.filter(|t| !own_root && t.contains(class.as_str())) {
                let base = class.clone();
                let mut n = 2;
                while mine.contains(&format!("{base}-{n}"))
                    || theirs.contains(&format!("{base}-{n}"))
                {
                    n += 1;
                }
                *class = format!("{base}-{n}");
                mine.insert(class.clone());
            }
        }
    }
    for c in &mut el.children {
        if let Child::Element(e) = c {
            keep_uses_apart(e, classes, any, roots, mine);
        }
    }
}

/// One `.vue` file from `c` and its trees, their classes settled. `dirs` holds
/// each component's folder, which its imports are written to.
fn sfc(
    c: &Component,
    mut bodies: Vec<Element>,
    tokens: bool,
    dirs: &HashMap<&str, &str>,
) -> String {
    let up = &super::up(&c.dir);
    let (uses, _) = super::references(bodies.iter());
    let uses = super::with_defaults(uses, &c.props);
    // A chain of `v-if`s is one root to Vue, so a class still falls through. The
    // default variant's tree is last, with no condition.
    for (i, (body, (when, _))) in bodies.iter_mut().zip(&c.bodies).enumerate() {
        if when.is_empty() {
            if i > 0 {
                body.branch = Some("v-else".into());
            }
            continue;
        }
        let cond = condition(when, Syntax::Vue);
        let directive = if i == 0 { "v-if" } else { "v-else-if" };
        body.branch = Some(format!("{directive}=\"{}\"", escape(&cond)));
    }

    let mut out = String::from("<script setup lang=\"ts\">\n");
    if c.props.iter().any(|(_, ty, _)| *ty == Ty::Component) {
        out.push_str("import type { Component } from \"vue\";\n");
    }
    let _ = writeln!(out, "import \"{up}base.css\";");
    if tokens {
        let _ = writeln!(out, "import \"{up}tokens.css\";");
    }
    for u in &uses {
        let at = dirs.get(u.as_str()).copied().unwrap_or("components");
        let from = super::relative(&c.dir, &format!("{at}/{u}.vue"));
        let _ = writeln!(out, "import {u} from \"{from}\";");
    }
    if !c.props.is_empty() {
        out.push_str("\nwithDefaults(\n  defineProps<{\n");
        for (p, ty, _) in &c.props {
            let ty = match ty {
                Ty::Bool => "boolean".to_string(),
                Ty::Text => "string".to_string(),
                Ty::Union(values) => {
                    let union: Vec<String> = values.iter().map(|v| js_string(v)).collect();
                    union.join(" | ")
                }
                Ty::Component => "Component".to_string(),
            };
            let _ = writeln!(out, "    {p}?: {ty};");
        }
        out.push_str("  }>(),\n  {\n");
        for (p, _, d) in &c.props {
            let v = match d {
                Some(Arg::Bool(b)) => b.to_string(),
                Some(Arg::Text(t)) => js_string(t),
                // An object prop's default is returned by a function.
                Some(Arg::Component(c)) => format!("() => {c}"),
                // Left unset, so the slot's own value stands in; a boolean with
                // no default would be cast to `false` instead.
                Some(Arg::Var { .. }) | None => "undefined".to_string(),
            };
            let _ = writeln!(out, "    {p}: {v},");
        }
        out.push_str("  },\n);\n");
    }
    out.push_str("</script>\n\n<template>\n");
    for body in &mut bodies {
        relink_images(body, up);
        body.markup(&mut out, 1, Syntax::Vue);
    }
    out.push_str("</template>\n\n<style scoped>\n");
    for body in &bodies {
        out.push_str(&super::stylesheet(body, up));
    }
    out.push_str("</style>\n");
    out
}

/// Renames every class in a file's trees that is also the root class of a
/// component the file uses.
///
/// A child component's root carries its user's scope too, so a user's rule for
/// one of its own elements would reach a child root of the same class. Rules are
/// printed from the tree, so renaming the element renames its rule.
fn avoid(bodies: &mut [Element], taken: &HashSet<String>) {
    if taken.is_empty() {
        return;
    }
    let mut all = classes_in(bodies);
    all.extend(taken.iter().cloned());
    for body in bodies {
        rename(body, taken, &mut all);
    }
}

fn rename(el: &mut Element, taken: &HashSet<String>, all: &mut HashSet<String>) {
    if el.instance.is_none() {
        for class in el.classes_mut().filter(|c| taken.contains(c.as_str())) {
            let base = class.clone();
            let mut n = 2;
            while all.contains(&format!("{base}-{n}")) {
                n += 1;
            }
            *class = format!("{base}-{n}");
            all.insert(class.clone());
        }
    }
    for c in &mut el.children {
        if let Child::Element(e) = c {
            rename(e, taken, all);
        }
    }
}

/// An `img`'s `src` written from the file, which Vue's compiler turns into an
/// import only when it starts with `.`.
fn relink_images(el: &mut Element, up: &str) {
    if el.tag == "img" {
        for (k, v) in &mut el.attrs {
            if *k == "src" {
                v.insert_str(0, up);
            }
        }
    }
    for c in &mut el.children {
        if let Child::Element(e) = c {
            relink_images(e, up);
        }
    }
}
