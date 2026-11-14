//! The MCP host (§8): the tool handlers, run on the thread that owns the
//! documents, the window's or `serve`'s.
//!
//! [`Engine`] keeps each connection's current document and what it last showed
//! that connection, and answers each call against an [`EditorSession`] lent by
//! the host through [`Documents`]. Writes go through `EditorSession::try_commit`,
//! the door the app's own edits use.

pub(crate) mod access;
mod actions;
mod agent_log;
mod context;
pub(crate) mod endpoint;
mod inspect;
mod kits;
pub(crate) mod propose;
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
use serde_json::{Map, Value, json};
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
    /// window does a tab behind. An error is why the edit is not on disk; a
    /// host that closed the document with it has dropped the edit too.
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
    /// A tool that may write is about to run on the document at `path`: the
    /// window ends the person's text edit there first, so the typing lands as
    /// its own step rather than being lost under the agent's.
    fn before_write(&mut self, _path: &Path) {}
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

/// How many of a connection's latest looks keep every layer's fingerprint, for
/// `since` and for naming what changed.
const KEPT: usize = 8;

/// How many of a connection's looks `base_rev` can be checked against. A look
/// past [`KEPT`] holds only the ids others changed, so an agent writing many
/// times off one read is not sent back to read again.
const LOOKS: usize = 1024;

/// What a connection was shown: a revision and, for the latest [`KEPT`], each
/// layer's fingerprint.
struct Shown {
    rev: u64,
    layers: Option<HashMap<String, Layer>>,
    /// The layers changed or removed since the look before by anyone but this
    /// client, so `base_rev` does not count the client's own steps against it.
    others: Vec<String>,
}

/// A layer as a connection saw it.
#[derive(Clone, PartialEq)]
struct Layer {
    name: String,
    /// `None` on a page's root.
    parent: Option<String>,
    hash: u64,
}

/// The most layers one list of a `changed` report names. One deletion of a
/// frame lists every layer in it, which buried the answer it rode on; past
/// this, the report counts the rest and names the top-level layers they sit
/// in, which an agent reads again.
const CHANGED_LISTED: usize = 20;

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

/// How many proposals a host keeps for the person to apply; the oldest goes
/// first.
const PROPOSALS: usize = 64;

/// The tool handlers and every client's state.
#[derive(Default)]
pub(crate) struct Engine {
    clients: HashMap<Who, Connection>,
    /// The connections open now, which `serve` waits on.
    live: std::collections::HashSet<Conn>,
    tick: u64,
    /// The levels the window granted its chats.
    grants: access::Grants,
    /// What the chats proposed, newest last.
    proposals: VecDeque<propose::Proposal>,
    /// The last proposal's id.
    proposed: u64,
    /// Where a granted chat's calls are written.
    log: agent_log::Log,
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

    /// Each id with the name it has (or, removed, had), at most [`CHANGED_LISTED`] a
    /// list. Past that, `left_out` counts the rest of each list and names, in
    /// `in`, the top-level layers holding them (at most [`CHANGED_LISTED`] too).
    fn to_json(&self, now: &HashMap<String, Layer>, then: &HashMap<String, Layer>) -> Value {
        let name = |l: &Layer| json!(l.name);
        let named = |ids: &[String], from: &HashMap<String, Layer>| -> Vec<Value> {
            ids.iter()
                .take(CHANGED_LISTED)
                .map(|id| json!({ "id": id, "name": from.get(id).map_or(json!(""), name) }))
                .collect()
        };
        let mut report = json!({
            "added": named(&self.added, now),
            "changed": named(&self.changed, now),
            "removed": named(&self.removed, then),
        });
        let past = |ids: &[String]| ids.len().saturating_sub(CHANGED_LISTED);
        if [&self.added, &self.changed, &self.removed]
            .iter()
            .all(|ids| past(ids) == 0)
        {
            return report;
        }
        let mut sections: Vec<String> = Vec::new();
        let left = [
            (&self.added, now),
            (&self.changed, now),
            (&self.removed, then),
        ];
        for (ids, from) in left {
            for id in ids.iter().skip(CHANGED_LISTED) {
                if let Some(top) = top_level(id, from)
                    && !sections.contains(&top)
                {
                    sections.push(top);
                }
            }
        }
        sections.sort();
        let sections: Vec<Value> = sections
            .iter()
            .take(CHANGED_LISTED)
            .map(|id| {
                let layer = now.get(id).or_else(|| then.get(id));
                json!({ "id": id, "name": layer.map_or(json!(""), name) })
            })
            .collect();
        report["left_out"] = json!({
            "added": past(&self.added),
            "changed": past(&self.changed),
            "removed": past(&self.removed),
            "in": sections,
        });
        report
    }
}

/// The layer of `from` that holds `id` (or is it) directly under a page.
fn top_level(id: &str, from: &HashMap<String, Layer>) -> Option<String> {
    let mut at = id;
    loop {
        let parent = from.get(at)?.parent.as_deref()?;
        if from.get(parent)?.parent.is_none() {
            return Some(at.to_owned());
        }
        at = parent;
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
                    parent: n.parent.clone(),
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

    /// The levels granted to the window's chats, shared with its endpoint.
    pub(crate) fn grants(&self) -> access::Grants {
        self.grants.clone()
    }

    /// The proposal called `id`, while it is among the last [`PROPOSALS`].
    pub(crate) fn proposal(&self, id: u64) -> Option<&propose::Proposal> {
        self.proposals.iter().find(|p| p.id == id)
    }

    /// Answer one call. A chat granted a level, by its key or by the process
    /// calling, acts under the chat's key, is refused a tool above its level,
    /// and has the call written to the agent log. A chat's key this window
    /// holds no grant for (a chat of an earlier run, or one ended long ago) is
    /// refused rather than answered as an ungranted client's.
    pub(crate) fn handle(&mut self, docs: &mut impl Documents, mut call: Call) -> Output {
        parse_json_text(&mut call);
        let Some((key, level)) = self.grants.resolve(call.key.as_deref(), call.pid) else {
            if call
                .key
                .as_deref()
                .is_some_and(crate::assistant::is_client_key)
            {
                return Output::error(access::CHAT_ENDED);
            }
            return self.answer(docs, call);
        };
        call.key = Some(key.clone());
        let tool = match call.tool.as_str() {
            "call" => call.args.get("tool").and_then(Value::as_str),
            tool => Some(tool),
        }
        .unwrap_or("call")
        .to_owned();
        let out = match level.map(|level| access::refused(level, &call)) {
            None => Output::error(access::CHAT_ENDED),
            Some(Some(why)) => Output::error(why),
            Some(None) => self.answer(docs, call),
        };
        let document = self
            .clients
            .get(&Who::Key(key.clone()))
            .and_then(|c| c.document.clone());
        self.log
            .write(&tool, document.as_deref(), &key, !out.is_error);
        out
    }

    /// Answer one call that its level allows.
    fn answer(&mut self, docs: &mut impl Documents, call: Call) -> Output {
        // `call` is the named tool's call itself, so it is checked and answered
        // as that tool, listed or not.
        let mut call = call;
        if call.tool == "call" {
            let Some(tool) = call.args.get("tool").and_then(Value::as_str) else {
                return Output::error("`call` needs `tool`, a tool's name");
            };
            if tool == "call" {
                return Output::error("`call` runs another tool, not itself");
            }
            let args = call.args.get("args").cloned().unwrap_or_else(|| json!({}));
            call.tool = tool.to_owned();
            call.args = args;
        }
        if let Err(why) = check_call(&call) {
            return Output::error(why);
        }
        // Ids start from the launch's time, so a card saved in a thread before
        // a restart never names a proposal made after it.
        if self.proposed == 0 {
            self.proposed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_millis() as u64 * 1000);
        }
        let next = self.proposed + 1;
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
                        remember(conn, rev, layers(&snap), Vec::new());
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
        if call.tool == "describe_tools" {
            return Output::json(describe_tools(&call.args));
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
        // draw every replaced layer in a fallback. Written in the host's spelling,
        // whether the call is `replace_font` or one step of a batch or proposal.
        let fonts_checked = match call.tool.as_str() {
            "replace_font" => host_font(docs, &mut call.args),
            "batch" | "propose_edit" => {
                // A batch's steps name their `tool`, a proposal's calls their `verb`.
                let (list, name) = match call.tool == "batch" {
                    true => ("steps", "tool"),
                    false => ("calls", "verb"),
                };
                let steps = call.args.get_mut(list).and_then(Value::as_array_mut);
                steps.into_iter().flatten().try_for_each(|step| {
                    match step.get(name).and_then(Value::as_str) {
                        Some("replace_font") => match step.get_mut("args") {
                            Some(args) => host_font(docs, args),
                            None => Ok(()),
                        },
                        _ => Ok(()),
                    }
                })
            }
            _ => Ok(()),
        };
        if let Err(why) = fonts_checked {
            return Output::error(why);
        }
        if ondin_mcp::schema::class(&call.tool) != ondin_mcp::schema::Class::Read {
            docs.before_write(&path);
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
        let mark = session.mark();
        let mut proposed = None;
        let mut held_redo = None;
        let answer = if call.tool == "propose_edit" {
            let title = call.args["title"].as_str().unwrap_or_default();
            propose::calls(&call.args)
                .and_then(|calls| propose::check(session, &call.client, title, calls))
                .map(|mut p| {
                    p.id = next;
                    p.key.clone_from(&call.key);
                    p.document.clone_from(&path);
                    let answer = json!({ "proposal": p.id, "changes": p.changes });
                    proposed = Some(p);
                    Answer::Json(answer)
                })
                .map_err(|why| format!("{why}. Nothing was proposed."))
        } else if kits::is_kit_tool(&call.tool) {
            kits::run(docs, &path, &call)
        } else if actions::TOOLS.contains(&call.tool.as_str()) {
            actions::run(docs, &path, &call)
        } else if call.tool == "get_view" {
            Ok(Answer::Json(view(session, canvas)))
        } else if matches!(call.tool.as_str(), "batch" | "set") {
            // Each step's commit clears the person's redo; one that ends refused,
            // or lands nothing, gives it back below.
            held_redo = Some(session.history.take_redo());
            run(conn, session, &call)
        } else {
            run(conn, session, &call)
        };
        let Some(session) = docs.session(&path) else {
            return Output::error(format!("{} closed during the call", path.display()));
        };
        // A refused `batch` or `set` takes back the steps it made, and leaves the
        // revision where it was, so no client sees a change that is not there.
        // Only these two: they change the document through the history alone,
        // which is what `rewind` checks.
        if answer.is_err() && matches!(call.tool.as_str(), "batch" | "set") {
            session.rewind(&mark);
        }
        if let Some(redo) = held_redo
            && (answer.is_err() || session.revision() == before)
        {
            session.history.restore_redo(redo);
        }
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
            let own = last_layers(conn).map(|then| delta(then, &now).to_json(&now, then));
            remember(conn, after, now, Vec::new());
            if matches!(call.tool.as_str(), "undo" | "redo")
                && let (Some(own), Some(Value::Object(map))) = (own, output.structured.as_mut())
            {
                map.insert("layers".into(), own);
            }
            if let Err(why) = docs.committed(&path) {
                // A host that dropped its copy rather than write it (`serve`,
                // finding the file changed outside) holds the edit nowhere.
                if docs.session(&path).is_none() {
                    return Output::error(why);
                }
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
        if let Some(p) = proposed {
            self.proposed = p.id;
            self.proposals.push_back(p);
            if self.proposals.len() > PROPOSALS {
                self.proposals.pop_front();
            }
        }
        output
    }
}

/// `replace_font`'s `to`, refused unless a family the host can set text in, and
/// written in the host's spelling.
fn host_font(docs: &mut impl Documents, args: &mut Value) -> Result<(), String> {
    let Some(to) = args.get("to").and_then(Value::as_str) else {
        return Ok(());
    };
    let to = to.trim().to_owned();
    let key = to.to_lowercase();
    match docs.fonts().into_iter().find(|f| f.to_lowercase() == key) {
        Some(family) => {
            args["to"] = json!(family);
            Ok(())
        }
        None => Err(format!(
            "`to`: no family called {to} can be set here; `list_fonts` names those that can"
        )),
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
        return Err(no_such_tool(&call.tool));
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

/// Parse the model parameters some clients send as JSON text, `call`'s own
/// `args` among them, so the level check and the handler read what the agent
/// meant; a verb's own `Args` does the same.
fn parse_json_text(call: &mut Call) {
    let parse = |map: &mut serde_json::Map<String, Value>, name: &str| {
        if let Some(Value::String(s)) = map.get(name)
            && let Ok(parsed) = serde_json::from_str::<Value>(s.trim())
            && (parsed.is_object() || parsed.is_array())
        {
            map.insert(name.to_owned(), parsed);
        }
    };
    let (tool, args) = if call.tool == "call" {
        let Some(map) = call.args.as_object_mut() else {
            return;
        };
        parse(map, "args");
        let Some(tool) = map.get("tool").and_then(Value::as_str).map(str::to_owned) else {
            return;
        };
        let Some(args) = map.get_mut("args") else {
            return;
        };
        (tool, args)
    } else {
        (call.tool.clone(), &mut call.args)
    };
    if ondin_core::verbs::find(&tool).is_some() {
        return;
    }
    let (Some(tool), Some(map)) = (ondin_mcp::schema::tool(&tool), args.as_object_mut()) else {
        return;
    };
    for p in tool.params {
        if let ondin_core::verbs::ParamKind::Model(model) = p.kind
            && !matches!(model, "PropValue" | "DtcgFile")
        {
            parse(map, p.name);
        }
    }
}

/// Remember a look at `rev`, with the layers `others` changed or removed since
/// the look before.
fn remember(conn: &mut Connection, rev: u64, layers: HashMap<String, Layer>, others: Vec<String>) {
    if conn.shown.back().is_some_and(|s| s.rev == rev) {
        return;
    }
    conn.shown.push_back(Shown {
        rev,
        layers: Some(layers),
        others,
    });
    if let Some(old) = conn.shown.len().checked_sub(KEPT + 1) {
        conn.shown[old].layers = None;
    }
    while conn.shown.len() > LOOKS {
        conn.shown.pop_front();
    }
}

/// The fingerprints of the connection's last look.
fn last_layers(conn: &Connection) -> Option<&HashMap<String, Layer>> {
    conn.shown.back().and_then(|s| s.layers.as_ref())
}

/// What changed since the connection's last look, and remember this one.
fn catch_up(conn: &mut Connection, session: &EditorSession) -> Option<Value> {
    let rev = session.revision();
    if conn.shown.back()?.rev == rev {
        return None;
    }
    let now = layers(&snapshot_of(session));
    let (report, others) = match last_layers(conn) {
        Some(last) => {
            let d = delta(last, &now);
            let report = (!d.is_empty()).then(|| d.to_json(&now, last));
            (report, d.changed.into_iter().chain(d.removed).collect())
        }
        None => (None, Vec::new()),
    };
    remember(conn, rev, now, others);
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

/// `export_png`'s `region`, `[x, y, w, h]` in page coordinates; `None` when
/// absent. One with no area is refused: it has nothing to draw.
fn region(args: &Value) -> Result<Option<ondin_core::kurbo::Rect>, String> {
    let Some(v) = args.get("region").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let n: Vec<f64> = v
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    match n[..] {
        [x, y, w, h] if w > 0.0 && h > 0.0 => {
            Ok(Some(ondin_core::kurbo::Rect::new(x, y, x + w, y + h)))
        }
        [_, _, _, _] => Err("`region` needs a width and a height above zero".into()),
        _ => Err("`region` is [x, y, w, h]".into()),
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
    "reach the part you need with `find_nodes` or `get_outline`, then `get_node` on it";

/// The most `export_png`'s `max` is taken at: a raster held in the person's
/// window, where an unbounded one could exhaust its memory and lose every tab.
const PNG_CEILING: u64 = 4096;

/// The viewport `export_png` draws `view` through, refused when it would pass
/// `max` pixels: the renderer floors its scale, so a huge box would otherwise
/// draw a raster of gigabytes on the person's UI thread.
fn png_viewport(
    what: &str,
    view: ondin_core::kurbo::Rect,
    scale: f64,
    max: f64,
) -> Result<ondin_render::Viewport, String> {
    let vp = ondin_export::png::viewport_at(view, scale);
    let side = f64::from(vp.pixel_size.0.max(vp.pixel_size.1));
    if !view.width().is_finite() || !view.height().is_finite() || side > max.ceil() + 1.0 {
        return Err(format!(
            "{what} is too large to draw within `max` pixels; export a smaller part"
        ));
    }
    Ok(vp)
}

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
                    "`{}` is a page: `get_outline` names its layers, each a tree",
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
            let mut answer = json!({ "nodes": nodes });
            // Below the root, a layer's page and frame are the root's or the
            // root itself: written once, on the root.
            if let Some(Value::Array(list)) = answer.get_mut("nodes")
                && let Some((root, below)) = list.split_first_mut()
            {
                let frame = root.get("artboard").cloned();
                for n in below.iter_mut().filter_map(Value::as_object_mut) {
                    n.remove("page");
                    if n.get("artboard") == frame.as_ref() || n.get("artboard") == root.get("id") {
                        n.remove("artboard");
                    }
                }
            }
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
        "get_outline" => {
            let root = match node(args, "id")? {
                Some(id) if s.doc.get(id).is_none() => {
                    return Err(format!("no layer `{}`", id.to_wire()));
                }
                Some(id) => id,
                None => s.page,
            };
            let depth = index(args, "depth").unwrap_or(2);
            let limit = index(args, "limit").unwrap_or(200) as usize;
            let answer = reads::outline(s, root, depth, limit);
            within_budget(
                &answer.to_string(),
                "This outline",
                "pass a smaller `depth` or `limit`, or outline one layer with `id`",
            )?;
            json(answer)
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
                let mut entry = json!({ "id": n.id, "name": n.name, "kind": n.kind, "path": path });
                if let Some(id) = NodeId::from_wire(&n.id) {
                    entry["box"] = reads::bounds_box(s, id);
                    if s.selection
                        .ids()
                        .iter()
                        .any(|sel| reads::within(&s.doc, *sel, id))
                    {
                        entry["selected"] = json!(true);
                    }
                }
                found.push(entry);
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
            if let Some(region) = region(args)? {
                if let Some(id) = id.filter(|id| !is_page(s, *id)) {
                    return Err(format!(
                        "`{}` is a layer: `region` is a rectangle of a page, so pass the page \
                         or no `id`; or export the layer alone without `region`",
                        id.to_wire()
                    ));
                }
                let page = page_of(s, id)?;
                let scale = (max / region.width().max(region.height())).min(2.0);
                let vp = png_viewport("`region`", region, scale, max)?;
                return Ok(Answer::Image(ondin_export::png::png(
                    &s.doc,
                    &s.resolved,
                    &vp,
                    page,
                )));
            }
            let png = match id {
                Some(id) if !is_page(s, id) => {
                    // The box `raster_of` frames: the ink, effects and unclipped
                    // children included, so `max` bounds what is drawn.
                    let r = ondin_export::extent(&s.resolved, &[id])
                        .ok_or_else(|| format!("`{}` has no bounds", id.to_wire()))?;
                    let scale = (max / r.width().max(r.height()).max(1.0)).min(2.0);
                    png_viewport("The layer", r, scale, max)?;
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
                    let vp = png_viewport("The page", view, scale, max)?;
                    ondin_export::png::png(&s.doc, &s.resolved, &vp, page)
                }
            };
            Ok(Answer::Image(png))
        }
        "undo" | "redo" if s.compare.is_some() => Err(READ_ONLY.to_owned()),
        "undo" => {
            let step = own_step(s.history.done().next_back(), &call.client, "undo")?;
            json(json!({ "undone": s.undo().then_some(step).flatten() }))
        }
        "redo" => {
            let step = own_step(s.history.undone().next(), &call.client, "redo")?;
            json(json!({ "redone": s.redo().then_some(step).flatten() }))
        }
        "run_exports" => reads::run_exports(s, nodes(args, "ids")?),
        "export_code" => reads::export_code(s, args, nodes(args, "ids")?),
        "list_components" => json(reads::components(&s.doc)),
        "list_variables" => json(reads::variables(&s.doc)),
        "list_styles" => json(reads::styles(&s.doc)),
        tool if inspect::TOOLS.contains(&tool) => inspect::run(s, tool, args),
        tool if context::TOOLS.contains(&tool) => context::run(s, tool, args),
        "batch" => batch(conn, s, &call.client, args),
        "set" => set(conn, s, &call.client, args),
        tool => write(conn, s, &call.client, tool, args),
    }
}

/// Refuse an agent's `undo` or `redo` of a step it did not make: the history is
/// the person's too, and one agent call must not take back their work or another
/// client's. A step is this client's when it came over MCP under its name.
/// Answers the step's name as the History panel lists it, `None` at the end of
/// the history.
fn own_step(
    step: Option<ondin_core::StepView<'_>>,
    client: &str,
    verb: &str,
) -> Result<Option<String>, String> {
    let Some(step) = step else {
        return Ok(None);
    };
    match &step.note.origin {
        ondin_core::Origin::Mcp { client: c, .. } if c == client => {
            Ok(Some(step.note.label.clone()))
        }
        ondin_core::Origin::Mcp { client: c, .. } => Err(format!(
            "The step to {verb} is `{}`, made by {c}, not by you: nothing was changed",
            step.note.label
        )),
        ondin_core::Origin::User => Err(format!(
            "The step to {verb} is the person's `{}`: nothing was changed. Ask them, or reverse your own edit with a write",
            step.note.label
        )),
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
    let Some(then) = conn
        .shown
        .iter()
        .find(|s| s.rev == since)
        .and_then(|s| s.layers.as_ref())
    else {
        return Ok(None);
    };
    let now = layers(snap);
    let d = delta(then, &now);
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
    // A layer that was there before is not one the call created: `make_component`
    // answers a frame it turned into a main as `main` alone.
    let created: Vec<Value> = done
        .created
        .iter()
        .filter(|id| s.doc.get(**id).is_none())
        .map(|id| json!(id.to_wire()))
        .collect();
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
    let mut answer = json!({ "created": created });
    if let Some(made) = done.made {
        answer[made_key(verb.name)] = match verb.name {
            "define_property" => json!(property_name(s, &args, &made).unwrap_or(made)),
            _ => json!(made),
        };
    }
    if verb.name == "make_component"
        && let Some(main) = done.created.first()
    {
        answer["main"] = json!(main.to_wire());
    }
    if let Some(said) = done.said {
        answer["said"] = json!(said);
    }
    if verb.name == "insert_subtree"
        && let Some(tree) = args.get("tree")
    {
        let mut named = Map::new();
        named_paths(tree, "", &mut done.created.iter(), &mut named);
        // Each id once: `created` keeps the root, which a batch label names,
        // and the layers no name reaches.
        let ids: std::collections::HashSet<&Value> = named.values().collect();
        let created: Vec<Value> = done
            .created
            .iter()
            .enumerate()
            .map(|(i, id)| (i, json!(id.to_wire())))
            .filter(|(i, id)| *i == 0 || !ids.contains(id))
            .map(|(_, id)| id)
            .collect();
        answer["created"] = json!(created);
        answer["named"] = Value::Object(named);
    }
    if let Some(px) = pixels {
        answer["pixel_size"] = json!(px);
    }
    // A move keeps where the layer is drawn, so its place in the new parent is
    // new to the caller, and the next write that places it needs it.
    // Measured in the parent's own space, as `set`'s `at` places it, so a
    // rotated or scaled parent answers the place `at` takes back.
    if verb.name == "reparent"
        && let Ok(Some(id)) = node(&args, "node")
        && let Some(corner) = ondin_core::edit::box_corner(&s.doc, &s.resolved, id)
    {
        let r = |v: f64| (v * 10.0).round() / 10.0;
        answer = json!({ "at": [r(corner.x), r(corner.y)] });
    }
    Ok(Answer::Json(answer))
}

/// The key a write answers what else it made under, named for what it is.
fn made_key(verb: &str) -> &'static str {
    match verb {
        "add_image" => "image",
        "add_variable" => "variable",
        "add_mode" => "mode",
        "add_collection" | "duplicate_collection" => "collection",
        "add_style" => "style",
        "define_property" => "property",
        _ => "made",
    }
}

/// The name of the property `define_property` just made, whose id is `item`:
/// `set_instance_property` takes a property by its name.
fn property_name(s: &EditorSession, args: &Value, item: &str) -> Option<String> {
    let owner = ondin_core::variant::owner_above(&s.doc, node(args, "node").ok()??)?;
    let props = s.doc.get(owner)?.props();
    props
        .iter()
        .find(|p| p.id.0.to_wire() == item)
        .map(|p| p.name.clone())
}

/// Each layer of `tree` the caller named, by the `/`-joined names of it and its
/// named ancestors, with the id `created` minted for it: `insert_subtree` mints
/// in the tree's order, each layer before its children.
fn named_paths<'a>(
    tree: &Value,
    above: &str,
    created: &mut impl Iterator<Item = &'a NodeId>,
    named: &mut Map<String, Value>,
) {
    let Some(id) = created.next() else {
        return;
    };
    let path = match tree.get("name").and_then(Value::as_str) {
        Some(name) => {
            let path = match above {
                "" => name.to_owned(),
                _ => format!("{above}/{name}"),
            };
            named
                .entry(path.clone())
                .or_insert_with(|| json!(id.to_wire()));
            path
        }
        None => above.to_owned(),
    };
    for child in tree
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        named_paths(child, &path, created, named);
    }
}

/// `batch`: each step's verb or `set` committed in turn, each reading what the last left,
/// then joined into one undo step named `batch`. A refused step takes back the
/// steps before it, so the batch lands whole or not at all. A step names what an
/// earlier one made as `"@label"`, the label that step gave in `as`, as
/// `ondin script` does, and a layer an `insert_subtree` step named by
/// `"@label/Path"`, its path of names below the root; `base_rev` holds for every
/// step.
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
                .or_else(|| answer.get("main"))
                .or_else(|| answer.get(made_key(tool)))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if let Some(first) = first {
                labels.insert(label.to_owned(), first);
            }
            // `insert_subtree`'s layers by `label/Path`, the path below the root.
            let root = step["args"]["tree"]["name"].as_str();
            for (path, id) in answer
                .get("named")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let below = match root {
                    Some(root) => path.strip_prefix(root).and_then(|p| p.strip_prefix('/')),
                    None => Some(path.as_str()),
                };
                if let (Some(below), Some(id)) = (below, id.as_str()) {
                    labels.insert(format!("{label}/{below}"), id.to_owned());
                }
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

/// What `set` writes for each property, in the order it writes them: the setter
/// verb, whether it takes the layers at once as `nodes` (else one `node` per
/// call), and the name it takes the value under.
const SET_PROPS: &[(&str, &str, bool, &str)] = &[
    ("name", "set_name", false, "name"),
    ("visible", "set_visible", true, "visible"),
    ("locked", "set_locked", true, "locked"),
    ("opacity", "set_opacity", true, "opacity"),
    ("blend", "set_blend", true, "blend"),
    ("clip", "set_clip", true, "clip"),
    ("geometry", "set_geometry", false, "geometry"),
    ("text", "set_text", false, "text"),
    ("text_style", "set_text_style", true, "style"),
    ("paragraph", "set_paragraph_style", true, "paragraph"),
    ("block", "set_block_style", true, "block"),
    ("fills", "set_fills", true, "fills"),
    ("strokes", "set_strokes", true, "strokes"),
    ("effects", "set_effects", true, "effects"),
    ("exports", "set_exports", true, "exports"),
    ("display", "set_display", true, "display"),
    ("item", "set_layout_item", true, "item"),
    ("insets", "set_insets", true, "insets"),
    ("element", "set_element", false, "element"),
    ("link", "set_link", false, "href"),
    ("transform", "set_transform", false, "transform"),
];

/// The names agents write for what `set` takes otherwise: `x`, `y` for `at`,
/// `width`, `height` for `size`, `corner_radius` for `geometry`, `fill` and
/// `stroke` for one paint, and `rotation`, which no setter takes.
const SET_ALIASES: [&str; 8] = [
    "x",
    "y",
    "width",
    "height",
    "corner_radius",
    "rotation",
    "fill",
    "stroke",
];

/// Guesses at a `set` property no spelling slip reaches, with the one each means.
const SET_GUESSES: &[(&str, &str)] = &[
    ("color", "fill"),
    ("colour", "fill"),
    ("background", "fill"),
    ("border", "stroke"),
    ("radius", "corner_radius"),
    ("border_radius", "corner_radius"),
    ("hidden", "visible"),
    ("font", "text_style"),
    ("font_size", "text_style"),
    ("content", "text"),
    ("characters", "text"),
    ("angle", "rotation"),
    ("position", "at"),
    ("shadow", "effects"),
];

/// `set`: each property through its own setter, as a [`batch`] named `set`, so
/// one call writes what a handful of setters would and lands as one undo step.
/// `size`, `rotation` and `at` follow the rest in that order, each measured once
/// the ones before have landed, so a move lands where the new size and turn
/// leave the box ([`placed_at`]). New fills on a text clear its style's colour,
/// which would draw over them; a call that changes nothing answers `unchanged`.
fn set(
    conn: &Connection,
    s: &mut EditorSession,
    client: &str,
    args: &Value,
) -> Result<Answer, String> {
    let ids = nodes(args, "nodes")?.unwrap_or_default();
    if ids.is_empty() {
        return Err("`nodes` names no layer".into());
    }
    let Some(Value::Object(props)) = args.get("props") else {
        return Err("`props` is an object of properties".into());
    };
    let known: Vec<&str> = SET_PROPS
        .iter()
        .map(|p| p.0)
        .chain(["size", "at"])
        .chain(SET_ALIASES)
        .collect();
    if let Some(k) = props.keys().find(|k| !known.contains(&k.as_str())) {
        let near = nearest_name(k, &known)
            .map(|n| format!(" Did you mean `{n}`?"))
            .unwrap_or_default();
        return Err(format!(
            "`set` sets no `{k}`; it sets {}.{near}",
            known.join(", ")
        ));
    }
    let canon = canonical(props)?;
    if canon.contains_key("text") {
        text_targets(s, &ids)?;
    }
    let size = sides(
        props,
        "size",
        ["width", "height"],
        "`size` is {width, height}",
    )?;
    let at = sides(
        props,
        "at",
        ["x", "y"],
        "`at` is [x, y], the left and top of the layer's box in its parent",
    )?;
    // A layer its container lays out has no place of its own to set; the move
    // would be dropped and the call read as done.
    if at.is_some()
        && let Some(id) = ids
            .iter()
            .find(|id| ondin_core::build::is_flex_item(&s.doc, **id))
    {
        return Err(format!(
            "`{}` is placed by its container's layout, so `at`, `x` and `y` cannot move it: \
             reorder it with `reorder`, or pin it to the frame's sides with `set_insets`, \
             which takes it out of the layout",
            id.to_wire()
        ));
    }
    let rotation = match props.get("rotation") {
        None => None,
        Some(v) => Some(
            v.as_f64()
                .ok_or("`rotation` is the layer's angle in degrees, clockwise")?,
        ),
    };
    let wire: Vec<Value> = ids.iter().map(|id| json!(id.to_wire())).collect();
    let mut steps = Vec::new();
    let mut step = |tool: &str, args: Value| steps.push(json!({ "tool": tool, "args": args }));
    for (prop, verb, all, param) in SET_PROPS {
        let Some(v) = canon.get(*prop) else { continue };
        let with = |key: &str, layers: Value| {
            let mut m = Map::new();
            m.insert(key.into(), layers);
            m.insert((*param).into(), v.clone());
            Value::Object(m)
        };
        if *all {
            step(verb, with("nodes", json!(wire)));
        } else {
            for id in &wire {
                step(verb, with("node", id.clone()));
            }
        }
    }
    // A text draws in its style's colour over its fills, so new fills clear it.
    let given_colour = canon
        .get("text_style")
        .is_some_and(|st| st.get("color").is_some());
    // Over every text the fills reach, through groups, as `set_fills` writes.
    if canon.contains_key("fills") && !given_colour {
        let coloured: Vec<Value> = ondin_core::build::coloured_texts(&s.doc, &ids)
            .iter()
            .map(|id| json!(id.to_wire()))
            .collect();
        if !coloured.is_empty() {
            step(
                "set_text_style",
                json!({ "nodes": coloured, "style": { "color": null } }),
            );
        }
    }
    let batched = |steps: Vec<Value>| {
        let mut batched = json!({ "steps": steps });
        if let Some(base) = args.get("base_rev") {
            batched["base_rev"] = base.clone();
        }
        batched
    };
    let base = match args.get("base_rev").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(v.as_u64().ok_or("`base_rev` must be a `rev`")?),
    };
    let depth = s.history.undo_depth();
    let landed = (|| {
        if !steps.is_empty() {
            batch(conn, s, client, &batched(steps))?;
        }
        // Each measured after the ones before landed: a side kept is the side
        // the other properties left, and `at` places the box the size and turn left.
        if let Some([w, h]) = size {
            let steps = ids
                .iter()
                .map(|id| sized(s, *id, w, h))
                .collect::<Result<Vec<_>, _>>()?;
            batch(conn, s, client, &batched(steps))?;
        }
        if let Some(degrees) = rotation {
            for id in &ids {
                let turned =
                    ondin_core::edit::turned_to(&s.doc, &s.resolved, *id, degrees.to_radians());
                let Some(tx) = turned else {
                    continue;
                };
                refuse_kit_vault(s, &json!({ "node": id.to_wire() }))?;
                if let Some(base) = base {
                    stale(conn, s, base, &tx)?;
                }
                commit(s, client, "set", tx)?;
            }
        }
        if let Some([x, y]) = at {
            let steps = ids
                .iter()
                .map(|id| placed_at(s, *id, x, y))
                .collect::<Result<Vec<_>, _>>()?;
            batch(conn, s, client, &batched(steps))?;
        }
        Ok::<(), String>(())
    })();
    if let Err(why) = landed {
        while s.history.undo_depth() > depth && s.take_back() {}
        return Err(why);
    }
    let made = s.history.undo_depth().saturating_sub(depth);
    if made > 1 {
        s.history.join_last(made);
    }
    let mut written: Vec<&str> = props.keys().map(String::as_str).collect();
    written.sort_unstable();
    // Named by what it set, as a setter's step is named by its tool.
    if made > 0
        && let Some(note) = s.history.current_note_mut()
    {
        note.label = format!("set {}", written.join(", "));
        note.origin = ondin_core::Origin::Mcp {
            client: client.to_owned(),
            tool: "set".into(),
        };
    }
    if made == 0 {
        // A write that lands nothing must not read as one that did.
        return Ok(Answer::Json(
            json!({ "set": [], "unchanged": written, "note": "the layers already had these values" }),
        ));
    }
    Ok(Answer::Json(json!({ "set": written })))
}

/// The `set_transform` step that puts the left and top of `id`'s box at `x`, `y`
/// in its parent's space ([`ondin_core::edit::placed_at`]).
fn placed_at(
    s: &EditorSession,
    id: NodeId,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<Value, String> {
    let moved = ondin_core::edit::placed_at(&s.doc, &s.resolved, id, x, y)
        .ok_or_else(|| format!("no layer `{}`", id.to_wire()))?;
    Ok(json!({ "tool": "set_transform",
        "args": { "node": id.to_wire(), "transform": moved.as_coeffs() } }))
}

/// The `set_size` step that makes `id` `w` wide and `h` high, a side not given
/// kept at its box's.
fn sized(s: &EditorSession, id: NodeId, w: Option<f64>, h: Option<f64>) -> Result<Value, String> {
    let now = match (w, h) {
        (Some(_), Some(_)) => ondin_core::kurbo::Size::ZERO,
        _ => ondin_core::query::local_box(&s.doc, &s.resolved, id)
            .map(|b| b.size())
            .ok_or_else(|| {
                format!(
                    "`{}` has no box to keep a side of: give `width` and `height`",
                    id.to_wire()
                )
            })?,
    };
    Ok(json!({ "tool": "set_size", "args": { "node": id.to_wire(),
        "width": w.unwrap_or(now.width), "height": h.unwrap_or(now.height) } }))
}

/// `set`'s pair `key` (`size` or `at`) as two sides, each `None` when not
/// given: from the pair as an object or `[a, b]`, then from its sides' own
/// names (`x`, `width`), which win. `None` when neither is written.
fn sides(
    props: &Map<String, Value>,
    key: &str,
    names: [&str; 2],
    form: &str,
) -> Result<Option<[Option<f64>; 2]>, String> {
    let mut out = [None, None];
    let number = |v: &Value| v.as_f64().ok_or_else(|| form.to_owned());
    match props.get(key) {
        None => {}
        Some(Value::Array(a)) if a.len() == 2 => {
            out = [Some(number(&a[0])?), Some(number(&a[1])?)];
        }
        Some(Value::Object(m)) if m.keys().all(|k| names.contains(&k.as_str())) => {
            for (side, name) in out.iter_mut().zip(names) {
                *side = m.get(name).map(number).transpose()?;
            }
        }
        Some(_) => return Err(form.to_owned()),
    }
    for (side, name) in out.iter_mut().zip(names) {
        if let Some(v) = props.get(name) {
            *side = Some(v.as_f64().ok_or_else(|| format!("`{name}` is a number"))?);
        }
    }
    Ok(match out {
        [None, None] if props.contains_key(key) => {
            return Err(format!("{form}; either side alone keeps the other"));
        }
        [None, None] => None,
        sides => Some(sides),
    })
}

/// `props` with `fill`, `stroke` and `corner_radius` written as what their
/// setters take: `fills` and `strokes` lists, and a `geometry` patch.
fn canonical(props: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut canon = props.clone();
    for (alias, prop) in [
        ("fill", "fills"),
        ("stroke", "strokes"),
        ("corner_radius", "geometry"),
    ] {
        let Some(v) = canon.remove(alias) else {
            continue;
        };
        if canon.contains_key(prop) {
            return Err(format!("`{alias}` and `{prop}` set one thing: give one"));
        }
        let v = match (alias, v) {
            ("corner_radius", Value::Number(n)) => json!({ "CornerRadius": n }),
            ("corner_radius", Value::Array(r)) if r.len() == 4 => json!({ "CornerRadii": {
                "top_left": r[0], "top_right": r[1], "bottom_right": r[2], "bottom_left": r[3] } }),
            ("corner_radius", corners @ Value::Object(_)) => json!({ "CornerRadii": corners }),
            ("corner_radius", _) => {
                return Err(
                    "`corner_radius` is a number, or four as [top_left, top_right, \
                     bottom_right, bottom_left]"
                        .into(),
                );
            }
            (_, list @ Value::Array(_)) => list,
            (_, one) => json!([one]),
        };
        canon.insert(prop.into(), v);
    }
    Ok(canon)
}

/// The name of `known` an unknown `set` property most likely meant: a guess
/// [`SET_GUESSES`] lists, else the nearest within two edits once written in
/// snake case.
fn nearest_name<'a>(asked: &str, known: &[&'a str]) -> Option<&'a str> {
    let mut snake = String::new();
    for c in asked.chars() {
        match c {
            '-' | ' ' => snake.push('_'),
            c if c.is_uppercase() => {
                if !snake.is_empty() {
                    snake.push('_');
                }
                snake.extend(c.to_lowercase());
            }
            c => snake.push(c),
        }
    }
    let meant = SET_GUESSES
        .iter()
        .find(|(guess, _)| *guess == snake)
        .map_or(snake.as_str(), |(_, meant)| meant);
    known
        .iter()
        .map(|k| (edits(meant, k), *k))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

/// The edit distance between `a` and `b`, in characters.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

/// Refuse `set`'s `text` on a layer that is no text, saying where its text is
/// set: an instance's through its properties, any layer's through the text
/// layers inside it.
fn text_targets(s: &EditorSession, ids: &[NodeId]) -> Result<(), String> {
    for &id in ids {
        let Some(n) = s.doc.get(id) else {
            continue;
        };
        if matches!(n.kind(), ondin_core::NodeKind::Text { .. }) {
            continue;
        }
        let mut texts = Vec::new();
        let mut below: Vec<NodeId> = n.children().iter().rev().copied().collect();
        while let Some(at) = below.pop() {
            let Some(c) = s.doc.get(at) else { continue };
            if matches!(c.kind(), ondin_core::NodeKind::Text { .. }) {
                texts.push(format!("`{}` {}", at.to_wire(), c.name()));
            }
            below.extend(c.children().iter().rev());
        }
        let listed = match texts.len() {
            0 => None,
            n if n > TEXTS_LISTED => Some(format!(
                "{}, and {} more",
                texts[..TEXTS_LISTED].join(", "),
                n - TEXTS_LISTED
            )),
            _ => Some(texts.join(", ")),
        };
        let id_wire = id.to_wire();
        if ondin_core::component::instance_root(&s.doc, id) == Some(id) {
            let props: Vec<String> = ondin_core::variant::instance_properties(&s.doc, id)
                .iter()
                .filter(|p| p.kind != ondin_core::variant::PropKind::Nested && !p.name.is_empty())
                .map(|p| {
                    let kind = format!("{:?}", p.kind).to_lowercase();
                    format!("`{}` ({kind})", p.name)
                })
                .collect();
            let props = match props.is_empty() {
                true => "it has none".to_owned(),
                false => format!("its properties: {}", props.join(", ")),
            };
            let inside = listed
                .map(|l| format!(" or set a text layer inside it ({l})"))
                .unwrap_or_default();
            return Err(format!(
                "`{id_wire}` is an instance; set its text with `set_instance_property` \
                 ({props}){inside}"
            ));
        }
        return Err(match listed {
            Some(l) => format!("`{id_wire}` is no text layer; set a text layer inside it ({l})"),
            None => format!("`{id_wire}` is no text layer and holds none"),
        });
    }
    Ok(())
}

/// The most text layers a refusal of `set`'s `text` names.
const TEXTS_LISTED: usize = 8;

/// `describe_tools`: the full definition of each tool named in `names`, then of
/// those whose name holds `query`, then of those whose description alone does,
/// at most [`DESCRIBED`]; with `brief`, every match's name and description only.
/// A name that is no tool is answered in `not_found`.
fn describe_tools(args: &Value) -> Value {
    let all = every_definition();
    let names: Vec<&str> = args
        .get("names")
        .and_then(Value::as_array)
        .map(|n| n.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let brief = args.get("brief").and_then(Value::as_bool).unwrap_or(false);
    // 0 named, 1 a name holding the query, 2 a description holding it.
    let rank = |d: &Value| {
        let name = d["name"].as_str().unwrap_or_default();
        if names.contains(&name) {
            return Some(0);
        }
        let q = query.as_deref()?;
        if name.contains(q) {
            return Some(1);
        }
        d["description"]
            .as_str()
            .is_some_and(|t| t.to_lowercase().contains(q))
            .then_some(2)
    };
    let mut matching: Vec<(u8, &Value)> =
        all.iter().filter_map(|d| rank(d).map(|r| (r, d))).collect();
    matching.sort_by_key(|(r, _)| *r);
    let not_found: Vec<&str> = names
        .iter()
        .filter(|n| !all.iter().any(|d| d["name"] == **n))
        .copied()
        .collect();
    let (tools, more): (Vec<Value>, usize) = if brief {
        let tools = matching
            .iter()
            .map(|(_, d)| {
                let full = d["description"].as_str().unwrap_or_default();
                json!({ "name": d["name"], "description": gist(full) })
            })
            .collect();
        (tools, 0)
    } else {
        let more = matching.len().saturating_sub(DESCRIBED);
        let tools = matching
            .into_iter()
            .take(DESCRIBED)
            .map(|(_, d)| d.clone())
            .collect();
        (tools, more)
    };
    let mut answer = json!({ "tools": tools, "more": more });
    if !not_found.is_empty() {
        answer["not_found"] = json!(not_found);
    }
    answer
}

/// Every tool's `tools/list` entry, listed or not.
fn every_definition() -> Vec<Value> {
    use ondin_mcp::tools::{PHASE_1, PHASE_2, PHASE_3, PHASE_4};
    PHASE_1
        .iter()
        .chain(&PHASE_2)
        .chain(&PHASE_3)
        .chain(&PHASE_4)
        .filter_map(|name| ondin_mcp::schema::definition(name))
        .collect()
}

/// Why `name` is refused as no tool, with the nearest tools by `describe_tools`'
/// search run on each of its words: a word in a tool's name counts twice one in
/// its description. `tools/list` names only the core set, so pointing there
/// alone does not find the rest.
fn no_such_tool(name: &str) -> String {
    let words: Vec<String> = name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(str::to_owned)
        .collect();
    let mut scored: Vec<(usize, String, String)> = every_definition()
        .iter()
        .filter_map(|d| {
            let tool = d["name"].as_str()?;
            let about = d["description"].as_str().unwrap_or_default();
            let lower = about.to_lowercase();
            let score: usize = words
                .iter()
                .map(
                    |w| match (tool.contains(w.as_str()), lower.contains(w.as_str())) {
                        (true, _) => 2,
                        (false, true) => 1,
                        _ => 0,
                    },
                )
                .sum();
            (score > 0).then(|| (score, tool.to_owned(), gist(about)))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
    let nearest: Vec<String> = scored
        .iter()
        .take(NEAREST)
        .map(|(_, tool, about)| format!("`{tool}`: {about}"))
        .collect();
    match nearest.is_empty() {
        true => format!(
            "No tool is called `{name}`; `describe_tools` with a `query` finds one by a word."
        ),
        false => format!(
            "No tool is called `{name}`. Nearest: {} `describe_tools` with a `query` finds others.",
            nearest.join(" ")
        ),
    }
}

/// How many tools an unknown name's refusal suggests.
const NEAREST: usize = 3;

/// The longest `gist` a brief `describe_tools` gives, in characters.
const GIST: usize = 120;

/// A tool's description cut to what a brief listing needs to pick it: its first
/// sentence, and of a long one the words up to [`GIST`] characters.
fn gist(description: &str) -> String {
    let first = description
        .split_once(". ")
        .map_or(description, |(s, _)| s)
        .trim_end_matches('.');
    if first.chars().count() <= GIST {
        return format!("{first}.");
    }
    let cut: String = first.chars().take(GIST).collect();
    let at = cut.rfind(' ').unwrap_or(cut.len());
    format!("{}…", cut[..at].trim_end_matches([',', ':', ';']))
}

/// The most full definitions one `describe_tools` answers.
const DESCRIBED: usize = 12;

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
    if tool != "set" && ondin_core::verbs::find(tool).is_none() {
        return Err(format!(
            "a batch runs `set` and the write tools that are verbs, and `{tool}` is neither"
        ));
    }
    let mut args = step.get("args").cloned().unwrap_or_else(|| json!({}));
    ondin_core::verbs::substitute(&mut args, labels)?;
    if let (Some(base), Some(map)) = (base, args.as_object_mut()) {
        map.insert("base_rev".into(), base.clone());
    }
    let depth = s.history.undo_depth();
    let ran = match tool {
        "set" => set(conn, s, client, &args)?,
        _ => write(conn, s, client, tool, &args)?,
    };
    let Answer::Json(answer) = ran else {
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
    // What others changed since `base`: every step but the client's own writes,
    // which are not news to it. `handle` caught up before this call, so the last
    // look is the document as the call found it.
    let others: std::collections::HashSet<&str> = conn
        .shown
        .iter()
        .skip(at + 1)
        .flat_map(|s| s.others.iter().map(String::as_str))
        .collect();
    // A delete writes away its whole subtree, so a layer inside it the person
    // changed counts as one the call writes.
    let deleted: Vec<ondin_core::NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                ondin_core::Operation::DeleteNode { id } => Some(*id),
                _ => None,
            })
            .collect();
    let mut written = tx.subjects();
    for id in ondin_core::build::subtree_nodes(&s.doc, &deleted) {
        if !written.contains(&id) {
            written.push(id);
        }
    }
    let moved: Vec<String> = written
        .iter()
        .map(|id| id.to_wire())
        .filter(|id| others.contains(id.as_str()))
        .collect();
    if moved.is_empty() {
        return Ok(());
    }
    // Each by the name the caller read, or the earliest look since that holds it.
    let names: Vec<String> = moved
        .iter()
        .map(|id| {
            let seen = conn
                .shown
                .iter()
                .skip(at)
                .find_map(|s| s.layers.as_ref()?.get(id));
            match seen {
                Some(l) => format!("{} ({id})", l.name),
                None => id.clone(),
            }
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
    pub(in crate::mcp) struct One {
        pub(in crate::mcp) path: PathBuf,
        pub(in crate::mcp) session: EditorSession,
        committed: usize,
    }

    impl One {
        pub(in crate::mcp) fn new() -> Self {
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

    pub(in crate::mcp) fn call(
        engine: &mut Engine,
        docs: &mut One,
        conn: Conn,
        tool: &str,
        args: Value,
    ) -> Output {
        keyed(engine, docs, conn, None, tool, args)
    }

    /// [`call`] from a client that gave `key`.
    pub(in crate::mcp) fn keyed(
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

    pub(in crate::mcp) fn ok(out: &Output) -> &Value {
        assert!(!out.is_error, "{:?}", out.content);
        out.structured.as_ref().unwrap()
    }

    pub(in crate::mcp) fn open(engine: &mut Engine, docs: &mut One, conn: Conn) {
        ok(&call(
            engine,
            docs,
            conn,
            "use_document",
            json!({ "document": "a" }),
        ));
    }

    /// Make a rectangle on the first page through the engine, answering its id.
    pub(in crate::mcp) fn rect(
        engine: &mut Engine,
        docs: &mut One,
        conn: Conn,
        name: &str,
    ) -> String {
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

    /// `reparent` answers where the moved layer now sits in its new parent,
    /// its box's left and top there, since the move kept it where it was drawn.
    ///
    /// flip: the `reparent` arm of `write` reading the box against the old
    /// parent → fails at `[50, 70]`, answering `[150, 170]`.
    #[test]
    fn reparent_answers_the_place_in_the_new_parent() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let mut insert = |kind: Value, x: f64, y: f64| {
            let args = json!({ "parent": page, "tree": { "kind": kind }, "x": x, "y": y });
            let out = call(&mut engine, &mut docs, 1, "insert_subtree", args);
            ok(&out)["created"][0].as_str().unwrap().to_owned()
        };
        let frame = insert(
            json!({ "Artboard": { "size": { "width": 200, "height": 200 } } }),
            100.0,
            100.0,
        );
        let dot = insert(
            json!({ "Rect": { "size": { "width": 10, "height": 10 } } }),
            150.0,
            170.0,
        );
        let turned = insert(
            json!({ "Artboard": { "size": { "width": 200, "height": 200 } } }),
            400.0,
            100.0,
        );
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "reparent",
            json!({ "node": dot, "parent": frame }),
        );
        assert_eq!(ok(&out)["at"], json!([50.0, 70.0]), "{:?}", out.structured);

        // Under a turned parent the answer is in the parent's own space: set
        // back as `at`, it moves nothing.
        // flip: the `reparent` arm measuring world box less parent origin →
        // fails at "moves nothing".
        let turn = json!({ "nodes": [turned], "props": { "rotation": 90 } });
        ok(&call(&mut engine, &mut docs, 1, "set", turn));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "reparent",
            json!({ "node": dot, "parent": turned }),
        );
        let at = ok(&out)["at"].clone();
        let id = NodeId::from_wire(&dot).unwrap();
        let before = docs.session.doc.get(id).unwrap().transform();
        let place = json!({ "nodes": [dot], "props": { "at": at } });
        ok(&call(&mut engine, &mut docs, 1, "set", place));
        let after = docs.session.doc.get(id).unwrap().transform();
        let moved = (after.translation() - before.translation()).hypot();
        assert!(moved < 0.06, "moves nothing: {moved}");
    }

    /// `set`'s `rotation` on a line is the angle the Rotation field shows, its
    /// start-to-end: a line drawn at 45° set to 0 lies flat.
    /// flip: `turned_to` measuring the transform's basis → fails at "flat".
    #[test]
    fn set_rotation_turns_a_line_to_the_angle_shown() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "kind": { "Line": { "end": { "x": 100.0, "y": 100.0 } } } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let line = ok(&out)["created"][0].clone();
        let set = json!({ "nodes": [line], "props": { "rotation": 0 } });
        ok(&call(&mut engine, &mut docs, 1, "set", set));
        let id = NodeId::from_wire(line.as_str().unwrap()).unwrap();
        let world = docs.session.resolved.world_transform(id).unwrap();
        let ondin_core::NodeKind::Line { end } = docs.session.doc.get(id).unwrap().kind() else {
            panic!("not a line");
        };
        let (a, b) = (world * ondin_core::kurbo::Point::ZERO, world * *end);
        assert!((b.y - a.y).abs() < 1e-6 && b.x > a.x, "flat: {a:?} → {b:?}");
    }

    /// `set`'s `at` on a layer its container lays out is refused, naming the
    /// layout, rather than dropped and answered as done; new fills through a
    /// group clear the colour of the texts inside it.
    ///
    /// flip: `set` without its flex-item refusal → fails at "refused";
    /// clearing over `ids` alone → fails at "the nested text's colour".
    #[test]
    fn set_refuses_a_laid_out_place_and_clears_nested_text_colour() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "kind": { "Artboard": { "size": { "width": 200, "height": 100 } } },
            "children": [ { "kind": { "Rect": { "size": { "width": 10, "height": 10 } } } } ] });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let created = ok(&out)["created"].as_array().unwrap().clone();
        let (frame, item) = (created[0].clone(), created[1].clone());
        let display = ondin_core::verbs::example("Display").unwrap();
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_display",
            json!({ "nodes": [frame], "display": display }),
        ));
        let item_id = NodeId::from_wire(item.as_str().unwrap()).unwrap();
        assert!(
            ondin_core::build::is_flex_item(&docs.session.doc, item_id),
            "the fixture laid the item out"
        );
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [item], "props": { "x": 50 } }),
        );
        assert!(out.is_error, "refused");

        let text = call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Hi" }),
        );
        let text = ok(&text)["created"][0].clone();
        let style = json!({ "nodes": [text], "style": { "color": "#ff0000" } });
        ok(&call(&mut engine, &mut docs, 1, "set_text_style", style));
        let group = call(
            &mut engine,
            &mut docs,
            1,
            "group",
            json!({ "nodes": [text] }),
        );
        let group = ok(&group)["created"][0].clone();
        let fills = json!({ "nodes": [group], "props": { "fills": ["#00ff00"] } });
        ok(&call(&mut engine, &mut docs, 1, "set", fills));
        let id = NodeId::from_wire(text.as_str().unwrap()).unwrap();
        let ondin_core::NodeKind::Text { style, .. } = docs.session.doc.get(id).unwrap().kind()
        else {
            panic!("not a text");
        };
        assert_eq!(style.color, None, "the nested text's colour");
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
            // `call` answers as the tool it names, which its own test covers.
            if tool.name == "use_document" || tool.name == "call" {
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
        // As a batch's step too.
        let step =
            json!({ "tool": "replace_font", "args": { "from": from, "to": "Nowhere Sans" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [step] }),
        );
        assert!(out.is_error, "a batch step is refused");
        assert_eq!(family(&docs), from);
        // And as a proposal's call.
        // flip: the check reading a proposal call's `tool` → fails here.
        let proposed = json!({ "title": "Swap", "calls": [{ "verb": "replace_font",
            "args": { "from": from, "to": "Nowhere Sans" } }] });
        let out = call(&mut engine, &mut docs, 1, "propose_edit", proposed);
        assert!(out.is_error, "a proposal's call is refused");
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
        assert!(ok(&out)["image"].is_string());
        assert_eq!(ok(&out)["pixel_size"], json!([3, 2]), "the size");
    }

    /// A connection that never called closing is no client leaving, so a host
    /// keeps its documents; one that called is.
    ///
    /// flip: `Engine::closed` answering `true` always → fails at the probe.
    #[test]
    fn a_probe_closing_is_no_client_leaving() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        assert!(!engine.closed(2), "a probe never called");
        assert!(engine.closed(1), "a client that called left");
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

    /// The person deleting a frame of many layers is reported with each list cut
    /// at `CHANGED_LISTED`, the rest counted, and the frame named as where they were; a
    /// sibling frame's layers do not read as changed.
    ///
    /// flip: `Delta::to_json` naming every id → fails at the cut list's length.
    #[test]
    fn a_large_change_is_cut_and_names_its_sections() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page();
        let frame = docs.session.doc.get(page).unwrap().children()[0];
        let args = json!({ "nodes": [frame.to_wire()] });
        let out = call(&mut engine, &mut docs, 1, "duplicate", args);
        let copy = ok(&out)["created"][0].as_str().unwrap().to_owned();
        let kind = ondin_core::NodeKind::Rect {
            size: ondin_core::kurbo::Size::new(120.0, 80.0),
            corner_radii: ondin_core::kurbo::RoundedRectRadii::from_single_radius(8.0),
            corner_smoothing: 0.0,
        };
        let ops = (0..CHANGED_LISTED + 5)
            .map(|i| ondin_core::Operation::CreateNode {
                id: docs.session.ids.mint(),
                parent: frame,
                index: i,
                kind: kind.clone(),
                transform: None,
                name: None,
            })
            .collect();
        docs.session
            .try_commit(ondin_core::Transaction(ops))
            .unwrap();
        ok(&call(&mut engine, &mut docs, 1, "get_selection", json!({})));
        let held = 1 + docs.session.doc.get(frame).unwrap().children().len();
        assert!(
            held > CHANGED_LISTED + 5,
            "the frame holds more than a list names"
        );
        let tx = ondin_core::Transaction(vec![ondin_core::Operation::DeleteNode { id: frame }]);
        docs.session.try_commit(tx).unwrap();
        let view = call(&mut engine, &mut docs, 1, "get_view", json!({}));
        let changed = &ok(&view)["changed"];
        assert_eq!(
            changed["removed"].as_array().map(Vec::len),
            Some(CHANGED_LISTED)
        );
        assert_eq!(
            changed["left_out"]["removed"],
            held - CHANGED_LISTED,
            "{changed}"
        );
        assert_eq!(changed["left_out"]["in"][0]["id"], json!(frame.to_wire()));
        let page = json!([{ "id": page.to_wire(), "name": "Page 1" }]);
        assert_eq!(
            changed["changed"], page,
            "only the page's children: {changed}"
        );
        assert!(
            !changed.to_string().contains(&copy),
            "the sibling is unchanged"
        );
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

    /// A table tool's model argument sent as JSON text is read as the structure
    /// it holds, by the handler as well as the check, and so is `call`'s own
    /// `args`.
    ///
    /// flip: `parse_json_text` leaving `call`'s `args` as text → fails at
    /// "call's args".
    #[test]
    fn json_text_arguments_reach_the_handler_parsed() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Box");
        let props = json!({ "name": "Card" }).to_string();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [id], "props": props }),
        );
        ok(&out);
        let node = ondin_core::NodeId::from_wire(&id).unwrap();
        assert_eq!(docs.session.doc.get(node).unwrap().name(), "Card", "props");
        let names = json!(["set_point"]).to_string();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "names": names }),
        );
        assert_eq!(ok(&out)["tools"][0]["name"], "set_point", "names");
        // `call`'s own `args` as JSON text, and a model argument inside it.
        let args = json!({ "node": id, "name": "Renamed" }).to_string();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "call",
            json!({ "tool": "set_name", "args": args }),
        );
        ok(&out);
        assert_eq!(
            docs.session.doc.get(node).unwrap().name(),
            "Renamed",
            "call's args"
        );
        let args = json!({ "names": json!(["set"]).to_string() }).to_string();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "call",
            json!({ "tool": "describe_tools", "args": args }),
        );
        assert_eq!(ok(&out)["tools"][0]["name"], "set", "nested");
    }

    /// A delete with a stale `base_rev` is refused when a layer inside what it
    /// removes changed, not only the layer it names.
    ///
    /// flip: `stale` checking `subjects()` alone → fails at "refused".
    #[test]
    fn a_stale_delete_checks_what_it_removes() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let child = rect(&mut engine, &mut docs, 1, "Label");
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "group",
            json!({ "nodes": [child] }),
        );
        let group = ok(&out)["created"][0].as_str().unwrap().to_owned();
        let read = call(
            &mut engine,
            &mut docs,
            2,
            "get_node",
            json!({ "id": group }),
        );
        let base = ok(&read)["rev"].clone();
        let rename = json!({ "node": child, "name": "Edited" });
        ok(&call(&mut engine, &mut docs, 1, "set_name", rename));
        let delete = json!({ "nodes": [group], "base_rev": base });
        let late = call(&mut engine, &mut docs, 2, "delete_node", delete);
        assert!(late.is_error, "refused");
        let id = ondin_core::NodeId::from_wire(&child).unwrap();
        assert!(docs.session.doc.get(id).is_some(), "the child stays");
    }

    /// `set` writes several properties through their setters as one undo step
    /// named by what it set, the size before the place.
    ///
    /// flip: `set` skipping its `at` → fails at "moved".
    #[test]
    fn set_writes_several_properties_as_one_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Box");
        let props = json!({ "name": "Card", "opacity": 0.5, "fills": ["#ff0000"],
            "size": { "width": 30.0, "height": 40.0 }, "at": [10.0, 20.0] });
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [id], "props": props }),
        ));
        let nid = NodeId::from_wire(&id).unwrap();
        let n = docs.session.doc.get(nid).unwrap();
        assert_eq!(n.name(), "Card");
        assert_eq!(n.opacity(), 0.5);
        let r = docs.session.resolved.world_bounds(nid).unwrap();
        assert_eq!((r.x0, r.y0), (10.0, 20.0), "moved");
        assert_eq!((r.width(), r.height()), (30.0, 40.0));
        assert_eq!(docs.session.history.undo_depth(), 2, "one step for `set`");
        let label = docs
            .session
            .history
            .done()
            .next_back()
            .unwrap()
            .note
            .label
            .clone();
        assert_eq!(
            label, "set at, fills, name, opacity, size",
            "named by what it set"
        );
        let bad = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [id], "props": { "colour": "#fff" } }),
        );
        assert!(bad.is_error, "an unknown property is refused");
    }

    /// `set`'s fill on a text whose style carries a colour clears that colour so
    /// the fill is what draws, and a `set` that changes nothing answers `unchanged`.
    ///
    /// flip: `set` skipping its `set_text_style` step → fails at "the style's colour
    /// is cleared"; `set` answering what it was given when nothing landed → fails
    /// at "nothing set".
    #[test]
    fn set_fill_recolours_a_text_and_a_no_op_says_so() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "fills": ["#12352a"], "kind": { "Text": { "content": "Hi",
            "sizing": "Auto", "style": { "color": "#ffffff" } } } });
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let id = ok(&made)["created"][0].as_str().unwrap().to_owned();
        let nid = NodeId::from_wire(&id).unwrap();
        let colour = |docs: &One| match docs.session.doc.get(nid).unwrap().kind() {
            ondin_core::NodeKind::Text { style, .. } => style.color,
            _ => unreachable!(),
        };
        assert!(colour(&docs).is_some(), "the fixture's style has a colour");
        let set = |engine: &mut Engine, docs: &mut One| {
            call(
                engine,
                docs,
                1,
                "set",
                json!({ "nodes": [id], "props": { "fill": "#12352a" } }),
            )
        };
        let first = set(&mut engine, &mut docs);
        assert_eq!(ok(&first)["set"], json!(["fill"]));
        assert_eq!(colour(&docs), None, "the style's colour is cleared");
        let again = set(&mut engine, &mut docs);
        let answer = ok(&again);
        assert_eq!(answer["set"], json!([]), "nothing set");
        assert_eq!(answer["unchanged"], json!(["fill"]));
    }

    /// `set`'s `at` puts a group's box where it puts a layer's: its left and top in
    /// the parent, wherever its children sat, not the group's transform moved there.
    ///
    /// flip: `placed_at` writing `x`, `y` as the transform's offset → fails at "the
    /// box's corner".
    #[test]
    fn set_places_a_groups_box_by_its_corner() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let a = rect(&mut engine, &mut docs, 1, "A");
        let b = rect(&mut engine, &mut docs, 1, "B");
        for (id, at) in [(&a, [100.0, 50.0]), (&b, [160.0, 80.0])] {
            let props = json!({ "at": at });
            ok(&call(
                &mut engine,
                &mut docs,
                1,
                "set",
                json!({ "nodes": [id], "props": props }),
            ));
        }
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "group",
            json!({ "nodes": [a, b] }),
        );
        let group = ok(&made)["created"][0].clone();
        let gid = NodeId::from_wire(group.as_str().unwrap()).unwrap();
        let aid = NodeId::from_wire(&a).unwrap();
        let before = docs.session.resolved.world_bounds(aid).unwrap();
        let box_of = |docs: &One| {
            let s = &docs.session;
            let local = s.resolved.used_local(&s.doc, gid).unwrap();
            let b = ondin_core::query::local_box(&s.doc, &s.resolved, gid).unwrap();
            ondin_core::geometry::transform_rect(local, b).origin()
        };
        assert_eq!(
            box_of(&docs),
            (100.0, 50.0).into(),
            "the group spans its children"
        );
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [group], "props": { "at": [10.0, 20.0] } }),
        ));
        let after = docs.session.resolved.world_bounds(aid).unwrap();
        assert_eq!(
            (after.x0 - before.x0, after.y0 - before.y0),
            (-90.0, -30.0),
            "the box's corner moved to [10, 20]"
        );
        assert_eq!(box_of(&docs), (10.0, 20.0).into());
    }

    /// `set` takes the names agents guess: one side of `at` or `size` keeps the
    /// other, `corner_radius`, `fill` and `rotation` land as their setters would,
    /// and an unknown name is answered with the one it most likely meant.
    ///
    /// flip: `placed_at` taking an absent side as 0 → fails at "y kept";
    /// `turned_to` turning about the layer's origin → fails at "about its centre".
    #[test]
    fn set_takes_the_names_agents_guess() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Box");
        let nid = NodeId::from_wire(&id).unwrap();
        let mut set = |docs: &mut One, props: Value| {
            call(
                &mut engine,
                docs,
                1,
                "set",
                json!({ "nodes": [id], "props": props }),
            )
        };
        ok(&set(
            &mut docs,
            json!({ "at": [10.0, 20.0], "size": { "width": 30.0, "height": 40.0 } }),
        ));
        ok(&set(&mut docs, json!({ "x": 50.0, "width": 60.0 })));
        let bounds = |docs: &One| docs.session.resolved.world_bounds(nid).unwrap();
        let r = bounds(&docs);
        assert_eq!((r.x0, r.width()), (50.0, 60.0), "the sides given");
        assert_eq!((r.y0, r.height()), (20.0, 40.0), "y kept, and the height");
        ok(&set(&mut docs, json!({ "size": { "height": 10.0 } })));
        assert_eq!(
            (bounds(&docs).width(), bounds(&docs).height()),
            (60.0, 10.0)
        );

        ok(&set(
            &mut docs,
            json!({ "corner_radius": 4.0, "fill": "#ff0000" }),
        ));
        let n = docs.session.doc.get(nid).unwrap();
        let ondin_core::NodeKind::Rect { corner_radii, .. } = n.kind() else {
            panic!("the fixture is a rect");
        };
        assert_eq!(corner_radii.top_left, 4.0);
        assert_eq!(n.paint().fills.len(), 1, "one paint as a list");

        let centre = bounds(&docs).center();
        ok(&set(&mut docs, json!({ "rotation": 90.0 })));
        let r = bounds(&docs);
        assert!(
            (r.width() - 10.0).abs() < 1e-6 && (r.height() - 60.0).abs() < 1e-6,
            "a quarter turn: {r:?}"
        );
        let moved = r.center() - centre;
        assert!(moved.hypot() < 1e-6, "about its centre: {moved:?}");
        ok(&set(&mut docs, json!({ "rotation": 90.0 })));
        assert!(
            (bounds(&docs).width() - 10.0).abs() < 1e-6,
            "the angle is set, not added"
        );

        for (guess, meant) in [("corner_radiu", "corner_radius"), ("colour", "fill")] {
            let out = set(&mut docs, json!({ guess: 1.0 }));
            let text = format!("{:?}", out.content);
            assert!(out.is_error, "{guess} is refused");
            assert!(text.contains(&format!("Did you mean `{meant}`")), "{text}");
        }
    }

    /// `set`'s `text` on an instance names `set_instance_property` with the
    /// instance's properties and the text layers inside it, before any setter runs.
    /// On the way, `make_component` answers a group it made a main as `main`, not
    /// `created`, and `define_property` answers the property by its name.
    ///
    /// flip: `set` without its `text_targets` check → fails at "names the tool",
    /// as the setter's own refusal; `write` not filtering `created` → fails at
    /// "not answered as created".
    #[test]
    fn set_text_on_an_instance_names_where_its_text_is() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "name": "Button", "kind": { "Group": null },
            "children": [{ "name": "Label", "kind": { "Text": { "content": "Go",
                "style": ondin_core::verbs::example("TextStyle").unwrap(), "sizing": "Auto" } } }] });
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let answer = ok(&made).clone();
        let group = answer["created"][0].clone();
        let label = answer["named"]["Button/Label"].clone();
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "make_component",
            json!({ "nodes": [group] }),
        );
        let main = ok(&made)["main"].clone();
        assert_eq!(main, group, "the group itself is made the main");
        assert_eq!(
            ok(&made)["created"],
            json!([]),
            "and is not answered as created"
        );
        let args = json!({ "node": label, "kind": "text", "name": "Caption" });
        let defined = call(&mut engine, &mut docs, 1, "define_property", args);
        assert_eq!(ok(&defined)["property"], "Caption", "named, not by id");
        assert!(ok(&defined).get("made").is_none());
        let args = json!({ "main": main, "parent": page, "x": 300, "y": 300 });
        let made = call(&mut engine, &mut docs, 1, "create_instance", args);
        let instance = ok(&made)["created"][0].clone();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [instance], "props": { "text": "Stop" } }),
        );
        let text = format!("{:?}", out.content);
        assert!(out.is_error);
        assert!(
            text.contains("set_instance_property"),
            "names the tool: {text}"
        );
        assert!(text.contains("`Caption` (text)"), "{text}");
        assert!(text.contains("Label"), "the text layer inside: {text}");
    }

    /// A batch step names a layer an earlier `insert_subtree` step made by
    /// `@label/Path`, its path of names below the root.
    ///
    /// flip: `batch` not labelling `named` → fails at the batch's `ok`, as no
    /// earlier step is called `card/Title`.
    #[test]
    fn a_batch_names_a_subtrees_layer_by_its_path() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let tree = json!({ "name": "Card", "kind": { "Group": null }, "children": [
            { "name": "Title", "kind": kind },
        ]});
        let insert = json!({ "tool": "insert_subtree", "as": "card",
            "args": { "parent": page, "tree": tree } });
        let rename = json!({ "tool": "set_name",
            "args": { "node": "@card/Title", "name": "Heading" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [insert, rename] }),
        );
        let title = ok(&out)["steps"][0]["named"]["Card/Title"]
            .as_str()
            .unwrap()
            .to_owned();
        let n = docs.session.doc.get(NodeId::from_wire(&title).unwrap());
        assert_eq!(n.unwrap().name(), "Heading");
    }

    /// `call` of a tool that does not exist suggests the nearest by the words of
    /// its name, and points at `describe_tools`.
    ///
    /// flip: `no_such_tool` scoring a description's word as nothing → fails at
    /// `set_geometry`, which only its description reaches.
    #[test]
    fn an_unknown_tool_suggests_the_nearest() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "call",
            json!({ "tool": "set_corner_radius", "args": {} }),
        );
        assert!(out.is_error);
        let text = format!("{:?}", out.content);
        assert!(text.contains("`set_geometry`"), "{text}");
        assert!(text.contains("describe_tools"), "{text}");
    }

    /// Writes off one read keep passing its `base_rev` long after the last few
    /// looks the connection keeps whole, and another client's change to the layer
    /// in between still refuses it.
    ///
    /// flip: `remember` dropping looks past `KEPT` → fails at "an old read still
    /// holds".
    #[test]
    fn a_base_rev_holds_across_many_own_writes() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Card");
        let other = rect(&mut engine, &mut docs, 1, "Other");
        let read = call(&mut engine, &mut docs, 1, "get_selection", json!({}));
        let base = ok(&read)["rev"].clone();
        for i in 0..KEPT * 3 {
            let args = json!({ "node": id, "name": format!("Card {i}"), "base_rev": base });
            let out = call(&mut engine, &mut docs, 1, "set_name", args);
            assert!(!out.is_error, "an old read still holds: {:?}", out.content);
        }
        let args = json!({ "nodes": [other], "opacity": 0.5 });
        ok(&call(&mut engine, &mut docs, 2, "set_opacity", args));
        for _ in 0..KEPT * 2 {
            let args = json!({ "node": id, "name": "Card", "base_rev": base });
            ok(&call(&mut engine, &mut docs, 1, "set_name", args));
        }
        let args = json!({ "node": other, "name": "Mine", "base_rev": base });
        let late = call(&mut engine, &mut docs, 1, "set_name", args);
        assert!(late.is_error, "the other client's change refuses it");
        assert!(format!("{:?}", late.content).contains("Other"));
    }

    /// `call` answers as the tool it names, listed or not, and never names itself;
    /// `describe_tools` finds a tool by name and by a word.
    ///
    /// flip: `handle` leaving `call` unrewritten → fails at the call's `ok`, as no
    /// tool.
    #[test]
    fn call_runs_any_tool_and_describe_tools_finds_it() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Box");
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "call",
            json!({ "tool": "set_name", "args": { "node": id, "name": "Called" } }),
        );
        ok(&out);
        let n = docs
            .session
            .doc
            .get(NodeId::from_wire(&id).unwrap())
            .unwrap();
        assert_eq!(n.name(), "Called", "renamed");
        assert!(call(&mut engine, &mut docs, 1, "call", json!({ "tool": "call" })).is_error);
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "names": ["set_point"], "query": "gradient" }),
        );
        let tools = ok(&found)["tools"].as_array().unwrap().clone();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"set_point"), "{names:?}");
        assert!(tools.iter().all(|t| t["inputSchema"].is_object()));
    }

    /// `describe_tools` answers the tools whose name holds the query before those
    /// whose description alone does, answers names and a short line each when
    /// `brief`, and names what it did not find.
    ///
    /// flip: `describe_tools` not sorting by rank → fails at the first tool's name,
    /// as `get_tree` leads; a brief listing the whole description → fails at "a
    /// brief line is short".
    #[test]
    fn describe_tools_ranks_name_matches_first() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "query": "style" }),
        );
        let tools = ok(&found)["tools"].as_array().unwrap().clone();
        let named: Vec<bool> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap().contains("style"))
            .collect();
        assert!(named[0], "{tools:?}");
        assert!(
            named.windows(2).all(|w| w[0] || !w[1]),
            "name matches first: {named:?}"
        );
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "query": "style", "brief": true, "names": ["no_such_tool"] }),
        );
        let answer = ok(&found);
        let tools = answer["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "add_style"), "{tools:?}");
        assert!(tools.iter().all(|t| t.get("inputSchema").is_none()));
        let longest = tools
            .iter()
            .map(|t| t["description"].as_str().unwrap().chars().count())
            .max();
        assert!(
            longest <= Some(GIST + 1),
            "a brief line is short: {longest:?}"
        );
        assert_eq!(answer["not_found"], json!(["no_such_tool"]), "not found");
    }

    /// A subtree names its page and frame once, on the root: a layer below keeps
    /// only a frame that is not the root's.
    ///
    /// flip: `get_node` writing every layer's `page` → fails at "said once".
    #[test]
    fn a_subtree_names_its_page_and_frame_once() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "name": "Card", "kind": { "Artboard": { "size": { "width": 200.0, "height": 100.0 },
            "corner_radii": { "top_left": 0.0, "top_right": 0.0, "bottom_right": 0.0, "bottom_left": 0.0 } } },
            "children": [{ "kind": ondin_core::verbs::example("NodeKind").unwrap() }] });
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let card = ok(&made)["created"][0].clone();
        let read = call(
            &mut engine,
            &mut docs,
            1,
            "get_node",
            json!({ "id": card, "subtree": true }),
        );
        let nodes = &ok(&read)["nodes"];
        assert_eq!(nodes[0]["page"], json!(page), "{nodes}");
        assert!(nodes[1].get("page").is_none(), "said once: {nodes}");
        assert!(nodes[1].get("artboard").is_none(), "{nodes}");
    }

    /// An agent's `undo` takes back its own step, and is refused over the person's
    /// step or another client's, changing nothing.
    ///
    /// flip: `own_step` answering `Ok(())` always → fails at "the person's step is
    /// refused".
    #[test]
    fn undo_takes_back_only_the_callers_own_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = NodeId::from_wire(&rect(&mut engine, &mut docs, 1, "Mine")).unwrap();
        let rename = ondin_core::Transaction(vec![ondin_core::Operation::SetName {
            id,
            name: "Theirs".into(),
        }]);
        docs.session.try_commit(rename).unwrap();
        let out = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert!(out.is_error, "the person's step is refused");
        assert_eq!(docs.session.history.undo_depth(), 2, "nothing changed");
        docs.session.undo();
        let (reply, _answer) = sync_channel(1);
        let mut other = Call::new(1, None, "undo", json!({}), reply);
        other.client = "other".into();
        assert!(engine.handle(&mut docs, other).is_error, "another client's");
        ok(&call(&mut engine, &mut docs, 1, "undo", json!({})));
        assert_eq!(docs.session.history.undo_depth(), 0, "its own is undone");
    }

    /// `undo` and `redo` answer the name of the step they moved, as the history
    /// lists it, and `undone: null` at the end of the history.
    ///
    /// flip: `own_step` answering `None` for the client's own step → fails at
    /// the undo's name.
    #[test]
    fn undo_and_redo_name_the_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Chip");
        let args = json!({ "node": id, "name": "Pill" });
        ok(&call(&mut engine, &mut docs, 1, "set_name", args));
        let label = docs
            .session
            .history
            .done()
            .next_back()
            .unwrap()
            .note
            .label
            .clone();
        assert!(!label.is_empty());
        let undo = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert_eq!(ok(&undo)["undone"], json!(label), "the undo's name");
        let redo = call(&mut engine, &mut docs, 1, "redo", json!({}));
        assert_eq!(ok(&redo)["redone"], json!(label));
        ok(&call(&mut engine, &mut docs, 1, "undo", json!({})));
        ok(&call(&mut engine, &mut docs, 1, "undo", json!({})));
        let none = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert_eq!(ok(&none)["undone"], Value::Null);
    }

    /// `insert_subtree` answers each named layer by its path of names, an unnamed
    /// layer between them taking its id but no entry, and `created` holds the root
    /// and the unnamed layers, so each id is said once.
    ///
    /// flip: `named_paths` taking an id only for a named layer → fails at the
    /// title's name.
    #[test]
    fn insert_subtree_answers_each_named_layer_by_path() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let tree = json!({ "name": "Hero", "kind": { "Group": null }, "children": [
            { "kind": kind },
            { "name": "Title", "kind": kind },
        ]});
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let answer = ok(&out).clone();
        let created = &answer["created"];
        assert_eq!(answer["named"]["Hero"], created[0], "{answer}");
        assert_eq!(answer["named"].as_object().map(Map::len), Some(2));
        let title = answer["named"]["Hero/Title"].as_str().unwrap();
        let title = NodeId::from_wire(title).unwrap();
        assert_eq!(docs.session.doc.get(title).unwrap().name(), "Title");
        assert_eq!(
            created.as_array().map(Vec::len),
            Some(2),
            "the root and the unnamed layer: {answer}"
        );
        assert!(
            !created
                .as_array()
                .unwrap()
                .contains(&json!(title.to_wire())),
            "each id once"
        );
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

    /// A 120 × 80 rectangle named `name` at (`x`, `y`) on the first page,
    /// answering its id.
    fn rect_at(engine: &mut Engine, docs: &mut One, name: &str, x: f64, y: f64) -> String {
        let page = docs.session.doc.first_page().to_wire();
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let args = json!({
            "parent": page, "kind": kind, "name": name,
            "transform": [1.0, 0.0, 0.0, 1.0, x, y],
        });
        let out = call(engine, docs, 1, "create_node", args);
        ok(&out)["created"][0].as_str().unwrap().to_owned()
    }

    /// Two rectangles named alike, one grouped with a third: `Header` at the
    /// origin, then `Section` holding `Header` at (200, 50.04) and `Body`, on
    /// a page cleared of a new document's sample frame.
    fn two_headers(engine: &mut Engine, docs: &mut One) -> [String; 4] {
        open(engine, docs, 1);
        let sample: Vec<String> = docs
            .session
            .doc
            .get(docs.session.doc.first_page())
            .unwrap()
            .children()
            .iter()
            .map(|c| c.to_wire())
            .collect();
        ok(&call(
            engine,
            docs,
            1,
            "delete_node",
            json!({ "nodes": sample }),
        ));
        let top = rect_at(engine, docs, "Header", 0.0, 0.0);
        let inner = rect_at(engine, docs, "Header", 200.0, 50.04);
        let body = rect_at(engine, docs, "Body", 400.0, 0.0);
        let out = call(engine, docs, 1, "group", json!({ "nodes": [inner, body] }));
        let group = ok(&out)["created"][0].as_str().unwrap().to_owned();
        ok(&call(
            engine,
            docs,
            1,
            "set_name",
            json!({ "node": group, "name": "Section" }),
        ));
        [top, group, inner, body]
    }

    /// `get_outline` answers a page's layers as rows in paint order, a parent
    /// before its children, with world boxes to 0.1, cut at `depth` and `limit`
    /// with the rest counted, and the selection under its root.
    ///
    /// flip: `reads::outline` pushing children unreversed → fails at the row order.
    #[test]
    fn get_outline_maps_a_page_in_one_table() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let [top, group, inner, body] = two_headers(&mut engine, &mut docs);
        assert_eq!(
            docs.session
                .doc
                .get(docs.session.page)
                .unwrap()
                .children()
                .len(),
            2,
            "the fixture's page holds the top header and the section"
        );
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_selection",
            json!({ "ids": [body] }),
        ));
        let out = call(&mut engine, &mut docs, 1, "get_outline", json!({}));
        let a = ok(&out);
        assert_eq!(
            a["columns"],
            json!(["id", "name", "kind", "x", "y", "w", "h", "depth"])
        );
        let rows = a["rows"].as_array().unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r[0].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            [&top, &group, &inner, &body],
            "paint order, parent first"
        );
        assert_eq!(
            rows[2],
            json!([inner, "Header", "rect", 200, 50, 120, 80, 2])
        );
        assert_eq!(rows[1][7], 1);
        assert_eq!(a["more"], 0);
        assert_eq!(a["selected"], json!([body]));

        let shallow = call(
            &mut engine,
            &mut docs,
            1,
            "get_outline",
            json!({ "depth": 1, "limit": 1 }),
        );
        assert_eq!(ok(&shallow)["rows"].as_array().unwrap().len(), 1);
        assert_eq!(
            ok(&shallow)["more"],
            1,
            "the section, its children past depth"
        );

        let one = call(
            &mut engine,
            &mut docs,
            1,
            "get_outline",
            json!({ "id": top }),
        );
        assert_eq!(
            ok(&one)["rows"],
            json!([[top, "Header", "rect", 0, 0, 120, 80, 0]])
        );
        assert_eq!(ok(&one)["selected"], json!([]));
    }

    /// `get_outline` and `find_nodes` give a stroked layer's own box, where
    /// `get_node`'s `world_bounds` take in the stroke.
    ///
    /// flip: `reads::bounds_box` answering the world bounds → fails at the
    /// outline's row.
    #[test]
    fn the_outline_boxes_a_layer_without_its_stroke() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect_at(&mut engine, &mut docs, "Stroked", 10.0, 10.0);
        let strokes = ondin_core::keyed_by_position([ondin_core::Stroke {
            width: 3.0,
            ..Default::default()
        }]);
        let nid = NodeId::from_wire(&id).unwrap();
        let tx =
            ondin_core::Transaction(vec![ondin_core::Operation::SetStrokes { id: nid, strokes }]);
        docs.session.try_commit(tx).unwrap();
        let node = call(&mut engine, &mut docs, 1, "get_node", json!({ "id": id }));
        let inked = &ok(&node)["nodes"][0]["world_bounds"];
        assert!(
            inked[2].as_f64() > Some(120.0),
            "the stroke is drawn: {inked}"
        );
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "get_outline",
            json!({ "id": id }),
        );
        assert_eq!(
            ok(&out)["rows"],
            json!([[id, "Stroked", "rect", 10, 10, 120, 80, 0]])
        );
        let args = json!({ "query": "stroked" });
        let found = call(&mut engine, &mut docs, 1, "find_nodes", args);
        assert_eq!(ok(&found)["nodes"][0]["box"], json!([10, 10, 120, 80]));
    }

    /// `find_nodes` tells two layers of one name apart by their boxes, and marks
    /// the one selected and the group holding it.
    ///
    /// flip: `find_nodes` asking whether a match lies under the selection rather
    /// than holds it → fails at "the section holds the selection".
    #[test]
    fn find_nodes_gives_boxes_and_the_selection() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let [top, group, inner, _] = two_headers(&mut engine, &mut docs);
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_selection",
            json!({ "ids": [inner] }),
        ));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "find_nodes",
            json!({ "query": "e" }),
        );
        let found = ok(&out)["nodes"].as_array().unwrap().clone();
        let by = |id: &str| found.iter().find(|n| n["id"] == id).unwrap().clone();
        assert_eq!(by(&top)["box"], json!([0, 0, 120, 80]));
        assert!(by(&top).get("selected").is_none());
        assert_eq!(by(&inner)["box"], json!([200, 50, 120, 80]));
        assert_eq!(by(&inner)["selected"], true);
        assert_eq!(
            by(&group)["selected"],
            true,
            "the section holds the selection"
        );
    }

    /// `export_png`'s `region` draws that rectangle of the page, up to 2×, and
    /// refuses a layer `id` and a rectangle with no area.
    ///
    /// flip: the region's scale capped at 1× as a page's is → fails at the size.
    #[test]
    fn export_png_draws_a_region_of_the_page() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let [top, ..] = two_headers(&mut engine, &mut docs);
        // The example's solid fill alone; its image fill has no picture here.
        let fills = json!([ondin_core::verbs::example("[Fill]").unwrap()[0]]);
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_fills",
            json!({ "nodes": [top], "fills": fills }),
        ));
        let page = docs.session.page.to_wire();
        let mut look = |args: Value| {
            let out = call(&mut engine, &mut docs, 1, "export_png", args);
            assert!(!out.is_error, "{:?}", out.content);
            match &out.content[0] {
                Content::Image { bytes, .. } => image::load_from_memory(bytes).unwrap().to_rgba8(),
                other => panic!("not an image: {other:?}"),
            }
        };
        let on = look(json!({ "region": [10, 10, 100, 50], "max": 400 }));
        assert_eq!(on.dimensions(), (200, 100));
        assert_eq!(
            on.get_pixel(100, 50).0,
            [0x33, 0x66, 0xcc, 0xff],
            "the header"
        );
        let off = look(json!({ "id": page, "region": [1000, 1000, 100, 50] }));
        assert_ne!(off.get_pixel(100, 50).0, [0x33, 0x66, 0xcc, 0xff]);

        for refused in [
            json!({ "id": top, "region": [0, 0, 10, 10] }),
            json!({ "region": [0, 0, 0, 10] }),
        ] {
            let out = call(&mut engine, &mut docs, 1, "export_png", refused.clone());
            assert!(out.is_error, "{refused} is refused");
        }
    }

    /// `export_png` never draws past `max`: a vast region, and a `max` over the
    /// ceiling, are refused; a layer stops at 2× and a page at 1×; a layer's
    /// far shadow counts toward `max`.
    ///
    /// flip: `png_viewport` not checking the pixel size → fails at the vast region
    /// (or runs out of memory first); the layer arm measuring `world_bounds` →
    /// fails at "the ink stays within".
    #[test]
    fn export_png_stays_within_max() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let [top, ..] = two_headers(&mut engine, &mut docs);
        for refused in [
            json!({ "region": [0, 0, 1e9, 1e9] }),
            json!({ "region": [0, 0, 1e9, 1e9], "max": 4096 }),
            json!({ "max": PNG_CEILING + 1 }),
        ] {
            let out = call(&mut engine, &mut docs, 1, "export_png", refused.clone());
            assert!(out.is_error, "{refused} is refused");
        }
        let mut size = |args: Value| {
            let out = call(&mut engine, &mut docs, 1, "export_png", args);
            assert!(!out.is_error, "{:?}", out.content);
            match &out.content[0] {
                Content::Image { bytes, .. } => image::load_from_memory(bytes)
                    .unwrap()
                    .to_rgba8()
                    .dimensions(),
                other => panic!("not an image: {other:?}"),
            }
        };
        let (w, h) = size(json!({ "id": top, "max": 4096 }));
        assert_eq!((w, h), (240, 160), "a 120×80 layer stops at 2×");
        let (w, h) = size(json!({ "max": 4096 }));
        assert_eq!(w, 520, "a page 520 wide stops at 1× ({w}×{h})");
        // A shadow far off the layer's box is drawn, so `max` counts it.
        let far = json!({ "nodes": [top], "effects": [{ "kind": "DropShadow", "color": "#000000",
                          "blur": 4, "offset": [3000, 3000], "spread": 0 }] });
        ok(&call(&mut engine, &mut docs, 1, "set_effects", far));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "export_png",
            json!({ "id": top, "max": 64 }),
        );
        let Some(Content::Image { bytes, .. }) = out.content.first() else {
            panic!("not an image: {:?}", out.content);
        };
        let (w, h) = image::load_from_memory(bytes)
            .unwrap()
            .to_rgba8()
            .dimensions();
        assert!(w.max(h) <= 65, "the ink stays within `max` ({w}×{h})");
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
    /// the properties it owns; cells and values in the short forms writes take.
    ///
    /// flip: `components` listing properties for sets alone → fails at "a plain
    /// main's properties are listed"; `variables` without `shorten` → fails at "a
    /// cell as set_variable_value takes it".
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
        let cells = v["cells"].as_object().unwrap();
        assert_eq!(
            cells.values().next(),
            Some(&json!({ "color": "#3366cc" })),
            "a cell as set_variable_value takes it"
        );

        let id = rect(&mut engine, &mut docs, 1, "Chip");
        let value = ondin_core::verbs::example("StyleValue").unwrap();
        let args = json!({ "value": value, "name": "Surface", "nodes": [id] });
        ok(&call(&mut engine, &mut docs, 1, "add_style", args));
        let styles = call(&mut engine, &mut docs, 1, "list_styles", json!({}));
        let s = &ok(&styles)["styles"][0];
        assert_eq!(s["name"], "Surface");
        assert_eq!(s["slot"], "fills");
        assert_eq!(s["users"], 1);
        assert!(
            !s["value"].to_string().contains("components"),
            "a style's value in the short forms: {s}"
        );

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

        // The person undoes it, so there is a step to redo.
        assert!(docs.session.undo());
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
        assert_eq!(s.history.undo_depth(), 0);
        // flip: `answer` not giving back the redo it held → fails here.
        assert_eq!(s.history.redo_depth(), 1, "the person's redo is kept");
        assert!(docs.session.redo(), "and still redoes");
        assert_eq!(docs.session.doc.get(id).unwrap().name(), "Hero");
    }

    /// A batch step can be `set`, naming an earlier step's layer by its label, and
    /// the batch is still one step; a `set` refused there takes back the steps
    /// before it.
    ///
    /// flip: `batch_step` refusing a tool that is no verb, as it did `set` → fails
    /// at the first batch's `ok`.
    #[test]
    fn a_batch_runs_set_as_a_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let make = json!({ "tool": "create_node", "as": "card",
            "args": { "parent": page, "kind": kind, "name": "Card" } });
        let set = json!({ "tool": "set",
            "args": { "nodes": ["@card"], "props": { "name": "Hero", "opacity": 0.5 } } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [make, set] }),
        );
        let card = ok(&out)["steps"][0]["created"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let s = &docs.session;
        let n = s.doc.get(NodeId::from_wire(&card).unwrap()).unwrap();
        assert_eq!((n.name(), n.opacity()), ("Hero", 0.5));
        assert_eq!(s.history.undo_depth(), 1, "one step");
        assert_eq!(s.history.done().last().unwrap().note.label, "batch");

        let layers = docs.session.doc.len();
        let bad = json!({ "tool": "set",
            "args": { "nodes": ["@card"], "props": { "name": "X", "colour": "#fff" } } });
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
    }

    /// A refused `batch` or `set` that took back the steps it made leaves `rev`
    /// where it was, for the caller and for another client, with no `changed`.
    ///
    /// flip: `handle` without its `rewind` → fails at "the batch's rev".
    #[test]
    fn a_refused_write_leaves_rev_where_it_was() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Card");
        let look = |e: &mut Engine, d: &mut One, conn| {
            let out = call(e, d, conn, "get_selection", json!({}));
            ok(&out).clone()
        };
        let seen = look(&mut engine, &mut docs, 2);
        let rev = look(&mut engine, &mut docs, 1)["rev"].clone();
        let renamed = json!({ "tool": "set_name", "args": { "node": id, "name": "Hero" } });
        let bad = json!({ "tool": "set_name", "args": { "node": "@nobody", "name": "X" } });
        let args = json!({ "steps": [renamed, bad] });
        let out = call(&mut engine, &mut docs, 1, "batch", args);
        assert!(out.is_error, "the fixture's batch is refused");
        assert_eq!(docs.session.history.undo_depth(), 1, "after a step it made");
        assert_eq!(json!(docs.session.revision()), rev, "the batch's rev");
        let props = json!({ "name": "Hero", "opacity": "loud" });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [id], "props": props }),
        );
        assert!(out.is_error, "the fixture's set is refused");
        assert_eq!(json!(docs.session.revision()), rev, "the set's rev");
        let mine = look(&mut engine, &mut docs, 1);
        assert_eq!((&mine["rev"], mine.get("changed")), (&rev, None));
        let theirs = look(&mut engine, &mut docs, 2);
        assert_eq!(theirs["rev"], seen["rev"]);
        assert!(theirs.get("changed").is_none(), "{theirs}");
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
    /// passes, listing a passing one only when asked for `all`.
    ///
    /// flip: `check_contrast` keeping every finding without `all` → fails at "a
    /// pass is not listed".
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
        let quiet = ok(&out);
        assert_eq!(
            quiet["findings"],
            json!([]),
            "a pass is not listed: {quiet}"
        );
        assert!(quiet["measured"].as_u64() > Some(0), "but counted: {quiet}");
        let args = json!({ "all": true });
        let out = call(&mut engine, &mut docs, 1, "check_contrast", args);
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
        // A text too faint to read is listed without `all`, and counted.
        // flip: `check_contrast` keeping passes in its default list → fails at
        // "only the failing one".
        let faint = call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Faint", "y": 100 }),
        );
        let faint = ok(&faint)["created"][0].clone();
        let fills = json!({ "nodes": [faint], "fills": ["#eeeeee"] });
        ok(&call(&mut engine, &mut docs, 1, "set_fills", fills));
        let out = call(&mut engine, &mut docs, 1, "check_contrast", json!({}));
        let report = ok(&out);
        assert_eq!(report["failing"], 1, "{report}");
        let listed = report["findings"].as_array().unwrap();
        assert_eq!(listed.len(), 1, "only the failing one: {report}");
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
