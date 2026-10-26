//! Document diff — what changed between two states of one document, for
//! `ondin diff` (export.md §7) and a version compare.
//!
//! Layers, pages, variables, modes, styles and list items are matched by **id**,
//! which survives a save and a load, so a renamed layer is a rename rather than a
//! delete and a create. Values are compared through the saved form (`io::schema`):
//! every field a file carries is compared, and a field added to the model is
//! reported without being listed here. Structure — pages, parents, sibling order —
//! is read from the documents.
//!
//! A number counts as changed at [`EPSILON`] or more, and a colour when its hex or
//! alpha reads differently: below that is float noise, not an edit. A layer inside
//! an instance is named under its instance, and a change it shares with the layer
//! it was copied from is reported once, on that layer.

use crate::document::Document;
use crate::id::NodeId;
use crate::io::schema::DocumentDto;
use crate::node::Node;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use serde_json::{Map, Value};

/// The smallest difference a number must show to count as changed, in the
/// number's own unit — a pixel for a position or a size.
pub const EPSILON: f64 = 0.01;

/// How long a value may print before it is cut with an ellipsis.
const SHOWN_CHARS: usize = 60;

/// Whether a page, a layer or an item is new, gone, or in both and changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Added,
    Removed,
    Changed,
}

/// One value that differs, written as the inspector shows it. `before` is `None`
/// for something added, `after` for something removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    pub what: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

impl Change {
    fn new(what: impl Into<String>, before: Option<String>, after: Option<String>) -> Self {
        Self {
            what: what.into(),
            before,
            after,
        }
    }

    /// The change as one line of `ondin diff`'s text.
    pub fn line(&self) -> String {
        match (&self.before, &self.after) {
            (Some(b), Some(a)) => format!("{}: {b} → {a}", self.what),
            (None, Some(a)) => format!("{} added: {a}", self.what),
            (Some(b), None) => format!("{} removed: {b}", self.what),
            (None, None) => self.what.clone(),
        }
    }
}

/// One layer that differs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LayerDiff {
    /// The layer's id in its saved form.
    pub id: String,
    /// The names from the page's top level down to the layer; an instance's own
    /// name carries ` (instance)`, so a layer inside one reads under it.
    pub trail: Vec<String>,
    /// The kind, as a new layer of it is named ("Rectangle").
    pub kind: String,
    pub status: Status,
    /// For an added or removed layer, how many layers inside it came or went with it.
    #[serde(skip_serializing_if = "is_zero")]
    pub inside: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
}

/// One page that differs: its own changes, then its layers' in layer order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PageDiff {
    pub id: String,
    pub name: String,
    pub status: Status,
    /// For an added or removed page, how many layers it holds.
    #[serde(skip_serializing_if = "is_zero")]
    pub inside: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerDiff>,
}

/// Everything that differs between two documents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diff {
    /// The pages that differ, in the second document's order with removed pages
    /// last, and the kit vault under its own name when its layers differ.
    pub pages: Vec<PageDiff>,
    /// What belongs to no page: variables, modes, styles, guides, images, the
    /// library block and a kit's publish.
    pub document: Vec<Change>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty() && self.document.is_empty()
    }

    /// The diff as `ondin diff` prints it: a block per page, a line per layer
    /// (`+` added, `-` removed, `~` changed) and its changes indented below.
    pub fn text(&self) -> String {
        let mut s = String::new();
        let mut line = |indent: usize, text: String| {
            s.push_str(&" ".repeat(indent));
            s.push_str(&text);
            s.push('\n');
        };
        for page in &self.pages {
            line(
                0,
                match page.status {
                    Status::Added => {
                        format!("+ page “{}”{}", page.name, layers_inside(page.inside))
                    }
                    Status::Removed => {
                        format!("- page “{}”{}", page.name, layers_inside(page.inside))
                    }
                    Status::Changed => format!("page “{}”", page.name),
                },
            );
            for c in &page.changes {
                line(4, c.line());
            }
            for layer in &page.layers {
                let trail = layer.trail.join(" › ");
                line(
                    2,
                    match layer.status {
                        Status::Added => {
                            format!("+ {trail} ({}{})", layer.kind, and_inside(layer.inside))
                        }
                        Status::Removed => {
                            format!("- {trail} ({}{})", layer.kind, and_inside(layer.inside))
                        }
                        Status::Changed => format!("~ {trail}"),
                    },
                );
                for c in &layer.changes {
                    line(6, c.line());
                }
            }
        }
        if !self.document.is_empty() {
            line(0, "document".into());
            for c in &self.document {
                line(4, c.line());
            }
        }
        s
    }
}

fn layers_inside(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => " (1 layer)".into(),
        n => format!(" ({n} layers)"),
    }
}

fn and_inside(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => ", and 1 layer inside".into(),
        n => format!(", and {n} layers inside"),
    }
}

/// What differs between `a`, the earlier state, and `b`, the later.
pub fn diff(a: &Document, b: &Document) -> Diff {
    let d = Differ::new(a, b);
    let changed = d.layer_changes();
    let mut pages = Vec::new();
    let order = moved_in_order(
        &a.pages().iter().map(|p| p.id).collect::<Vec<_>>(),
        &b.pages().iter().map(|p| p.id).collect::<Vec<_>>(),
    );
    for (index, page) in b.pages().iter().enumerate() {
        let name = d.name_in(b, page.id);
        let Some(old) = a.page(page.id) else {
            pages.push(PageDiff {
                id: page.id.to_wire(),
                name,
                status: Status::Added,
                inside: d.count_new(b, a, page.id),
                changes: Vec::new(),
                layers: Vec::new(),
            });
            continue;
        };
        let mut changes = Vec::new();
        if order.contains(&page.id) {
            changes.push(Change::new(
                "order",
                a.page_index(page.id).map(|i| ordinal(i, a.pages().len())),
                Some(ordinal(index, b.pages().len())),
            ));
        }
        if let (Some(na), Some(nb)) = (a.get(page.id), b.get(page.id)) {
            d.name_change(na, nb, &mut changes);
            d.fields(na, nb, &mut changes);
        }
        let ground = |p: &crate::page::Page| serde_json::to_value(p.ground).unwrap_or_default();
        d.compare(
            &["background".into()],
            "ground",
            &ground(old),
            &ground(page),
            &mut changes,
        );
        if old.draft != page.draft {
            changes.push(Change::new(
                "draft",
                Some(on_off(old.draft)),
                Some(on_off(page.draft)),
            ));
        }
        let layers = d.layers(Some(page.id), Some(page.id), &changed);
        if !changes.is_empty() || !layers.is_empty() {
            pages.push(PageDiff {
                id: page.id.to_wire(),
                name,
                status: Status::Changed,
                inside: 0,
                changes,
                layers,
            });
        }
    }
    for page in a.pages().iter().filter(|p| b.page(p.id).is_none()) {
        pages.push(PageDiff {
            id: page.id.to_wire(),
            name: d.name_in(a, page.id),
            status: Status::Removed,
            inside: d.count_new(a, b, page.id),
            changes: Vec::new(),
            layers: Vec::new(),
        });
    }
    if a.vault().is_some() || b.vault().is_some() {
        let layers = d.layers(b.vault(), a.vault(), &changed);
        if let Some(vault) = b.vault().or(a.vault())
            && !layers.is_empty()
        {
            let doc = if b.vault().is_some() { b } else { a };
            pages.push(PageDiff {
                id: vault.to_wire(),
                name: d.name_in(doc, vault),
                status: Status::Changed,
                inside: 0,
                changes: Vec::new(),
                layers,
            });
        }
    }
    Diff {
        pages,
        document: d.document_changes(),
    }
}

struct Differ<'d> {
    a: &'d Document,
    b: &'d Document,
    /// Each document's saved form, its `nodes` taken out into the maps below.
    saved: [Value; 2],
    /// Each document's saved layers by id, less what [`Differ::fields`] leaves
    /// to other words.
    nodes: [FxHashMap<String, Value>; 2],
    /// Display names by saved id — layers, variables, modes and styles of both
    /// documents, the later one's winning — so a binding or a link prints as what
    /// it names.
    names: FxHashMap<String, String>,
}

impl<'d> Differ<'d> {
    fn new(a: &'d Document, b: &'d Document) -> Self {
        let mut names = FxHashMap::default();
        let mut saved = [a, b]
            .map(|doc| serde_json::to_value(DocumentDto::from_document(doc)).unwrap_or_default());
        let nodes = saved.each_mut().map(|table| {
            let list = table.get_mut("nodes").map(Value::take).unwrap_or_default();
            let mut by_id = FxHashMap::default();
            for mut node in list.as_array().cloned().unwrap_or_default() {
                let id = node["id"].as_str().unwrap_or_default().to_string();
                if let Some(name) = node["name"].as_str() {
                    names.insert(id.clone(), name.to_string());
                }
                if let Some(map) = node.as_object_mut() {
                    for key in ["id", "parent", "children", "name", "transform"] {
                        map.remove(key);
                    }
                }
                by_id.insert(id, node);
            }
            by_id
        });
        for table in &saved {
            let lists = [
                &table["variables"]["vars"],
                &table["variables"]["modes"],
                &table["styles"],
            ];
            for item in lists.into_iter().filter_map(Value::as_array).flatten() {
                if let (Some(id), Some(name)) = (item["id"].as_str(), item["name"].as_str()) {
                    names.insert(id.to_string(), name.to_string());
                }
            }
        }
        Self {
            a,
            b,
            saved,
            nodes,
            names,
        }
    }

    fn name_in(&self, doc: &Document, id: NodeId) -> String {
        doc.get(id)
            .map(|n| n.name().to_string())
            .unwrap_or_default()
    }

    /// Every layer in both documents that changed, with its changes — less those
    /// a copy shares with the layer it was copied from, which are that layer's.
    fn layer_changes(&self) -> FxHashMap<NodeId, Vec<Change>> {
        let roots: FxHashSet<NodeId> = (self.a.page_ids().chain(self.b.page_ids()))
            .chain(self.a.vault())
            .chain(self.b.vault())
            .collect();
        let order = self.sibling_order();
        let mut changed: FxHashMap<NodeId, Vec<Change>> = FxHashMap::default();
        for nb in self.b.nodes_iter().filter(|n| !roots.contains(&n.id())) {
            let Some(na) = self.a.get(nb.id()) else {
                continue;
            };
            let mut out = Vec::new();
            self.name_change(na, nb, &mut out);
            if na.parent() != nb.parent() {
                let parent = |doc: &Document, n: &Node| n.parent().map(|p| self.name_in(doc, p));
                out.push(Change::new(
                    "parent",
                    parent(self.a, na),
                    parent(self.b, nb),
                ));
            } else if let Some(moved) = order.get(&nb.id()) {
                out.push(moved.clone());
            }
            transform_changes(na, nb, &mut out);
            self.fields(na, nb, &mut out);
            if !out.is_empty() {
                changed.insert(nb.id(), out);
            }
        }
        let echoes: Vec<(NodeId, Vec<Change>)> = changed
            .iter()
            .filter_map(|(id, own)| {
                let source = changed.get(&self.b.get(*id)?.link()?)?;
                let kept: Vec<Change> = own
                    .iter()
                    .filter(|c| !source.contains(c))
                    .cloned()
                    .collect();
                (kept.len() < own.len()).then_some((*id, kept))
            })
            .collect();
        for (id, kept) in echoes {
            if kept.is_empty() {
                changed.remove(&id);
            } else {
                changed.insert(id, kept);
            }
        }
        changed
    }

    /// The `order` change of each layer that moved among siblings it kept: the
    /// layers outside the longest run the two orders share, so one layer raised
    /// past three reports one move, not four.
    fn sibling_order(&self) -> FxHashMap<NodeId, Change> {
        let mut out = FxHashMap::default();
        for parent in self.b.nodes_iter() {
            let Some(old) = self.a.get(parent.id()) else {
                continue;
            };
            let (ca, cb) = (old.children(), parent.children());
            let moved = moved_in_order(ca, cb);
            for id in moved {
                let before = ca
                    .iter()
                    .position(|c| *c == id)
                    .map(|i| from_top(i, ca.len()));
                let after = cb
                    .iter()
                    .position(|c| *c == id)
                    .map(|i| from_top(i, cb.len()));
                out.insert(id, Change::new("order", before, after));
            }
        }
        out
    }

    fn name_change(&self, na: &Node, nb: &Node, out: &mut Vec<Change>) {
        if na.name() != nb.name() {
            out.push(Change::new(
                "name",
                Some(quoted(na.name())),
                Some(quoted(nb.name())),
            ));
        }
    }

    /// Every saved field of a layer but its place in the tree, its name and its
    /// transform, which are reported in their own words.
    fn fields(&self, na: &Node, nb: &Node, out: &mut Vec<Change>) {
        let null = Value::Null;
        let [sa, sb] = &self.nodes;
        let saved = |side: &'_ FxHashMap<String, Value>, n: &Node| {
            side.get(&n.id().to_wire()).unwrap_or(&null).clone()
        };
        self.compare(&[], "", &saved(sa, na), &saved(sb, nb), out);
    }

    /// The entries under `b_root` in `b`, top first as the Layers panel lists
    /// them, with `a`'s removed layers after their old siblings; `a_root` is the
    /// same place in `a`.
    fn layers(
        &self,
        b_root: Option<NodeId>,
        a_root: Option<NodeId>,
        changed: &FxHashMap<NodeId, Vec<Change>>,
    ) -> Vec<LayerDiff> {
        let mut out = Vec::new();
        if let Some(root) = b_root.and_then(|r| self.b.get(r)) {
            for child in root.children().iter().rev() {
                self.visit(*child, changed, &mut out);
            }
        }
        if let Some(root) = a_root {
            self.removed_under(root, &mut out);
        }
        out
    }

    fn visit(
        &self,
        id: NodeId,
        changed: &FxHashMap<NodeId, Vec<Change>>,
        out: &mut Vec<LayerDiff>,
    ) {
        let Some(node) = self.b.get(id) else {
            return;
        };
        let entry = |status, inside, changes| LayerDiff {
            id: id.to_wire(),
            trail: trail(self.b, id),
            kind: node.kind().default_name().to_string(),
            status,
            inside,
            changes,
        };
        if !self.a.contains(id) {
            out.push(entry(
                Status::Added,
                self.count_new(self.b, self.a, id) - 1,
                Vec::new(),
            ));
        } else if let Some(changes) = changed.get(&id) {
            out.push(entry(Status::Changed, 0, changes.clone()));
        }
        for child in node.children().iter().rev() {
            self.visit(*child, changed, out);
        }
        if self.a.contains(id) {
            self.removed_under(id, out);
        }
    }

    /// An entry for each of `parent`'s children in `a` that `b` no longer has.
    fn removed_under(&self, parent: NodeId, out: &mut Vec<LayerDiff>) {
        let Some(node) = self.a.get(parent) else {
            return;
        };
        for id in node
            .children()
            .iter()
            .rev()
            .filter(|c| !self.b.contains(**c))
        {
            let Some(gone) = self.a.get(*id) else {
                continue;
            };
            out.push(LayerDiff {
                id: id.to_wire(),
                trail: trail(self.a, *id),
                kind: gone.kind().default_name().to_string(),
                status: Status::Removed,
                inside: self.count_new(self.a, self.b, *id) - 1,
                changes: Vec::new(),
            });
        }
    }

    /// How many layers of `doc`'s subtree at `id`, itself included, `other` lacks;
    /// the page root of a page is not a layer and is not counted.
    fn count_new(&self, doc: &Document, other: &Document, id: NodeId) -> usize {
        let mut n = 0;
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            let Some(node) = doc.get(at) else { continue };
            if !other.contains(at) && !doc.is_page(at) {
                n += 1;
            }
            stack.extend(node.children());
        }
        n
    }

    /// The changes outside every page: variables, modes, styles, guides, images,
    /// the library block, anything a later schema adds there — and a publish,
    /// reported only as having happened.
    fn document_changes(&self) -> Vec<Change> {
        let empty = Map::new();
        let [va, vb] = &self.saved;
        let (ma, mb) = (
            va.as_object().unwrap_or(&empty),
            vb.as_object().unwrap_or(&empty),
        );
        let mut out = Vec::new();
        for key in union_keys(ma, mb) {
            if matches!(key.as_str(), "schema_version" | "pages" | "nodes" | "vault") {
                continue;
            }
            let (x, y) = (ma.get(&key), mb.get(&key));
            if key == "published" {
                match (x, y) {
                    (None, Some(_)) => {
                        out.push(Change::new("kit publish", None, Some("published".into())))
                    }
                    (Some(_), None) => {
                        out.push(Change::new("kit publish", Some("published".into()), None))
                    }
                    (Some(x), Some(y)) if !self.same(x, y) => out.push(Change::new(
                        "kit publish",
                        Some("published".into()),
                        Some("published again".into()),
                    )),
                    _ => {}
                }
                continue;
            }
            let null = Value::Null;
            let here = self.segment(&[], &key);
            self.compare(
                &here,
                &key,
                x.unwrap_or(&null),
                y.unwrap_or(&null),
                &mut out,
            );
        }
        out
    }

    /// Push a [`Change`] for every leaf of `a` and `b` that differs, under the
    /// label `path` and the key that holds them.
    fn compare(&self, path: &[String], key: &str, a: &Value, b: &Value, out: &mut Vec<Change>) {
        if self.same(a, b) {
            return;
        }
        match (a, b) {
            (Value::Null, _) => {
                out.push(Change::new(label(path, key), None, Some(self.show(key, b))))
            }
            (_, Value::Null) => {
                out.push(Change::new(label(path, key), Some(self.show(key, a)), None))
            }
            // An enum's variant changed: report the whole value, not its fields.
            (Value::Object(x), Value::Object(y))
                if is_color(a)
                    || is_color(b)
                    || (x.len() == 1 && y.len() == 1 && x.keys().ne(y.keys())) =>
            {
                out.push(self.whole(path, key, a, b));
            }
            (Value::Object(x), Value::Object(y)) => {
                // A value under an id — a variable's cell under its mode — is an
                // enum whose tag only repeats the variable's kind.
                let tagged = self.names.contains_key(key) && x.len() == 1 && y.len() == 1;
                for k in union_keys(x, y) {
                    let here = if tagged {
                        path.to_vec()
                    } else {
                        self.segment(path, &k)
                    };
                    match (x.get(&k), y.get(&k)) {
                        (Some(p), Some(q)) => self.compare(&here, &k, p, q, out),
                        (None, Some(q)) => match skipped_default(q) {
                            Some(p) => self.compare(&here, &k, &p, q, out),
                            None => out.push(Change::new(
                                label(&here, &k),
                                None,
                                Some(self.show(&k, q)),
                            )),
                        },
                        (Some(p), None) => match skipped_default(p) {
                            Some(q) => self.compare(&here, &k, p, &q, out),
                            None => out.push(Change::new(
                                label(&here, &k),
                                Some(self.show(&k, p)),
                                None,
                            )),
                        },
                        (None, None) => {}
                    }
                }
            }
            (Value::Array(x), Value::Array(y)) if keyed(x) && keyed(y) => {
                self.compare_keyed(path, key, x, y, out);
            }
            _ => out.push(self.whole(path, key, a, b)),
        }
    }

    fn whole(&self, path: &[String], key: &str, a: &Value, b: &Value) -> Change {
        Change::new(
            label(path, key),
            Some(self.show(key, a)),
            Some(self.show(key, b)),
        )
    }

    /// A list whose items carry an `id` (fills, effects, variables…): items are
    /// matched by it, and a new order of the items both sides hold is one change.
    fn compare_keyed(
        &self,
        path: &[String],
        key: &str,
        a: &[Value],
        b: &[Value],
        out: &mut Vec<Change>,
    ) {
        let id = |v: &Value| v["id"].as_str().unwrap_or_default().to_string();
        let (ida, idb): (Vec<String>, Vec<String>) =
            (a.iter().map(id).collect(), b.iter().map(id).collect());
        let noun = singular(key);
        let name_of = |list: &[Value], i: usize| match list[i]["name"].as_str() {
            Some(name) => format!("{noun} {}", quoted(name)),
            None => format!("{noun} {}", i + 1),
        };
        // `path` ends in the list's own word, which each item's label replaces.
        let list = label(path, key);
        let mut here = path.to_vec();
        if here.last().is_some_and(|w| *w == key.replace('_', " ")) {
            here.pop();
        }
        for (i, item) in b.iter().enumerate() {
            let item_label = name_of(b, i);
            here.push(item_label.clone());
            match ida.iter().position(|x| *x == idb[i]) {
                Some(j) => self.compare(&here, "", &without_id(&a[j]), &without_id(item), out),
                None => out.push(Change::new(
                    here.join(" "),
                    None,
                    Some(self.show(key, item)),
                )),
            }
            here.pop();
        }
        for (j, item) in a
            .iter()
            .enumerate()
            .filter(|(j, _)| !idb.contains(&ida[*j]))
        {
            here.push(name_of(a, j));
            out.push(Change::new(
                here.join(" "),
                Some(self.show(key, item)),
                None,
            ));
            here.pop();
        }
        if !moved_in_order(&ida, &idb).is_empty() {
            let shared = |ids: &[String], list: &[Value]| {
                let kept: Vec<String> = ids
                    .iter()
                    .zip(list)
                    .filter(|(x, _)| ida.contains(x) && idb.contains(x))
                    .map(|(_, v)| self.show(key, v))
                    .collect();
                cut(&kept.join(", "))
            };
            out.push(Change::new(
                list + " order",
                Some(shared(&ida, a)),
                Some(shared(&idb, b)),
            ));
        }
    }

    /// `path` with `key`'s word added, if it has one: the tags of an enum, the
    /// wrappers `paint`, `kind`, `brush`, `size` and the like add none, and an id
    /// adds the name it stands for.
    fn segment(&self, path: &[String], key: &str) -> Vec<String> {
        let mut here = path.to_vec();
        let silent = matches!(
            key,
            "" | "paint" | "kind" | "brush" | "size" | "value" | "variables" | "cells" | "style"
        ) || key.starts_with(|c: char| c.is_ascii_uppercase());
        if !silent {
            here.push(match self.names.get(key) {
                Some(name) => name.clone(),
                None => match key {
                    "content" => "text".into(),
                    "ground" => "background".into(),
                    "meta" => "library".into(),
                    other => other.replace('_', " "),
                },
            });
        }
        here
    }

    /// Whether two saved values read the same: numbers within [`EPSILON`], colours
    /// by what they print as, everything else exactly.
    fn same(&self, a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => !differs(
                x.as_f64().unwrap_or_default(),
                y.as_f64().unwrap_or_default(),
            ),
            _ if is_color(a) && is_color(b) => self.show("", a) == self.show("", b),
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .all(|(k, v)| y.get(k).is_some_and(|w| self.same(v, w)))
            }
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(v, w)| self.same(v, w))
            }
            _ => a == b,
        }
    }

    /// A saved value as the inspector would write it, cut to one short line.
    fn show(&self, key: &str, v: &Value) -> String {
        let shown = match v {
            Value::Null => "none".into(),
            Value::Bool(b) => on_off(*b),
            Value::Number(n) => number(n.as_f64().unwrap_or_default()),
            Value::String(s) if key == "data" => format!("{} bytes of data", s.len() / 4 * 3),
            Value::String(s) => match self.names.get(s) {
                Some(name) if key != "id" => name.clone(),
                _ if matches!(key, "content" | "name" | "description") => quoted(s),
                _ => s.replace('_', " "),
            },
            _ if is_color(v) => hex(v),
            Value::Array(items) => {
                let parts: Vec<String> = items.iter().map(|i| self.show(key, i)).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Object(map) => self.show_object(map),
        };
        cut(&shown)
    }

    fn show_object(&self, map: &Map<String, Value>) -> String {
        // A bound paint: the variable's name, and its alpha when not whole.
        if let Some(var) = map.get("var") {
            let alpha = map.get("alpha").and_then(Value::as_f64).unwrap_or(1.0);
            let name = self.show("var", var);
            return if differs(alpha, 1.0) {
                format!("{name} {}%", number(alpha * 100.0))
            } else {
                name
            };
        }
        // A fill or a stroke reads as its paint, an effect as its kind.
        if let Some(brush) = map.get("brush") {
            let hidden = map.get("visible") == Some(&Value::Bool(false));
            return self.show("brush", brush) + if hidden { " (hidden)" } else { "" };
        }
        if let Some(kind) = map.get("kind").and_then(|k| k.get("type")) {
            return self.show("type", kind);
        }
        if let Some(name) = map.get("name").and_then(Value::as_str) {
            return quoted(name);
        }
        // An enum variant: a colour inside reads as the colour, else the variant.
        if map.len() == 1
            && let Some((tag, inner)) = map.iter().next()
            && tag.starts_with(|c: char| c.is_ascii_uppercase())
        {
            return if is_color(inner) {
                hex(inner)
            } else {
                tag.to_lowercase()
            };
        }
        let parts: Vec<String> = map
            .iter()
            .filter(|(k, _)| k.as_str() != "id")
            .map(|(k, v)| format!("{}: {}", k.replace('_', " "), self.show(k, v)))
            .collect();
        parts.join(", ")
    }
}

/// The position, rotation, flip and skew changes between two transforms, each
/// in the parent's space.
fn transform_changes(a: &Node, b: &Node, out: &mut Vec<Change>) {
    let (ca, cb) = (a.transform().as_coeffs(), b.transform().as_coeffs());
    if differs(ca[4], cb[4]) || differs(ca[5], cb[5]) {
        let at = |c: [f64; 6]| format!("{}, {}", number(c[4]), number(c[5]));
        out.push(Change::new("position", Some(at(ca)), Some(at(cb))));
    }
    let flipped = |c: [f64; 6]| c[0] * c[3] - c[1] * c[2] < 0.0;
    let degrees = |c: [f64; 6]| c[1].atan2(c[0]).to_degrees();
    if flipped(ca) != flipped(cb) {
        out.push(Change::new(
            "flipped",
            Some(on_off(flipped(ca))),
            Some(on_off(flipped(cb))),
        ));
    } else if differs(degrees(ca), degrees(cb)) {
        let deg = |c| format!("{}°", number(degrees(c)));
        out.push(Change::new("rotation", Some(deg(ca)), Some(deg(cb))));
    } else if (0..4).any(|i| (ca[i] - cb[i]).abs() > 1e-6) {
        let m = |c: [f64; 6]| {
            format!(
                "{}, {}, {}, {}",
                number(c[0]),
                number(c[1]),
                number(c[2]),
                number(c[3])
            )
        };
        out.push(Change::new("skew", Some(m(ca)), Some(m(cb))));
    }
}

/// The names from the top of `id`'s page (or the vault) down to `id`, an
/// instance's own name marked.
fn trail(doc: &Document, id: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = doc.get(id);
    while let Some(node) = at {
        let Some(parent) = node.parent() else { break };
        if crate::component::instance_root(doc, node.id()) == Some(node.id()) {
            out.push(format!("{} (instance)", node.name()));
        } else {
            out.push(node.name().to_string());
        }
        at = doc.get(parent);
    }
    out.reverse();
    out
}

/// The items of `b` that both sides hold but that left the longest common run
/// of the two orders — the ones that moved, as opposed to the ones moved past.
fn moved_in_order<T: PartialEq + Clone>(a: &[T], b: &[T]) -> Vec<T> {
    let x: Vec<&T> = a.iter().filter(|v| b.contains(v)).collect();
    let y: Vec<&T> = b.iter().filter(|v| a.contains(v)).collect();
    if x == y {
        return Vec::new();
    }
    // The longest common subsequence, by the table of suffix lengths.
    let (n, m) = (x.len(), y.len());
    let mut len = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            len[i][j] = if x[i] == y[j] {
                len[i + 1][j + 1] + 1
            } else {
                len[i + 1][j].max(len[i][j + 1])
            };
        }
    }
    let mut kept = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if x[i] == y[j] {
            kept.push(y[j]);
            i += 1;
            j += 1;
        } else if len[i + 1][j] >= len[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    y.into_iter()
        .filter(|v| !kept.contains(v))
        .cloned()
        .collect()
}

/// A layer's place among `len` siblings as the Layers panel lists them, top first.
fn from_top(index: usize, len: usize) -> String {
    format!("{} of {len} from the top", len - index)
}

/// A page's place in the page list, first first.
fn ordinal(index: usize, len: usize) -> String {
    format!("{} of {len}", index + 1)
}

fn differs(x: f64, y: f64) -> bool {
    // Less a hair, so an edit of exactly `EPSILON` is not lost to rounding.
    (x - y).abs() >= EPSILON - 1e-9
}

/// A number as the inspector prints one: at most two decimals, no trailing zeros.
fn number(x: f64) -> String {
    let r = (x * 100.0).round() / 100.0;
    if r == 0.0 {
        "0".into()
    } else if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r}")
    }
}

fn on_off(b: bool) -> String {
    if b { "on" } else { "off" }.into()
}

fn quoted(s: &str) -> String {
    format!("“{}”", s.replace(['\r', '\n'], " "))
}

fn cut(s: &str) -> String {
    if s.chars().count() <= SHOWN_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(SHOWN_CHARS - 1).collect();
    out.push('…');
    out
}

/// A saved colour: `{"components": [r, g, b, a]}`.
fn is_color(v: &Value) -> bool {
    v.as_object().is_some_and(|m| {
        m.len() == 1
            && m.get("components")
                .and_then(Value::as_array)
                .is_some_and(|c| c.len() == 4)
    })
}

/// `#RRGGBB`, and the alpha as a percentage when it is not whole.
fn hex(v: &Value) -> String {
    let c: Vec<f64> = v["components"]
        .as_array()
        .map(|a| a.iter().map(|x| x.as_f64().unwrap_or_default()).collect())
        .unwrap_or_default();
    let byte = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut s = format!("#{:02X}{:02X}{:02X}", byte(c[0]), byte(c[1]), byte(c[2]));
    if byte(c[3]) != 255 {
        s.push_str(&format!(" {}%", (c[3].clamp(0.0, 1.0) * 100.0).round()));
    }
    s
}

/// What an absent key stood for, when the file skips it at a default of `v`'s
/// type: an empty list, `false`, zero. `None` for a value an `Option` held,
/// whose absence is nothing, so the value reads as added or removed.
fn skipped_default(v: &Value) -> Option<Value> {
    match v {
        Value::Array(_) => Some(Value::Array(Vec::new())),
        Value::Bool(_) => Some(Value::Bool(false)),
        Value::Number(_) => Some(Value::from(0)),
        _ => None,
    }
}

/// Whether a list's items are matched by id: every one an object carrying one.
fn keyed(items: &[Value]) -> bool {
    items
        .iter()
        .all(|v| v.get("id").is_some_and(Value::is_string))
}

fn without_id(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(map) = v.as_object_mut() {
        map.remove("id");
    }
    v
}

/// `a`'s keys in its order, then those only `b` has.
fn union_keys(a: &Map<String, Value>, b: &Map<String, Value>) -> Vec<String> {
    a.keys()
        .chain(b.keys().filter(|k| !a.contains_key(*k)))
        .cloned()
        .collect()
}

fn label(path: &[String], key: &str) -> String {
    if path.is_empty() {
        key.replace('_', " ")
    } else {
        path.join(" ")
    }
}

/// What one item of the list under `key` is called.
fn singular(key: &str) -> String {
    match key {
        "vars" => "variable".into(),
        "props" => "property".into(),
        "grids" => "layout grid".into(),
        other => other.strip_suffix('s').unwrap_or(other).replace('_', " "),
    }
}
