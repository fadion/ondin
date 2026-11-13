//! The MCP host (§8): the tool handlers, run on the thread that owns the
//! documents, the window's or `serve`'s.
//!
//! [`Engine`] keeps each connection's current document and what it last showed
//! that connection, and answers each call against an [`EditorSession`] lent by
//! the host through [`Documents`]. Writes go through `EditorSession::try_commit`,
//! the door the app's own edits use.

mod actions;
pub(crate) mod endpoint;
mod inspect;
mod kits;
pub(crate) mod proxy;
mod reads;
pub(crate) mod serve;
mod system_fonts;
pub(crate) mod window;

use crate::library::state::Library;
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
    /// The document at `path` changed through a tool: `serve` writes it, as the
    /// window does a tab behind. An error is why the edit is not on disk.
    fn committed(&mut self, path: &Path) -> Result<(), String>;
    /// The library as it now stands on disk, or `None` with no base folder.
    fn library(&mut self) -> Option<&Library>;
    /// The open document at `path` and the library together, for a kit tool
    /// that reads both.
    fn with_library(&mut self, path: &Path) -> Option<(&mut EditorSession, Option<&Library>)>;
    /// Write the document at `path` now, as the kit mark's changes are.
    fn save(&mut self, path: &Path) -> Result<(), String>;
    /// Put `line` on the status line, where a person would read it.
    fn tell(&mut self, line: String);
    /// Every font family a text here can be set in, sorted.
    fn fonts(&mut self) -> Vec<String>;
    /// The canvas's size in device pixels while the document at `path` is the
    /// tab in front of the person; `None` behind and in `serve`, where nobody
    /// looks at it.
    fn canvas(&mut self, path: &Path) -> Option<(u32, u32)>;
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
    /// Whether this client's own call made it from the look before, so
    /// `base_rev` does not count that step against the client.
    own: bool,
}

/// A layer as a connection saw it.
#[derive(Clone, PartialEq)]
struct Layer {
    name: String,
    hash: u64,
}

/// One client's state: its current document and what it was last shown.
#[derive(Default)]
struct Connection {
    document: Option<PathBuf>,
    /// Newest last.
    shown: VecDeque<Shown>,
    /// When it last called, in [`Engine::tick`]s: the least recent keyed client
    /// is forgotten first.
    used: u64,
}

/// Whose state a call reads: a keyed client's, which outlives its connections,
/// or an unkeyed connection's, which ends with it.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Who {
    Key(String),
    Conn(Conn),
}

/// How many keyed clients a host remembers after their connections close. A
/// harness that starts a process per turn comes back on a new connection each
/// turn with the same key.
const CLIENTS: usize = 32;

/// The tool handlers and every client's state.
#[derive(Default)]
pub(crate) struct Engine {
    clients: HashMap<Who, Connection>,
    /// The connections open now, which `serve` waits on.
    live: std::collections::HashSet<Conn>,
    tick: u64,
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
    /// Forget a connection that closed, and its state unless a key holds it.
    /// Answers whether it had called: a connection that only looked whether the
    /// endpoint answers, as `endpoint::hosts` does, is no client leaving.
    pub(crate) fn closed(&mut self, conn: Conn) -> bool {
        self.clients.remove(&Who::Conn(conn));
        self.live.remove(&conn)
    }

    /// The documents some client is using, for a host deciding what it may close.
    pub(crate) fn in_use(&self) -> impl Iterator<Item = &Path> {
        self.clients.values().filter_map(|c| c.document.as_deref())
    }

    /// Whether any connection is open.
    pub(crate) fn has_connections(&self) -> bool {
        !self.live.is_empty()
    }

    /// Whether a keyed client's state is held, which a host keeps for its next
    /// connection.
    pub(crate) fn remembers_clients(&self) -> bool {
        self.clients.keys().any(|w| matches!(w, Who::Key(_)))
    }

    /// The state `call` reads and writes, made when new; the least recently
    /// used keyed client is forgotten past [`CLIENTS`].
    fn client(&mut self, call: &Call) -> &mut Connection {
        self.live.insert(call.conn);
        self.tick += 1;
        let who = match &call.key {
            Some(key) => Who::Key(key.clone()),
            None => Who::Conn(call.conn),
        };
        if !self.clients.contains_key(&who) && matches!(who, Who::Key(_)) {
            let keyed = self
                .clients
                .iter()
                .filter(|(w, _)| matches!(w, Who::Key(_)));
            if keyed.clone().count() >= CLIENTS
                && let Some(oldest) = keyed.min_by_key(|(_, c)| c.used).map(|(w, _)| w.clone())
            {
                self.clients.remove(&oldest);
            }
        }
        let tick = self.tick;
        let client = self.clients.entry(who).or_default();
        client.used = tick;
        client
    }

    /// Answer one call.
    pub(crate) fn handle(&mut self, docs: &mut impl Documents, call: Call) -> Output {
        if let Err(why) = check_call(&call) {
            return Output::error(why);
        }
        let conn = self.client(&call);
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
                    // The document already in use keeps what this client was
                    // shown, so `since`, `base_rev` and `changed` carry across a
                    // reconnect. The proxy's own restore reports no `changed`,
                    // leaving it for the agent's next call.
                    let mut changed = None;
                    if conn.document.as_deref() == Some(path.as_path()) {
                        if call.args.get("restore") != Some(&json!(true)) {
                            changed = catch_up(conn, session);
                        }
                    } else {
                        conn.document = Some(path.clone());
                        conn.shown.clear();
                        remember(conn, rev, layers(&snap), false);
                    }
                    let pages: Vec<Value> = snap
                        .pages
                        .iter()
                        .map(|p| json!({ "id": p.id, "name": p.name }))
                        .collect();
                    let mut answer = json!({
                        "rev": rev,
                        "document": path.display().to_string(),
                        "pages": pages,
                        "layers": snap.nodes.len(),
                    });
                    if let Some(c) = changed {
                        answer["changed"] = c;
                    }
                    Output::json(answer)
                }
                Err(why) => Output::error(why),
            };
        }
        if call.tool == "list_fonts" {
            return Output::json(fonts(docs, &call.args));
        }
        if kits::WITHOUT_DOCUMENT.contains(&call.tool.as_str()) {
            return match kits::without_document(docs, &call) {
                Ok(v) => Output::json(v),
                Err(why) => Output::error(why),
            };
        }
        let Some(path) = conn.document.clone() else {
            return Output::error(
                "No document yet: call `use_document` with the file the person named.",
            );
        };
        let canvas = match call.tool.as_str() {
            "get_view" => docs.canvas(&path),
            _ => None,
        };
        // Core cannot see the host's fonts: a family nobody can set text in would
        // draw every replaced layer in a fallback. Written in the host's spelling.
        let mut call = call;
        if call.tool == "replace_font"
            && let Some(to) = call.args.get("to").and_then(Value::as_str)
        {
            let to = to.trim().to_owned();
            let key = to.to_lowercase();
            match docs.fonts().into_iter().find(|f| f.to_lowercase() == key) {
                Some(family) => call.args["to"] = json!(family),
                None => {
                    return Output::error(format!(
                        "`to`: no family called {to} can be set here; `list_fonts` names those that can"
                    ));
                }
            }
        }
        let Some(session) = docs.session(&path) else {
            conn.document = None;
            conn.shown.clear();
            return Output::error(format!(
                "{} is no longer open: call `use_document` again.",
                path.display()
            ));
        };
        // What moved since this connection last looked, the person's edits and
        // undos included, reported before the call runs.
        let before = session.revision();
        let changed = catch_up(conn, session);
        let answer = if kits::is_kit_tool(&call.tool) {
            kits::run(docs, &path, &call)
        } else if actions::TOOLS.contains(&call.tool.as_str()) {
            actions::run(docs, &path, &call)
        } else if call.tool == "get_view" {
            Ok(Answer::Json(view(session, canvas)))
        } else {
            run(conn, session, &call)
        };
        let Some(session) = docs.session(&path) else {
            return Output::error(format!("{} closed during the call", path.display()));
        };
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
            Ok(Answer::Files(report, files)) => {
                let mut output = Output::json(report);
                output.content.extend(files);
                output
            }
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
            remember(conn, after, now, true);
            if matches!(call.tool.as_str(), "undo" | "redo")
                && let (Some(own), Some(Value::Object(map))) = (own, output.structured.as_mut())
            {
                map.insert("layers".into(), own);
            }
            if let Err(why) = docs.committed(&path) {
                let line = format!("The edit is made but not written to the file: {why}");
                match output.structured.as_mut() {
                    Some(Value::Object(map)) => {
                        map.insert("unsaved".into(), json!(line));
                    }
                    _ => output.content.push(Content::Text(line)),
                }
            }
        }
        stamp(&mut output, after, changed);
        output
    }
}

/// Refuse a tool no host serves, before anything else, and a table tool's
/// arguments as a verb's are refused: unknown, missing or of the wrong kind. A
/// `"12"` for a `since` read as absent answered in full without a word.
fn check_call(call: &Call) -> Result<(), String> {
    if ondin_core::verbs::find(&call.tool).is_some() {
        return Ok(());
    }
    let Some(tool) = ondin_mcp::schema::tool(&call.tool) else {
        return Err(format!(
            "No tool is called `{}`; `tools/list` names them.",
            call.tool
        ));
    };
    let mut args = call.args.clone();
    // An empty selection clears it, where a layer list elsewhere needs one.
    if tool.name == "set_selection" && call.args.get("ids") == Some(&json!([])) {
        return Ok(());
    }
    // The proxy's restore after a reconnect, never listed to the agent.
    if tool.name == "use_document"
        && let Some(map) = args.as_object_mut()
    {
        map.remove("restore");
    }
    ondin_core::verbs::check_args(tool.name, tool.params, &args)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn remember(conn: &mut Connection, rev: u64, layers: HashMap<String, Layer>, own: bool) {
    if conn.shown.back().is_some_and(|s| s.rev == rev) {
        return;
    }
    conn.shown.push_back(Shown { rev, layers, own });
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
    remember(conn, rev, now, false);
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
            // The first content is the structured answer's text; files follow it.
            let text = Content::Text(Value::Object(map.clone()).to_string());
            match output.content.first_mut() {
                Some(first @ Content::Text(_)) => *first = text,
                _ => output.content.insert(0, text),
            }
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
    /// A report and the files it lists.
    Files(Value, Vec<Content>),
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

/// A list of layer ids, `None` when absent.
fn nodes(args: &Value, name: &str) -> Result<Option<Vec<NodeId>>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(ids)) => ids
            .iter()
            .map(|v| {
                v.as_str()
                    .and_then(NodeId::from_wire)
                    .ok_or_else(|| format!("`{name}`: `{v}` is not a layer id"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(other) => Err(format!(
            "`{name}`: expected a list of layer ids, got {other}"
        )),
    }
}

fn required(args: &Value, name: &str) -> Result<NodeId, String> {
    node(args, name)?.ok_or_else(|| format!("needs `{name}`"))
}

fn index(args: &Value, name: &str) -> Option<u64> {
    args.get(name).and_then(Value::as_u64)
}

/// The page `id` names or sits on; for none, the page the person is on (the
/// first in a document nobody has open).
fn page_of(s: &EditorSession, id: Option<NodeId>) -> Result<NodeId, String> {
    match id {
        None => Ok(s.page),
        Some(id) if is_page(s, id) => Ok(id),
        Some(id) => s
            .doc
            .page_of(id)
            .ok_or_else(|| format!("no layer `{}`", id.to_wire())),
    }
}

/// What the person sees of the document: the page they are on, the camera's
/// centre and zoom, and, while it is the tab in front (`canvas`), the world
/// rectangle the canvas shows.
fn view(s: &EditorSession, canvas: Option<(u32, u32)>) -> Value {
    let name = s.doc.get(s.page).map(|p| p.name().to_owned());
    let c = s.camera;
    let mut answer = json!({
        "page": { "id": s.page.to_wire(), "name": name },
        "center": [c.center.x, c.center.y],
        "zoom": c.zoom,
        "in_front": canvas.is_some(),
    });
    if let Some(px) = canvas {
        let r = c.viewport(px).view;
        answer["visible"] = json!([r.x0, r.y0, r.width(), r.height()]);
    }
    answer
}

fn is_page(s: &EditorSession, id: NodeId) -> bool {
    s.doc.page_ids().any(|p| p == id)
}

/// The default longest side of `export_png`.
const PNG_MAX: u64 = 1600;

/// The longest answer a whole-part read gives, in bytes. Harnesses cut a tool
/// result past their own limit (Claude Code's is about 25,000 tokens), and a
/// snapshot cut mid-way reads as a smaller document; a refusal naming the size
/// and the narrower reads is what the model can act on.
const ANSWER_BUDGET: usize = 100_000;

/// Refuse `text` past [`ANSWER_BUDGET`], saying what to read `instead`.
fn within_budget(text: &str, what: &str, instead: &str) -> Result<(), String> {
    match text.len() {
        n if n <= ANSWER_BUDGET => Ok(()),
        n => Err(format!(
            "{what} is {} KB, past the {} KB a client is sure to read whole: {instead}",
            n / 1000,
            ANSWER_BUDGET / 1000
        )),
    }
}

/// What a read past the budget points to.
const NARROWER: &str =
    "reach the part you need with `find_nodes` or `list_children`, then `get_node` on it";

/// The most `export_png`'s `max` is taken at: a raster held in the person's
/// window, where an unbounded one could exhaust its memory and lose every tab.
const PNG_CEILING: u64 = 4096;

fn run(conn: &Connection, s: &mut EditorSession, call: &Call) -> Result<Answer, String> {
    let args = &call.args;
    let json = |v: Value| Ok(Answer::Json(v));
    match call.tool.as_str() {
        "get_document" => {
            let snap = snapshot_of(s);
            if let Some(since) = since(conn, args, s.revision(), &snap)? {
                return json(since);
            }
            let whole = serde_json::to_value(&snap).map_err(|e| e.to_string())?;
            within_budget(&whole.to_string(), "The whole document", NARROWER)?;
            json(whole)
        }
        "get_tree" => {
            let id = required(args, "id")?;
            if is_page(s, id) {
                return Err(format!(
                    "`{}` is a page: `list_children` names its layers, each a tree",
                    id.to_wire()
                ));
            }
            let tree = ondin_core::verbs::tree(&s.doc, id)
                .ok_or_else(|| format!("no layer `{}`", id.to_wire()))?;
            let answer = json!({ "tree": tree });
            within_budget(
                &answer.to_string(),
                "This tree",
                "read the layers inside it one `get_tree` at a time",
            )?;
            json(answer)
        }
        "get_node" => {
            let id = required(args, "id")?;
            if s.doc.get(id).is_none() {
                return Err(format!("no layer `{}`", id.to_wire()));
            }
            let id = id.to_wire();
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
            let answer = json!({ "nodes": nodes });
            within_budget(&answer.to_string(), "This subtree", NARROWER)?;
            json(answer)
        }
        "list_children" => {
            let id = node(args, "id")?;
            if let Some(id) = id
                && s.doc.get(id).is_none()
            {
                return Err(format!("no layer `{}`", id.to_wire()));
            }
            let snap = snapshot_of(s);
            if let Some(since) = since(conn, args, s.revision(), &snap)? {
                return json(since);
            }
            let Some(id) = id else {
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
            // As a person can select: layers of the page shown, never the page
            // itself or a kit's vendored component. The page is not switched,
            // which would move the person's view.
            if let Some(id) = ids
                .iter()
                .find(|id| **id == s.page || s.doc.page_of(**id) != Some(s.page))
            {
                return Err(format!(
                    "`{}` is not a layer of the page the person is on (`{}`); a selection \
                     never spans pages",
                    id.to_wire(),
                    s.page.to_wire()
                ));
            }
            let mut seen = std::collections::HashSet::new();
            let ids: Vec<NodeId> = ids.into_iter().filter(|id| seen.insert(*id)).collect();
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
            within_budget(
                &svg,
                "The SVG",
                "export one layer with `id`, or look with `export_png`",
            )?;
            json(json!({ "svg": svg }))
        }
        "export_png" => {
            let id = node(args, "id")?;
            let max = index(args, "max").unwrap_or(PNG_MAX);
            if max > PNG_CEILING {
                return Err(format!(
                    "`max` is at most {PNG_CEILING}; export a part with `id` to see it closer"
                ));
            }
            let max = max.max(16) as f64;
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
        "run_exports" => reads::run_exports(s, nodes(args, "ids")?),
        "export_code" => reads::export_code(s, args, nodes(args, "ids")?),
        "list_components" => json(reads::components(&s.doc)),
        "list_variables" => json(reads::variables(&s.doc)),
        "list_styles" => json(reads::styles(&s.doc)),
        tool if inspect::TOOLS.contains(&tool) => inspect::run(s, tool, args),
        "batch" => batch(conn, s, &call.client, args),
        tool => write(conn, s, &call.client, tool, args),
    }
}

/// `list_fonts`: the families whose name holds `query`, ignoring case, at most
/// `limit` (100 when absent), and how many matched in all.
fn fonts(docs: &mut impl Documents, args: &Value) -> Value {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let limit = index(args, "limit").unwrap_or(100) as usize;
    let matched: Vec<String> = docs
        .fonts()
        .into_iter()
        .filter(|f| f.to_lowercase().contains(&query))
        .collect();
    json!({
        "families": matched.iter().take(limit).collect::<Vec<_>>(),
        "matched": matched.len(),
    })
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
    refuse_kit_vault(s, args)?;
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
    commit(s, client, verb.name, done.tx)?;
    // A picture's own size, which decides what aspect `fit` and `crop` keep.
    let pixels = match verb.name {
        "add_image" => done
            .made
            .as_ref()
            .and_then(|id| s.doc.image(&ondin_core::ImageId(id.clone())))
            .map(|e| [e.width, e.height]),
        _ => None,
    };
    let mut answer = json!({
        "created": done.created.iter().map(|i| i.to_wire()).collect::<Vec<_>>(),
        "made": done.made,
    });
    if let Some(said) = done.said {
        answer["said"] = json!(said);
    }
    if let Some(px) = pixels {
        answer["pixel_size"] = json!(px);
    }
    Ok(Answer::Json(answer))
}

/// `batch`: each step's verb committed in turn, each reading what the last left,
/// then joined into one undo step named `batch`. A refused step takes back the
/// steps before it, so the batch lands whole or not at all. A step names what an
/// earlier one made as `"@label"`, the label that step gave in `as`, as
/// `ondin script` does; `base_rev` holds for every step.
fn batch(
    conn: &Connection,
    s: &mut EditorSession,
    client: &str,
    args: &Value,
) -> Result<Answer, String> {
    let Some(Value::Array(steps)) = args.get("steps") else {
        return Err("`steps` is a list of `{ tool, args, as }`".into());
    };
    if steps.is_empty() {
        return Err("`steps` is empty".into());
    }
    let base = args.get("base_rev").filter(|v| !v.is_null()).cloned();
    let mut labels = HashMap::new();
    let mut answers = Vec::new();
    let mut made = 0;
    for (i, step) in steps.iter().enumerate() {
        let tool = step.get("tool").and_then(Value::as_str).unwrap_or("?");
        let ran = batch_step(conn, s, client, step, &labels, base.as_ref());
        let (answer, pushed) = match ran {
            Ok(done) => done,
            Err(why) => {
                for _ in 0..made {
                    s.take_back();
                }
                return Err(format!(
                    "step {} (`{tool}`): {why}. Nothing was changed.",
                    i + 1
                ));
            }
        };
        made += pushed;
        if let Some(label) = step.get("as").and_then(Value::as_str) {
            let first = answer["created"]
                .get(0)
                .or_else(|| answer.get("made"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if let Some(first) = first {
                labels.insert(label.to_owned(), first);
            }
        }
        answers.push(answer);
    }
    if made > 1 {
        s.history.join_last(made);
    }
    if made > 0
        && let Some(note) = s.history.current_note_mut()
    {
        note.label = "batch".into();
        note.origin = ondin_core::Origin::Mcp {
            client: client.to_owned(),
            tool: "batch".into(),
        };
    }
    Ok(Answer::Json(json!({ "steps": answers })))
}

/// One step of [`batch`]: its answer and how many undo steps it committed.
fn batch_step(
    conn: &Connection,
    s: &mut EditorSession,
    client: &str,
    step: &Value,
    labels: &HashMap<String, String>,
    base: Option<&Value>,
) -> Result<(Value, usize), String> {
    if let Some(extra) = step.as_object().and_then(|m| {
        m.keys()
            .find(|k| !["tool", "args", "as"].contains(&k.as_str()))
    }) {
        return Err(format!("a step takes no `{extra}`"));
    }
    let tool = step
        .get("tool")
        .and_then(Value::as_str)
        .ok_or("a step names its `tool`")?;
    if ondin_core::verbs::find(tool).is_none() {
        return Err(format!(
            "a batch runs the write tools that are verbs, and `{tool}` is none"
        ));
    }
    let mut args = step.get("args").cloned().unwrap_or_else(|| json!({}));
    ondin_core::verbs::substitute(&mut args, labels)?;
    if let (Some(base), Some(map)) = (base, args.as_object_mut()) {
        map.insert("base_rev".into(), base.clone());
    }
    let depth = s.history.undo_depth();
    let Answer::Json(answer) = write(conn, s, client, tool, &args)? else {
        return Err("a write answers JSON".into());
    };
    Ok((answer, s.history.undo_depth().saturating_sub(depth)))
}

/// Refuse a verb that would edit a kit's vendored component, which a kit
/// instance's `component.main` names: the person can never select one, and an
/// edit there would change every instance until the kit's next update undid it.
/// `main` and `from` only read their layer, so they may name one.
fn refuse_kit_vault(s: &EditorSession, args: &Value) -> Result<(), String> {
    let mut ids = Vec::new();
    for key in ["node", "other", "parent"] {
        ids.extend(node(args, key).ok().flatten());
    }
    ids.extend(nodes(args, "nodes").ok().flatten().unwrap_or_default());
    match ids.into_iter().find(|id| s.doc.in_vault(*id)) {
        Some(id) => Err(format!(
            "`{}` is a kit's component, which changes only when the kit does: edit it in the \
             kit's own document and publish, or `make_local` the instance first",
            id.to_wire()
        )),
        None => Ok(()),
    }
}

/// Commit an agent's `tx` as one undo step named by `tool`, with the style
/// browser's *Edit* set aside: an agent's edit means the same whatever is open.
fn commit(
    s: &mut EditorSession,
    client: &str,
    tool: &str,
    tx: ondin_core::Transaction,
) -> Result<(), String> {
    s.name_mcp_step(client, tool, "");
    let held = s.style_edit.take();
    let applied = s.try_commit(tx);
    s.style_edit = held;
    applied.map_err(|e| match e {
        crate::session::CommitError::Op(e) => format!("Refused: {e}"),
        crate::session::CommitError::ReadOnly => READ_ONLY.to_owned(),
    })
}

/// Why nothing an agent asks changes a document the person is comparing.
const READ_ONLY: &str = "The document is read-only while the person compares versions.";

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
    let Some(at) = conn.shown.iter().position(|x| x.rev == base) else {
        return Err(format!(
            "`base_rev` {base} is older than this connection remembers: read again and retry"
        ));
    };
    let then = &conn.shown[at];
    // What others changed since `base`: every step but the client's own writes,
    // which are not news to it. `handle` caught up before this call, so the last
    // look is the document as the call found it.
    let mut others = std::collections::HashSet::new();
    for (a, b) in conn
        .shown
        .iter()
        .skip(at)
        .zip(conn.shown.iter().skip(at + 1))
    {
        if !b.own {
            let d = delta(&a.layers, &b.layers);
            others.extend(d.changed.into_iter().chain(d.removed));
        }
    }
    let moved: Vec<String> = tx
        .subjects()
        .iter()
        .map(|id| id.to_wire())
        .filter(|id| others.contains(id))
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
        fn committed(&mut self, _: &Path) -> Result<(), String> {
            self.committed += 1;
            Ok(())
        }
        fn library(&mut self) -> Option<&Library> {
            None
        }
        fn with_library(&mut self, path: &Path) -> Option<(&mut EditorSession, Option<&Library>)> {
            self.session(path).map(|s| (s, None))
        }
        fn save(&mut self, _: &Path) -> Result<(), String> {
            Ok(())
        }
        fn tell(&mut self, _: String) {}
        fn fonts(&mut self) -> Vec<String> {
            vec!["Inter".into(), "Inter Tight".into(), "Lora".into()]
        }
        fn canvas(&mut self, path: &Path) -> Option<(u32, u32)> {
            (path == self.path).then_some((800, 600))
        }
    }

    fn call(engine: &mut Engine, docs: &mut One, conn: Conn, tool: &str, args: Value) -> Output {
        keyed(engine, docs, conn, None, tool, args)
    }

    /// [`call`] from a client that gave `key`.
    fn keyed(
        engine: &mut Engine,
        docs: &mut One,
        conn: Conn,
        key: Option<&str>,
        tool: &str,
        args: Value,
    ) -> Output {
        let (reply, _answer) = sync_channel(1);
        engine.handle(
            docs,
            Call::new(conn, key.map(str::to_owned), tool, args, reply),
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

    /// Every listed tool that is not a verb reaches a handler: called with the
    /// least its parameters accept, none is answered as no tool, which a table
    /// tool missing from dispatch is (it falls through to the verbs). An unknown
    /// name is refused as one before anything else.
    ///
    /// flip: `run` without its `get_bounds` arm → fails at `get_bounds`.
    #[test]
    fn every_listed_tool_is_dispatched() {
        use ondin_core::verbs::ParamKind as K;
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let least = |kind: K| match kind {
            K::Node | K::Id(_) => json!(page),
            K::Nodes => json!([page]),
            K::Number | K::Index => json!(0),
            K::Bool => json!(false),
            K::Text => json!("x"),
            K::Choice(words) => json!(words[0]),
            K::Model("KitModeMap") => json!({}),
            K::Model("DtcgFile") => json!("{}"),
            K::Model(model) => ondin_core::verbs::example(model).unwrap_or(json!({})),
        };
        for tool in ondin_mcp::schema::TOOLS {
            if tool.name == "use_document" {
                continue;
            }
            let args: serde_json::Map<String, Value> = tool
                .params
                .iter()
                .filter(|p| p.required)
                .map(|p| (p.name.to_owned(), least(p.kind)))
                .collect();
            let out = call(&mut engine, &mut docs, 1, tool.name, Value::Object(args));
            let text = format!("{:?}", out.content);
            assert!(
                !text.contains("no tool `") && !text.contains("No tool is called"),
                "`{}` is not dispatched: {text}",
                tool.name
            );
        }
        let none = call(&mut engine, &mut docs, 1, "paint_it_red", json!({}));
        assert!(format!("{:?}", none.content).contains("No tool is called"));
    }

    /// `set_selection` takes layers of the page shown, once each, and refuses
    /// the page itself, leaving the selection as it was.
    ///
    /// flip: `set_selection` without its page check → fails at the page's
    /// refusal.
    #[test]
    fn set_selection_takes_layers_of_the_page_shown() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Chip");
        let set = |e: &mut Engine, d: &mut One, ids: Value| {
            call(e, d, 1, "set_selection", json!({ "ids": ids }))
        };
        ok(&set(&mut engine, &mut docs, json!([id, id])));
        assert_eq!(docs.session.selection.ids().len(), 1, "once each");
        let page = docs.session.page.to_wire();
        assert!(set(&mut engine, &mut docs, json!([page])).is_error);
        assert_eq!(docs.session.selection.ids().len(), 1, "left as it was");
    }

    /// `get_view` answers the page the person is on and what the canvas shows,
    /// and a read with no page reads that page, not the first.
    ///
    /// flip: `page_of` answering the first page for none → fails at "on the
    /// person's page".
    #[test]
    fn get_view_answers_the_persons_page_and_reads_default_to_it() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let added = call(
            &mut engine,
            &mut docs,
            1,
            "add_page",
            json!({ "name": "Two" }),
        );
        let two = ok(&added)["created"][0].as_str().unwrap().to_owned();
        let page = NodeId::from_wire(&two).unwrap();
        docs.session.switch_page(page);
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let args = json!({ "parent": two, "kind": kind });
        let made = call(&mut engine, &mut docs, 1, "create_node", args);
        let rect = ok(&made)["created"][0].clone();
        let view = call(&mut engine, &mut docs, 1, "get_view", json!({}));
        let view = ok(&view);
        assert_eq!(view["page"]["id"], json!(two));
        assert_eq!(view["page"]["name"], json!("Two"));
        let r = docs.session.camera.viewport((800, 600)).view;
        assert_eq!(view["visible"], json!([r.x0, r.y0, r.width(), r.height()]));
        // The example rectangle is 120 × 80 at the page's origin, where the first
        // page's frame also sits.
        let hit = call(
            &mut engine,
            &mut docs,
            1,
            "hit_test",
            json!({ "x": 10, "y": 10 }),
        );
        assert_eq!(ok(&hit)["ids"], json!([rect]), "on the person's page");
    }

    /// `get_tree` answers a layer in `insert_subtree`'s shape, which inserts back,
    /// and refuses a page.
    ///
    /// flip: `get_tree` without its page refusal → fails at "a page is refused".
    #[test]
    fn get_tree_answers_what_insert_subtree_takes() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Chip");
        let out = call(&mut engine, &mut docs, 1, "get_tree", json!({ "id": id }));
        let tree = ok(&out)["tree"].clone();
        assert_eq!(tree["name"], json!("Chip"));
        let page = docs.session.page.to_wire();
        let args = json!({ "parent": page, "tree": tree });
        ok(&call(&mut engine, &mut docs, 1, "insert_subtree", args));
        let read = call(&mut engine, &mut docs, 1, "get_tree", json!({ "id": page }));
        assert!(read.is_error, "a page is refused");
    }

    /// `replace_font` takes a family the host can set, in the host's spelling,
    /// and refuses one it cannot.
    ///
    /// flip: `handle` without its family check → fails at "a family the host
    /// lacks is refused".
    #[test]
    fn replace_font_takes_only_a_family_the_host_can_set() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.page.to_wire();
        let args = json!({ "parent": page, "text": "Hi" });
        let made = call(&mut engine, &mut docs, 1, "create_text", args);
        let t = NodeId::from_wire(ok(&made)["created"][0].as_str().unwrap()).unwrap();
        let family = |d: &One| match d.session.doc.get(t).unwrap().kind() {
            ondin_core::NodeKind::Text { style, .. } => style.font_family.clone(),
            _ => unreachable!(),
        };
        let from = family(&docs);
        let args = json!({ "from": from, "to": "Nowhere Sans" });
        let out = call(&mut engine, &mut docs, 1, "replace_font", args);
        assert!(out.is_error, "a family the host lacks is refused");
        let args = json!({ "from": from, "to": "lora" });
        ok(&call(&mut engine, &mut docs, 1, "replace_font", args));
        assert_eq!(family(&docs), "Lora", "in the host's spelling");
    }

    /// `add_image` answers the picture's own size beside its id.
    ///
    /// flip: `write` leaving out `pixel_size` → fails at the size.
    #[test]
    fn add_image_answers_the_pictures_pixel_size() {
        fn base64(bytes: &[u8]) -> String {
            const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let b = [
                    chunk[0],
                    *chunk.get(1).unwrap_or(&0),
                    *chunk.get(2).unwrap_or(&0),
                ];
                let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
                for i in 0..4 {
                    match i <= chunk.len() {
                        true => out.push(ABC[(n >> (18 - 6 * i)) as usize & 63] as char),
                        false => out.push('='),
                    }
                }
            }
            out
        }
        let mut png = Vec::new();
        image::RgbaImage::new(3, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let args = json!({ "bytes": base64(&png) });
        let out = call(&mut engine, &mut docs, 1, "add_image", args);
        assert!(ok(&out)["made"].is_string());
        assert_eq!(ok(&out)["pixel_size"], json!([3, 2]), "the size");
    }

    /// A connection that never called closing is no client leaving, which
    /// `serve` would otherwise end on; one that called is.
    ///
    /// flip: `Engine::closed` answering `true` always → fails at the probe.
    #[test]
    fn a_probe_closing_is_no_client_leaving() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        assert!(!engine.closed(2), "a probe never called");
        assert!(engine.has_connections());
        assert!(engine.closed(1), "a client that called left");
        assert!(!engine.has_connections());
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

    /// Two writes off one read apply: the client's own first write is no change
    /// against its `base_rev`. Another client's write in between still refuses.
    ///
    /// flip: `stale` counting every step, its own included → fails at "its own
    /// write is not news".
    #[test]
    fn a_clients_own_write_does_not_stale_its_base() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Button");
        let read = call(&mut engine, &mut docs, 1, "get_node", json!({ "id": id }));
        let base = ok(&read)["rev"].clone();
        let rename = |name: &str| json!({ "node": id, "name": name, "base_rev": base });
        ok(&call(&mut engine, &mut docs, 1, "set_name", rename("One")));
        let second = call(&mut engine, &mut docs, 1, "set_name", rename("Two"));
        assert!(
            !second.is_error,
            "its own write is not news: {:?}",
            second.content
        );

        let other = json!({ "node": id, "name": "Theirs" });
        ok(&call(&mut engine, &mut docs, 2, "set_name", other));
        assert!(
            call(&mut engine, &mut docs, 1, "set_name", rename("Three")).is_error,
            "another client's write still refuses"
        );
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

    /// `run_exports` refuses a document with no settings, then renders a layer's
    /// settings as a named file it returns rather than writes.
    ///
    /// flip: `run_exports` dropping the `Content::File`s → fails at the file count.
    #[test]
    fn run_exports_returns_the_files_settings_ask_for() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Badge");
        assert!(call(&mut engine, &mut docs, 1, "run_exports", json!({})).is_error);
        let exports = ondin_core::verbs::example("[ExportSpec]").unwrap();
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_exports",
            json!({ "nodes": [id], "exports": exports }),
        ));
        let out = call(&mut engine, &mut docs, 1, "run_exports", json!({}));
        let listed = ok(&out)["files"].as_array().unwrap().clone();
        assert!(!listed.is_empty());
        assert_eq!(listed[0]["layer"], json!(id));
        let files: Vec<&String> = out
            .content
            .iter()
            .filter_map(|c| match c {
                Content::File { name, bytes, .. } if !bytes.is_empty() => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(files.len(), listed.len());
        assert!(files[0].contains("Badge"), "{files:?}");
    }

    /// `export_code` answers files and the three report lists.
    #[test]
    fn export_code_answers_files_and_the_report() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Card");
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "export_code",
            json!({ "target": "html", "ids": [id] }),
        );
        let report = ok(&out);
        assert!(report["files"].as_array().is_some_and(|f| !f.is_empty()));
        assert!(report.get("approximated").is_some());
        assert!(
            out.content
                .iter()
                .any(|c| matches!(c, Content::File { name, .. } if name.ends_with(".html")))
        );
        let bad = call(
            &mut engine,
            &mut docs,
            1,
            "export_code",
            json!({ "target": "svelte" }),
        );
        assert!(bad.is_error);
    }

    /// The three table reads list what the verbs made: a variable with its
    /// resolved value per mode, a style with its users, a main component with
    /// the properties it owns.
    ///
    /// flip: `components` listing properties for sets alone → fails at "a plain
    /// main's properties are listed".
    #[test]
    fn the_lists_show_variables_styles_and_components() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let cell = ondin_core::verbs::example("Cell").unwrap();
        let args = json!({ "kind": "color", "name": "brand/blue", "value": cell });
        ok(&call(&mut engine, &mut docs, 1, "add_variable", args));
        let vars = call(&mut engine, &mut docs, 1, "list_variables", json!({}));
        let v = &ok(&vars)["variables"][0];
        assert_eq!(v["name"], "brand/blue");
        let resolved = v["resolved"].as_object().unwrap();
        assert_eq!(resolved.values().next(), Some(&json!("#3366ccff")));

        let id = rect(&mut engine, &mut docs, 1, "Chip");
        let value = ondin_core::verbs::example("StyleValue").unwrap();
        let args = json!({ "value": value, "name": "Surface", "nodes": [id] });
        ok(&call(&mut engine, &mut docs, 1, "add_style", args));
        let styles = call(&mut engine, &mut docs, 1, "list_styles", json!({}));
        let s = &ok(&styles)["styles"][0];
        assert_eq!(s["name"], "Surface");
        assert_eq!(s["slot"], "fills");
        assert_eq!(s["users"], 1);

        // A rectangle becomes a main inside a frame of its own.
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "make_component",
            json!({ "nodes": [id] }),
        );
        let main = ok(&made)["created"][0].clone();
        // A main in no set owns its properties, which `set_instance_property`
        // then names.
        let args = json!({ "node": id, "kind": "boolean", "name": "Shown" });
        ok(&call(&mut engine, &mut docs, 1, "define_property", args));
        let comps = call(&mut engine, &mut docs, 1, "list_components", json!({}));
        let listed = ok(&comps)["components"].as_array().unwrap().clone();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["kind"], "main");
        assert_eq!(listed[0]["id"], main);
        assert!(
            listed[0]["properties"].to_string().contains("Shown"),
            "a plain main's properties are listed: {}",
            listed[0]
        );
    }

    /// A batch lands as one undo step named `batch`, a later step naming what an
    /// earlier one made by its label; a refused step leaves the document, the undo
    /// stack and the redo stack as they were.
    ///
    /// flip: `batch` not taking back the steps before a refusal → fails at
    /// "nothing of a refused batch stays".
    #[test]
    fn a_batch_is_one_step_and_lands_whole_or_not_at_all() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let make = json!({ "tool": "create_node", "as": "card",
            "args": { "parent": page, "kind": kind, "name": "Card" } });
        let name = json!({ "tool": "set_name", "args": { "node": "@card", "name": "Hero" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [make, name] }),
        );
        let card = ok(&out)["steps"][0]["created"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let id = NodeId::from_wire(&card).unwrap();
        let s = &docs.session;
        assert_eq!(s.doc.get(id).unwrap().name(), "Hero");
        assert_eq!(s.history.undo_depth(), 1, "one step");
        assert_eq!(s.history.done().last().unwrap().note.label, "batch");

        let layers = docs.session.doc.len();
        let bad = json!({ "tool": "set_name", "args": { "node": "@nobody", "name": "X" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [make, bad] }),
        );
        assert!(out.is_error);
        let s = &docs.session;
        assert_eq!(s.doc.len(), layers, "nothing of a refused batch stays");
        assert_eq!(s.history.undo_depth(), 1);
        assert_eq!(s.history.redo_depth(), 0, "and nothing to redo");
    }

    /// An instance reads back the property values it is set to and each copy the
    /// fields it overrides, in the words `set_instance_property` and
    /// `reset_overrides` take.
    ///
    /// flip: `component_summary` reading `overrides` off the instance root alone →
    /// fails at "the copy's opacity".
    #[test]
    fn an_instance_reads_back_its_properties_and_overrides() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Dot");
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "make_component",
            json!({ "nodes": [id] }),
        );
        let main = ok(&made)["created"][0].clone();
        let args = json!({ "node": id, "kind": "boolean", "name": "Shown" });
        ok(&call(&mut engine, &mut docs, 1, "define_property", args));
        let page = docs.session.doc.first_page().to_wire();
        let args = json!({ "main": main, "parent": page, "x": 500, "y": 500 });
        let made = call(&mut engine, &mut docs, 1, "create_instance", args);
        let instance = ok(&made)["created"][0].clone();
        let args = json!({ "node": instance, "property": "Shown", "value": false });
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_instance_property",
            args,
        ));

        let read = |engine: &mut Engine, docs: &mut One, id: &Value| {
            let out = call(engine, docs, 1, "get_node", json!({ "id": id }));
            ok(&out)["nodes"][0].clone()
        };
        let root = read(&mut engine, &mut docs, &instance);
        let props = &root["component"]["properties"];
        assert_eq!(props[0]["name"], "Shown", "{root}");
        assert_eq!(props[0]["kind"], "boolean");
        assert_eq!(props[0]["value"], false);
        assert_eq!(props[0]["overridden"], true);

        let copy = root["children"][0].clone();
        let args = json!({ "nodes": [copy], "opacity": 0.5 });
        ok(&call(&mut engine, &mut docs, 1, "set_opacity", args));
        let node = read(&mut engine, &mut docs, &copy);
        let overrides = node["component"]["overrides"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            overrides.contains(&json!("opacity")),
            "the copy's opacity: {overrides:?}"
        );
        assert!(overrides.contains(&json!("visible")), "{overrides:?}");
        let main_node = read(&mut engine, &mut docs, &main);
        assert!(main_node["component"].get("overrides").is_none());
    }

    /// `outline_text` turns a text the session laid out into a path, through the
    /// engine as an agent calls it.
    #[test]
    fn outline_text_turns_text_into_a_path() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Hello" }),
        );
        let text = ok(&made)["created"][0].clone();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "outline_text",
            json!({ "node": text }),
        );
        let path = NodeId::from_wire(ok(&out)["created"][0].as_str().unwrap()).unwrap();
        assert!(matches!(
            docs.session.doc.get(path).map(|n| n.kind()),
            Some(ondin_core::NodeKind::Path { .. })
        ));
    }

    /// A verb's line on what it did reaches the agent as `said`.
    ///
    /// flip: `write` dropping `done.said` → fails at the `said` assertion.
    #[test]
    fn a_verbs_said_line_reaches_the_agent() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Log in" }),
        ));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "replace_text",
            json!({ "find": "Log in", "replace": "Sign in" }),
        );
        assert_eq!(ok(&out)["said"], "Replaced 1 match in 1 layer");
    }

    /// A fill list of one solid colour, as `set_fills` takes it.
    fn solid(r: u8, g: u8, b: u8) -> Value {
        use ondin_core::peniko::{Brush, Color};
        serde_json::to_value(vec![ondin_core::Fill {
            brush: Brush::Solid(Color::from_rgb8(r, g, b)).into(),
            visible: true,
        }])
        .unwrap()
    }

    /// `list_fonts` needs no document and filters by name, ignoring case.
    #[test]
    fn list_fonts_filters_by_name() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "list_fonts",
            json!({ "query": "inter" }),
        );
        assert_eq!(ok(&out)["families"], json!(["Inter", "Inter Tight"]));
        assert_eq!(ok(&out)["matched"], 2);
    }

    /// `find_like` by fill finds the layers painted the same, the source with
    /// them, and refuses a layer with nothing to match.
    ///
    /// flip: `shares` comparing fill counts only → fails at the two-layer count.
    #[test]
    fn find_like_matches_by_fill() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        // One fill each, so only the colour tells C apart.
        let fills = solid(0, 128, 0);
        let ids: Vec<String> = ["A", "B", "C"]
            .iter()
            .map(|n| rect(&mut engine, &mut docs, 1, n))
            .collect();
        for id in &ids[..2] {
            ok(&call(
                &mut engine,
                &mut docs,
                1,
                "set_fills",
                json!({ "nodes": [id], "fills": fills }),
            ));
        }
        let red = solid(255, 0, 0);
        let set = call(
            &mut engine,
            &mut docs,
            1,
            "set_fills",
            json!({ "nodes": [ids[2]], "fills": red }),
        );
        ok(&set);
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "find_like",
            json!({ "id": ids[0], "by": "fill" }),
        );
        let found: Vec<&str> = ok(&out)["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap())
            .collect();
        assert_eq!(found, ["A", "B"]);
        let bare = rect(&mut engine, &mut docs, 1, "D");
        assert!(
            call(
                &mut engine,
                &mut docs,
                1,
                "find_like",
                json!({ "id": bare, "by": "stroke" })
            )
            .is_error
        );
    }

    /// `check_contrast` measures a text layer on the page and says whether it
    /// passes.
    #[test]
    fn check_contrast_measures_text() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Hello" }),
        ));
        let out = call(&mut engine, &mut docs, 1, "check_contrast", json!({}));
        let report = ok(&out);
        let findings = report["findings"].as_array().unwrap();
        assert!(
            findings.iter().any(|f| f["trail"] == json!(["Hello"])
                || f["trail"].as_array().is_some_and(|t| t.len() == 1)),
            "{report}"
        );
        assert_eq!(report["failing"], 0, "black on white passes: {report}");
        assert!(
            call(
                &mut engine,
                &mut docs,
                1,
                "check_contrast",
                json!({ "level": "x" })
            )
            .is_error
        );
    }

    /// `review_colours` groups two near-identical fills, and `merge_colours`
    /// makes them one, after which the review has no group.
    #[test]
    fn review_colours_groups_what_merge_colours_merges() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        for (name, r) in [("A", 0x80), ("B", 0x81)] {
            let id = rect(&mut engine, &mut docs, 1, name);
            ok(&call(
                &mut engine,
                &mut docs,
                1,
                "set_fills",
                json!({ "nodes": [id], "fills": solid(r, 0x33, 0x66) }),
            ));
        }
        let out = call(&mut engine, &mut docs, 1, "review_colours", json!({}));
        let groups = ok(&out)["groups"].as_array().unwrap().clone();
        assert_eq!(groups.len(), 1, "{groups:?}");
        let colours = groups[0]["colours"].as_array().unwrap();
        let (into, other) = (colours[0]["colour"].clone(), colours[1]["colour"].clone());
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "merge_colours",
            json!({ "colours": [other], "into": into }),
        ));
        let again = call(&mut engine, &mut docs, 1, "review_colours", json!({}));
        assert_eq!(ok(&again)["groups"], json!([]));
    }

    /// `import_tokens` adds a token file's variable in one undo step, and
    /// `export_tokens` writes it back out.
    ///
    /// flip: `import_tokens` skipping its commit → fails at the variable count.
    #[test]
    fn tokens_import_and_export() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let file = json!({ "brand": { "$type": "color", "$value": "#ff0000" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "import_tokens",
            json!({ "tokens": file }),
        );
        assert!(
            ok(&out)["added"].as_u64() >= Some(1),
            "{:?}",
            out.structured
        );
        assert!(
            docs.session
                .doc
                .variables()
                .iter()
                .any(|v| v.name == "brand")
        );
        assert_eq!(docs.session.history.undo_depth(), 1);
        let out = call(&mut engine, &mut docs, 1, "export_tokens", json!({}));
        let written = ok(&out)["tokens"].as_str().unwrap().to_owned();
        assert!(written.contains("\"brand\""), "{written}");
        assert!(
            call(
                &mut engine,
                &mut docs,
                1,
                "import_tokens",
                json!({ "tokens": "[1, 2]" })
            )
            .is_error
        );
    }

    /// A keyed client that closes its connection and comes back on another, as a
    /// harness starting a process per turn does, keeps its document, hears what
    /// the person changed in between as `changed`, and its `since` and `base_rev`
    /// still work. An unkeyed connection's state ends with it.
    ///
    /// flip: `Engine::closed` removing keyed state too → fails at the first call
    /// back, refused with *No document yet*.
    #[test]
    fn a_keyed_client_keeps_its_state_across_connections() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let k = Some("chat-1");
        ok(&keyed(
            &mut engine,
            &mut docs,
            1,
            k,
            "use_document",
            json!({ "document": "a" }),
        ));
        let page = docs.session.doc.first_page().to_wire();
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let made = keyed(
            &mut engine,
            &mut docs,
            1,
            k,
            "create_node",
            json!({ "parent": page, "kind": kind, "name": "Card" }),
        );
        let card = ok(&made)["created"][0].clone();
        let base = ok(&made)["rev"].clone();
        engine.closed(1);

        // The person renames it while no connection is open.
        open(&mut engine, &mut docs, 9);
        ok(&call(
            &mut engine,
            &mut docs,
            9,
            "set_name",
            json!({ "node": card, "name": "Hero" }),
        ));

        let back = keyed(&mut engine, &mut docs, 2, k, "get_selection", json!({}));
        let back = ok(&back);
        assert_eq!(
            back["changed"]["changed"][0]["id"], card,
            "the document is remembered, and the rename reported: {back}"
        );
        let since = keyed(
            &mut engine,
            &mut docs,
            2,
            k,
            "get_document",
            json!({ "since": base }),
        );
        assert_eq!(ok(&since)["since"], base, "answered as a delta");
        let stale = keyed(
            &mut engine,
            &mut docs,
            2,
            k,
            "set_name",
            json!({ "node": card, "name": "Mine", "base_rev": base }),
        );
        let Content::Text(why) = &stale.content[0] else {
            panic!("no text")
        };
        assert!(
            stale.is_error && why.contains("Card"),
            "refused by the name it read: {why}"
        );

        engine.closed(9);
        assert!(
            call(&mut engine, &mut docs, 10, "get_selection", json!({})).is_error,
            "a new unkeyed connection starts with no document"
        );
    }

    /// The proxy's restore of the document a keyed client already uses keeps its
    /// history and reports nothing, so the agent's own next call hears what
    /// changed; a plain `use_document` of it reports the change itself.
    ///
    /// flip: `use_document` clearing `shown` whatever document it names → fails
    /// at "the next call hears it".
    #[test]
    fn a_restore_leaves_the_change_for_the_agent() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let k = Some("chat-2");
        let use_doc = |restore: bool| json!({ "document": "a", "restore": restore });
        ok(&keyed(
            &mut engine,
            &mut docs,
            1,
            k,
            "use_document",
            use_doc(false),
        ));
        engine.closed(1);
        open(&mut engine, &mut docs, 9);
        rect(&mut engine, &mut docs, 9, "Badge");
        let restored = keyed(&mut engine, &mut docs, 2, k, "use_document", use_doc(true));
        assert!(ok(&restored).get("changed").is_none());
        let next = keyed(&mut engine, &mut docs, 2, k, "get_selection", json!({}));
        assert_eq!(
            ok(&next)["changed"]["added"][0]["name"],
            "Badge",
            "the next call hears it"
        );
        rect(&mut engine, &mut docs, 9, "Chip");
        let again = keyed(&mut engine, &mut docs, 2, k, "use_document", use_doc(false));
        assert_eq!(ok(&again)["changed"]["added"][0]["name"], "Chip");
    }

    /// Past [`CLIENTS`] keyed clients, the least recently used is forgotten.
    #[test]
    fn keyed_clients_are_bounded() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        for n in 0..=CLIENTS {
            let key = format!("k{n}");
            ok(&keyed(
                &mut engine,
                &mut docs,
                n as Conn,
                Some(&key),
                "use_document",
                json!({ "document": "a" }),
            ));
            engine.closed(n as Conn);
        }
        // The kept one first: asking for the forgotten one makes it anew,
        // forgetting the next oldest.
        ok(&keyed(
            &mut engine,
            &mut docs,
            99,
            Some("k1"),
            "get_selection",
            json!({}),
        ));
        assert!(
            keyed(
                &mut engine,
                &mut docs,
                99,
                Some("k0"),
                "get_selection",
                json!({})
            )
            .is_error,
            "the oldest was forgotten"
        );
    }

    /// Revisions start from the launch time, not from 1, so a `rev` another
    /// process handed out does not match one here; they stay exact in JavaScript.
    ///
    /// flip: `revision_seed` answering 1 → fails at "not a small counter".
    #[test]
    fn revisions_start_from_the_launch_time() {
        let rev = crate::session::next_revision();
        assert!(rev > 1_700_000_000_000_000, "not a small counter: {rev}");
        assert!(rev < 1 << 53);
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
