//! The MCP host (§8): the tool handlers, run on the thread that owns the
//! documents, the window's or `serve`'s.
//!
//! [`Engine`] keeps each connection's current document and what it last showed
//! that connection, and answers each call against an [`EditorSession`] lent by
//! the host through [`Documents`]. Writes go through `EditorSession::try_commit`,
//! the door the app's own edits use.

pub(crate) mod endpoint;
pub(crate) mod proxy;
pub(crate) mod serve;
pub(crate) mod window;

use crate::session::EditorSession;
use ondin_core::NodeId;
use ondin_mcp::queue::{Call, Conn};
use ondin_mcp::wire::{Content, Output};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// What a host lends the engine: its documents, by absolute path.
pub(crate) trait Documents {
    /// Find the document `wanted` names and open it if it is not open,
    /// answering its absolute path, or why not (several candidates, none).
    fn open(&mut self, wanted: &str) -> Result<PathBuf, String>;
    /// The open document at `path`.
    fn session(&mut self, path: &Path) -> Option<&mut EditorSession>;
    /// The document at `path` changed through a tool: `serve` writes it.
    fn committed(&mut self, path: &Path);
}

/// The document `wanted` names: an absolute path; else a name or relative path
/// matched against the open documents, then the library's. Several matches and
/// none are errors the agent can show the person.
pub(crate) fn resolve(
    wanted: &str,
    open: &[PathBuf],
    library: Option<&Path>,
) -> Result<PathBuf, String> {
    let asked = Path::new(wanted);
    if asked.is_absolute() {
        return match asked.is_file() {
            true => Ok(asked.to_path_buf()),
            false => Err(format!("No file at {wanted}.")),
        };
    }
    let key = wanted
        .trim_end_matches(".ondin")
        .to_lowercase()
        .replace('\\', "/");
    let matches = |path: &Path, name: Option<&str>| {
        let p = path.to_string_lossy().to_lowercase().replace('\\', "/");
        let p = p.trim_end_matches(".ondin");
        p.ends_with(&format!("/{key}")) || name.is_some_and(|n| n.to_lowercase() == key)
    };
    let mut found: Vec<PathBuf> = open.iter().filter(|p| matches(p, None)).cloned().collect();
    if found.is_empty()
        && let Some(root) = library
    {
        found = crate::library::scan::scan(root)
            .into_iter()
            .filter(|e| matches(&e.path, e.meta.name.as_deref()))
            .map(|e| e.path)
            .collect();
    }
    found.sort();
    found.dedup();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "No document called `{wanted}` is open or in the library. Ask the person for its path."
        )),
        _ => Err(format!(
            "Several documents match `{wanted}`; ask the person which: {}",
            found
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// How many revisions a connection's earlier reads are remembered at, for
/// `since` and `base_rev`.
const KEPT: usize = 8;

/// What a connection was last shown: a revision and each layer's fingerprint.
struct Shown {
    rev: u64,
    layers: HashMap<String, Layer>,
}

/// A layer as a connection saw it.
#[derive(Clone, PartialEq)]
struct Layer {
    name: String,
    hash: u64,
}

#[derive(Default)]
struct Connection {
    document: Option<PathBuf>,
    /// Newest last.
    shown: VecDeque<Shown>,
}

/// The tool handlers and every connection's state.
#[derive(Default)]
pub(crate) struct Engine {
    connections: HashMap<Conn, Connection>,
}

/// What changed between two looks at a document.
#[derive(Default)]
struct Delta {
    added: Vec<String>,
    changed: Vec<String>,
    removed: Vec<String>,
}

impl Delta {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }

    /// Each id with the name it has (or, removed, had).
    fn to_json(&self, now: &HashMap<String, Layer>, then: &HashMap<String, Layer>) -> Value {
        let named = |ids: &[String], from: &HashMap<String, Layer>| -> Vec<Value> {
            ids.iter()
                .map(|id| json!({ "id": id, "name": from.get(id).map_or("", |l| l.name.as_str()) }))
                .collect()
        };
        json!({
            "added": named(&self.added, now),
            "changed": named(&self.changed, now),
            "removed": named(&self.removed, then),
        })
    }
}

fn delta(then: &HashMap<String, Layer>, now: &HashMap<String, Layer>) -> Delta {
    let mut d = Delta::default();
    for (id, layer) in now {
        match then.get(id) {
            None => d.added.push(id.clone()),
            Some(old) if old != layer => d.changed.push(id.clone()),
            Some(_) => {}
        }
    }
    d.removed = then
        .keys()
        .filter(|id| !now.contains_key(*id))
        .cloned()
        .collect();
    d.added.sort();
    d.changed.sort();
    d.removed.sort();
    d
}

/// Every layer's fingerprint: its snapshot entry, hashed.
fn layers(snapshot: &ondin_export::Snapshot) -> HashMap<String, Layer> {
    use std::hash::{Hash as _, Hasher as _};
    snapshot
        .nodes
        .iter()
        .map(|n| {
            let mut h = std::hash::DefaultHasher::new();
            serde_json::to_string(n).unwrap_or_default().hash(&mut h);
            (
                n.id.clone(),
                Layer {
                    name: n.name.clone(),
                    hash: h.finish(),
                },
            )
        })
        .collect()
}

fn snapshot_of(s: &EditorSession) -> ondin_export::Snapshot {
    ondin_export::snapshot(&s.doc, &s.resolved)
}

impl Engine {
    /// Forget a connection that closed.
    pub(crate) fn closed(&mut self, conn: Conn) {
        self.connections.remove(&conn);
    }

    /// The documents some connection is using, for a host deciding what it may close.
    pub(crate) fn in_use(&self) -> impl Iterator<Item = &Path> {
        self.connections
            .values()
            .filter_map(|c| c.document.as_deref())
    }

    /// Whether any connection is open.
    pub(crate) fn has_connections(&self) -> bool {
        !self.connections.is_empty()
    }

    /// Answer one call.
    pub(crate) fn handle(&mut self, docs: &mut impl Documents, call: Call) -> Output {
        let conn = self.connections.entry(call.conn).or_default();
        if call.tool == "use_document" {
            let Some(wanted) = call.args.get("document").and_then(Value::as_str) else {
                return Output::error("`use_document` needs `document`");
            };
            return match docs.open(wanted) {
                Ok(path) => {
                    let Some(session) = docs.session(&path) else {
                        return Output::error(format!("{} did not open", path.display()));
                    };
                    let snap = snapshot_of(session);
                    let rev = session.revision();
                    conn.document = Some(path.clone());
                    conn.shown.clear();
                    remember(conn, rev, layers(&snap));
                    let pages: Vec<Value> = snap
                        .pages
                        .iter()
                        .map(|p| json!({ "id": p.id, "name": p.name }))
                        .collect();
                    Output::json(json!({
                        "rev": rev,
                        "document": path.display().to_string(),
                        "pages": pages,
                        "layers": snap.nodes.len(),
                    }))
                }
                Err(why) => Output::error(why),
            };
        }
        let Some(path) = conn.document.clone() else {
            return Output::error(
                "No document yet: call `use_document` with the file the person named.",
            );
        };
        let Some(session) = docs.session(&path) else {
            conn.document = None;
            return Output::error(format!(
                "{} is no longer open: call `use_document` again.",
                path.display()
            ));
        };
        // What moved since this connection last looked, the person's edits and
        // undos included, reported before the call runs.
        let before = session.revision();
        let changed = catch_up(conn, session);
        let answer = run(conn, session, &call);
        let after = session.revision();
        let mut output = match answer {
            Ok(Answer::Json(v)) => Output::json(v),
            Ok(Answer::Image(bytes)) => Output {
                content: vec![Content::Image {
                    bytes,
                    mime: "image/png",
                }],
                structured: None,
                is_error: false,
            },
            Err(why) => Output::error(why),
        };
        if after != before {
            // The call's own edit is not news to the caller; an undo's or redo's
            // layers are, since it did not choose them.
            let now = layers(&snapshot_of(session));
            let own = conn
                .shown
                .back()
                .map(|s| delta(&s.layers, &now).to_json(&now, &s.layers));
            remember(conn, after, now);
            if matches!(call.tool.as_str(), "undo" | "redo")
                && let (Some(own), Some(Value::Object(map))) = (own, output.structured.as_mut())
            {
                map.insert("layers".into(), own);
            }
            docs.committed(&path);
        }
        stamp(&mut output, after, changed);
        output
    }
}

fn remember(conn: &mut Connection, rev: u64, layers: HashMap<String, Layer>) {
    if conn.shown.back().is_some_and(|s| s.rev == rev) {
        return;
    }
    conn.shown.push_back(Shown { rev, layers });
    while conn.shown.len() > KEPT {
        conn.shown.pop_front();
    }
}

/// What changed since the connection's last look, and remember this one.
fn catch_up(conn: &mut Connection, session: &EditorSession) -> Option<Value> {
    let rev = session.revision();
    let last = conn.shown.back()?;
    if last.rev == rev {
        return None;
    }
    let now = layers(&snapshot_of(session));
    let d = delta(&last.layers, &now);
    let report = (!d.is_empty()).then(|| d.to_json(&now, &last.layers));
    remember(conn, rev, now);
    report
}

/// Put `rev` and, when there is one, `changed` on an answer.
fn stamp(output: &mut Output, rev: u64, changed: Option<Value>) {
    match output.structured.as_mut() {
        Some(Value::Object(map)) => {
            map.insert("rev".into(), json!(rev));
            if let Some(c) = changed {
                map.insert("changed".into(), c);
            }
            output.content = vec![Content::Text(Value::Object(map.clone()).to_string())];
        }
        _ => {
            let mut note = json!({ "rev": rev });
            if let Some(c) = changed {
                note["changed"] = c;
            }
            output.content.push(Content::Text(note.to_string()));
        }
    }
}

enum Answer {
    Json(Value),
    Image(Vec<u8>),
}

fn node(args: &Value, name: &str) -> Result<Option<NodeId>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => NodeId::from_wire(s)
            .map(Some)
            .ok_or_else(|| format!("`{name}`: `{s}` is not a layer id")),
        Some(other) => Err(format!("`{name}`: expected a layer id, got {other}")),
    }
}

fn required(args: &Value, name: &str) -> Result<NodeId, String> {
    node(args, name)?.ok_or_else(|| format!("needs `{name}`"))
}

fn index(args: &Value, name: &str) -> Option<u64> {
    args.get(name).and_then(Value::as_u64)
}

/// The page `id` names or sits on; the first page for none.
fn page_of(s: &EditorSession, id: Option<NodeId>) -> Result<NodeId, String> {
    match id {
        None => Ok(s.doc.first_page()),
        Some(id) if is_page(s, id) => Ok(id),
        Some(id) => s
            .doc
            .page_of(id)
            .ok_or_else(|| format!("no layer `{}`", id.to_wire())),
    }
}

fn is_page(s: &EditorSession, id: NodeId) -> bool {
    s.doc.page_ids().any(|p| p == id)
}

/// The default longest side of `export_png`.
const PNG_MAX: u64 = 1600;

fn run(conn: &Connection, s: &mut EditorSession, call: &Call) -> Result<Answer, String> {
    let args = &call.args;
    let json = |v: Value| Ok(Answer::Json(v));
    match call.tool.as_str() {
        "get_document" => {
            let snap = snapshot_of(s);
            if let Some(since) = since(conn, args, s.revision(), &snap)? {
                return json(since);
            }
            json(serde_json::to_value(&snap).map_err(|e| e.to_string())?)
        }
        "get_node" => {
            let id = required(args, "id")?.to_wire();
            let snap = snapshot_of(s);
            if let Some(since) = since(conn, args, s.revision(), &snap)? {
                return json(since);
            }
            let by_id: HashMap<&str, &ondin_export::snapshot::NodeSnapshot> =
                snap.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
            let root = by_id
                .get(id.as_str())
                .ok_or_else(|| format!("no layer `{id}`"))?;
            let mut nodes = vec![*root];
            if args.get("subtree").and_then(Value::as_bool) == Some(true) {
                let mut i = 0;
                while i < nodes.len() {
                    for c in &nodes[i].children {
                        if let Some(n) = by_id.get(c.as_str()) {
                            nodes.push(n);
                        }
                    }
                    i += 1;
                }
            }
            json(json!({ "nodes": nodes }))
        }
        "list_children" => {
            let snap = snapshot_of(s);
            if let Some(since) = since(conn, args, s.revision(), &snap)? {
                return json(since);
            }
            let Some(id) = node(args, "id")? else {
                let pages: Vec<Value> = snap
                    .pages
                    .iter()
                    .map(|p| json!({ "id": p.id, "name": p.name, "kind": "page" }))
                    .collect();
                return json(json!({ "children": pages }));
            };
            let id = id.to_wire();
            let by_id: HashMap<&str, &ondin_export::snapshot::NodeSnapshot> =
                snap.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
            let parent = by_id
                .get(id.as_str())
                .ok_or_else(|| format!("no layer `{id}`"))?;
            let children: Vec<Value> = parent
                .children
                .iter()
                .filter_map(|c| by_id.get(c.as_str()))
                .map(|n| json!({ "id": n.id, "name": n.name, "kind": n.kind }))
                .collect();
            json(json!({ "children": children }))
        }
        "find_nodes" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("needs `query`")?
                .to_lowercase();
            let page = node(args, "page")?.map(NodeId::to_wire);
            let limit = index(args, "limit").unwrap_or(50) as usize;
            let snap = snapshot_of(s);
            let by_id: HashMap<&str, &ondin_export::snapshot::NodeSnapshot> =
                snap.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
            let mut found = Vec::new();
            let mut more = false;
            for n in &snap.nodes {
                if page.as_ref().is_some_and(|p| *p != n.page) || n.id == n.page {
                    continue;
                }
                let text = match &n.geometry {
                    ondin_export::snapshot::Geometry::Text { content, .. } => content.as_str(),
                    _ => "",
                };
                if !(n.name.to_lowercase().contains(&query) || text.to_lowercase().contains(&query))
                {
                    continue;
                }
                if found.len() == limit {
                    more = true;
                    break;
                }
                let mut path = Vec::new();
                let mut up = n.parent.as_deref();
                while let Some(p) = up.and_then(|p| by_id.get(p)) {
                    path.push(p.name.clone());
                    up = p.parent.as_deref();
                }
                path.reverse();
                found.push(json!({ "id": n.id, "name": n.name, "kind": n.kind, "path": path }));
            }
            json(json!({ "nodes": found, "more": more }))
        }
        "hit_test" => {
            let x = args.get("x").and_then(Value::as_f64).ok_or("needs `x`")?;
            let y = args.get("y").and_then(Value::as_f64).ok_or("needs `y`")?;
            let page = page_of(s, node(args, "page")?)?;
            let hits = ondin_core::query::hit_test(&s.resolved, &s.doc, page, (x, y).into(), 0.0);
            json(json!({ "ids": hits.iter().map(|h| h.to_wire()).collect::<Vec<_>>() }))
        }
        "get_bounds" => {
            let id = required(args, "id")?;
            let r = s
                .resolved
                .world_bounds(id)
                .ok_or_else(|| format!("`{}` has no bounds", id.to_wire()))?;
            json(json!({ "bounds": [r.x0, r.y0, r.width(), r.height()] }))
        }
        "get_selection" => json(json!({
            "ids": s.selection.ids().iter().map(|i| i.to_wire()).collect::<Vec<_>>()
        })),
        "set_selection" => {
            let ids = args
                .get("ids")
                .and_then(Value::as_array)
                .ok_or("needs `ids`")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .and_then(NodeId::from_wire)
                        .filter(|id| s.doc.get(*id).is_some())
                        .ok_or_else(|| format!("`{v}` is no layer"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            s.selection.set(ids);
            json(json!({}))
        }
        "export_svg" => {
            let id = node(args, "id")?;
            let svg = match id {
                Some(id) if !is_page(s, id) => {
                    ondin_export::svg::svg_of(&s.doc, &s.resolved, &[id])
                }
                _ => {
                    let page = page_of(s, id)?;
                    ondin_export::svg::svg(&s.doc, &s.resolved, page, None)
                }
            };
            json(json!({ "svg": svg }))
        }
        "export_png" => {
            let id = node(args, "id")?;
            let max = index(args, "max").unwrap_or(PNG_MAX).max(16) as f64;
            let png = match id {
                Some(id) if !is_page(s, id) => {
                    let r = s
                        .resolved
                        .world_bounds(id)
                        .ok_or_else(|| format!("`{}` has no bounds", id.to_wire()))?;
                    let scale = (max / r.width().max(r.height()).max(1.0)).min(2.0);
                    let opts = ondin_export::png::RasterOpts {
                        scale,
                        ..Default::default()
                    };
                    let r = ondin_export::png::raster_of(&s.doc, &s.resolved, &[id], &opts)
                        .ok_or("nothing to draw")?;
                    ondin_export::png::encode_rgba8(&r.rgba, r.width, r.height)
                }
                _ => {
                    let page = page_of(s, id)?;
                    let view = ondin_export::extent(&s.resolved, &[page])
                        .unwrap_or_else(|| ondin_core::kurbo::Rect::new(0.0, 0.0, 512.0, 512.0));
                    let scale = (max / view.width().max(view.height()).max(1.0)).min(1.0);
                    let vp = ondin_export::png::viewport_at(view, scale);
                    ondin_export::png::png(&s.doc, &s.resolved, &vp, page)
                }
            };
            Ok(Answer::Image(png))
        }
        "undo" => json(json!({ "undone": s.undo() })),
        "redo" => json(json!({ "redone": s.redo() })),
        tool => write(conn, s, &call.client, tool, args),
    }
}

/// A read's `since`: `unchanged`, or the layers changed and removed since that
/// revision; `None` to answer in full (no `since`, or one too old to diff).
fn since(
    conn: &Connection,
    args: &Value,
    rev: u64,
    snap: &ondin_export::Snapshot,
) -> Result<Option<Value>, String> {
    let Some(since) = index(args, "since") else {
        return Ok(None);
    };
    if since == rev {
        return Ok(Some(json!({ "unchanged": true })));
    }
    let Some(then) = conn.shown.iter().find(|s| s.rev == since) else {
        return Ok(None);
    };
    let now = layers(snap);
    let d = delta(&then.layers, &now);
    let fresh: Vec<&ondin_export::snapshot::NodeSnapshot> = snap
        .nodes
        .iter()
        .filter(|n| d.added.contains(&n.id) || d.changed.contains(&n.id))
        .collect();
    Ok(Some(
        json!({ "since": since, "nodes": fresh, "removed": d.removed }),
    ))
}

/// A registry verb, committed through the session's door as one undo step named
/// by the tool, with the style browser's *Edit* set aside for it.
fn write(
    conn: &Connection,
    s: &mut EditorSession,
    client: &str,
    tool: &str,
    args: &Value,
) -> Result<Answer, String> {
    let verb = ondin_core::verbs::find(tool).ok_or_else(|| format!("no tool `{tool}`"))?;
    let mut args = args.clone();
    let base = args
        .as_object_mut()
        .and_then(|m| m.remove("base_rev"))
        .filter(|v| !v.is_null());
    let done = {
        let mut pictures = crate::panels::import_files::Pictures;
        let mut measurer = ondin_export::contrast::Measurer::default();
        let mut ctx = ondin_core::verbs::Ctx {
            doc: &s.doc,
            res: &s.resolved,
            ids: &mut s.ids,
            images: Some(&mut pictures),
            contrast: Some(&mut measurer),
        };
        ondin_core::verbs::call(&mut ctx, verb.name, &args).map_err(|e| e.to_string())?
    };
    if let Some(base) = base {
        let base = base.as_u64().ok_or("`base_rev` must be a `rev`")?;
        stale(conn, s, base, &done.tx)?;
    }
    s.name_mcp_step(client, verb.name, "");
    let held = s.style_edit.take();
    let applied = s.try_commit(done.tx);
    s.style_edit = held;
    applied.map_err(|e| match e {
        crate::session::CommitError::Op(e) => format!("Refused: {e}"),
        crate::session::CommitError::ReadOnly => {
            "The document is read-only while the person compares versions.".to_owned()
        }
    })?;
    Ok(Answer::Json(json!({
        "created": done.created.iter().map(|i| i.to_wire()).collect::<Vec<_>>(),
        "made": done.made,
    })))
}

/// Refuse a write whose layers changed after `base`.
fn stale(
    conn: &Connection,
    s: &EditorSession,
    base: u64,
    tx: &ondin_core::Transaction,
) -> Result<(), String> {
    if base == s.revision() {
        return Ok(());
    }
    let Some(then) = conn.shown.iter().find(|x| x.rev == base) else {
        return Err(format!(
            "`base_rev` {base} is older than this connection remembers: read again and retry"
        ));
    };
    let now = layers(&snapshot_of(s));
    let d = delta(&then.layers, &now);
    let moved: Vec<String> = tx
        .subjects()
        .iter()
        .map(|id| id.to_wire())
        .filter(|id| d.changed.contains(id) || d.removed.contains(id))
        .collect();
    if moved.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = moved
        .iter()
        .map(|id| match then.layers.get(id) {
            Some(l) => format!("{} ({id})", l.name),
            None => id.clone(),
        })
        .collect();
    Err(format!(
        "Refused: changed since rev {base}: {}. Read them again and retry.",
        names.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    /// One open document at `/a.ondin`.
    struct One {
        path: PathBuf,
        session: EditorSession,
        committed: usize,
    }

    impl One {
        fn new() -> Self {
            Self {
                path: PathBuf::from("/a.ondin"),
                session: EditorSession::new(),
                committed: 0,
            }
        }
    }

    impl Documents for One {
        fn open(&mut self, wanted: &str) -> Result<PathBuf, String> {
            match wanted {
                "a" => Ok(self.path.clone()),
                _ => Err(format!("no `{wanted}`")),
            }
        }
        fn session(&mut self, path: &Path) -> Option<&mut EditorSession> {
            (path == self.path).then_some(&mut self.session)
        }
        fn committed(&mut self, _: &Path) {
            self.committed += 1;
        }
    }

    fn call(engine: &mut Engine, docs: &mut One, conn: Conn, tool: &str, args: Value) -> Output {
        let (reply, _answer) = sync_channel(1);
        engine.handle(
            docs,
            Call {
                conn,
                client: "test".into(),
                tool: tool.into(),
                args,
                reply,
            },
        )
    }

    fn ok(out: &Output) -> &Value {
        assert!(!out.is_error, "{:?}", out.content);
        out.structured.as_ref().unwrap()
    }

    fn open(engine: &mut Engine, docs: &mut One, conn: Conn) {
        ok(&call(
            engine,
            docs,
            conn,
            "use_document",
            json!({ "document": "a" }),
        ));
    }

    /// Make a rectangle on the first page through the engine, answering its id.
    fn rect(engine: &mut Engine, docs: &mut One, conn: Conn, name: &str) -> String {
        let page = docs.session.doc.first_page().to_wire();
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let args = json!({ "parent": page, "kind": kind, "name": name });
        let out = call(engine, docs, conn, "create_node", args);
        ok(&out)["created"][0].as_str().unwrap().to_owned()
    }

    /// A tool before `use_document` is refused; after, a write lands through the
    /// session as one undo step, and the host is told to write the file.
    #[test]
    fn a_write_needs_a_document_and_lands_as_one_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        assert!(call(&mut engine, &mut docs, 1, "get_document", json!({})).is_error);
        open(&mut engine, &mut docs, 1);
        let id = NodeId::from_wire(&rect(&mut engine, &mut docs, 1, "Header")).unwrap();
        assert_eq!(docs.session.doc.get(id).unwrap().name(), "Header");
        assert_eq!(docs.committed, 1);
        assert_eq!(docs.session.history.undo_depth(), 1);
    }

    /// Another connection's edit is reported as `changed` on this one's next
    /// call, once, and a connection is never told of its own write.
    ///
    /// flip: `catch_up` answering `None` always → fails at the second
    /// connection's `changed`.
    #[test]
    fn another_connections_edit_arrives_as_changed() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Card");
        let mine = call(&mut engine, &mut docs, 1, "get_selection", json!({}));
        assert!(
            ok(&mine).get("changed").is_none(),
            "own write reported back"
        );
        let theirs = call(&mut engine, &mut docs, 2, "get_selection", json!({}));
        let changed = &ok(&theirs)["changed"];
        assert_eq!(changed["added"][0]["id"], json!(id));
        assert_eq!(changed["added"][0]["name"], "Card");
        let again = call(&mut engine, &mut docs, 2, "get_selection", json!({}));
        assert!(ok(&again).get("changed").is_none(), "reported twice");
    }

    /// A write whose layer changed after its `base_rev` is refused naming it; one
    /// at the current revision applies.
    ///
    /// flip: `stale` answering `Ok(())` always → fails at "refused".
    #[test]
    fn a_stale_write_is_refused() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Button");
        let read = call(&mut engine, &mut docs, 2, "get_node", json!({ "id": id }));
        let base = ok(&read)["rev"].clone();
        let rename =
            |name: &str, base: &Value| json!({ "node": id, "name": name, "base_rev": base });
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_name",
            rename("Primary", &Value::Null),
        ));
        let late = call(&mut engine, &mut docs, 2, "set_name", rename("Mine", &base));
        assert!(late.is_error, "refused");
        let Content::Text(why) = &late.content[0] else {
            panic!("no text")
        };
        assert!(why.contains("Button"), "{why}");
        let read = call(&mut engine, &mut docs, 2, "get_node", json!({ "id": id }));
        let now = ok(&read)["rev"].clone();
        ok(&call(
            &mut engine,
            &mut docs,
            2,
            "set_name",
            rename("Mine", &now),
        ));
    }

    /// `find_nodes` finds by name, ignoring case, with its ancestors' names; a
    /// read with `since` at the current revision answers `unchanged`.
    #[test]
    fn find_reaches_a_layer_and_since_skips_an_unchanged_read() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        rect(&mut engine, &mut docs, 1, "Home header");
        rect(&mut engine, &mut docs, 1, "Footer");
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "find_nodes",
            json!({ "query": "HEADER" }),
        );
        let nodes = ok(&found)["nodes"].as_array().unwrap().clone();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["name"], "Home header");
        assert_eq!(nodes[0]["path"].as_array().unwrap().len(), 1, "the page");
        let rev = ok(&found)["rev"].clone();
        let read = call(
            &mut engine,
            &mut docs,
            1,
            "get_document",
            json!({ "since": rev }),
        );
        assert_eq!(ok(&read)["unchanged"], true);
    }

    /// An agent's write leaves the style browser's *Edit* as it found it.
    #[test]
    fn a_write_leaves_the_style_edit_open() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let style: ondin_core::style::StyleId = serde_json::from_value(json!("1:1")).unwrap();
        docs.session.style_edit = Some(style);
        rect(&mut engine, &mut docs, 1, "Chip");
        assert_eq!(docs.session.style_edit, Some(style));
    }

    /// A name matches an open document by file name, with or without the
    /// extension; an absolute path that is no file and an unknown name fail.
    #[test]
    fn a_document_is_found_by_name() {
        let open = [
            PathBuf::from("/work/dashboard.ondin"),
            PathBuf::from("/work/site.ondin"),
        ];
        assert_eq!(resolve("dashboard.ondin", &open, None), Ok(open[0].clone()));
        assert_eq!(resolve("Site", &open, None), Ok(open[1].clone()));
        assert!(resolve("nothing", &open, None).is_err());
        let missing = std::env::temp_dir().join("ondin-no-such-file.ondin");
        assert!(resolve(&missing.display().to_string(), &open, None).is_err());
    }
}
