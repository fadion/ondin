//! What an assistant chat may do: its access level, the tools that level lists,
//! and the grants the window holds by chat key and by process.
//!
//! The host enforces a level, not the harness: a call above it is refused
//! whichever way it arrives, by name, through `call` or as a `batch` step. A
//! connection from a process some chat owns gets that chat's level and key
//! whatever key it sends, so an agent cannot shed its level by starting its own
//! proxy.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the assistant panel grants and labels levels")
)]

use ondin_mcp::queue::Call;
use ondin_mcp::tools::{ASSISTANT, CORE, PHASE_1, PHASE_2, PHASE_3};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// How far a chat may act on the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Level {
    /// Read and export.
    Look,
    /// Read, export and propose edits the person applies.
    #[default]
    Propose,
    /// Read, export and edit, components, variables and styles included.
    Edit,
}

impl Level {
    pub(crate) const ALL: [Level; 3] = [Level::Look, Level::Propose, Level::Edit];

    /// The level's word in settings and on the wire.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Level::Look => "look",
            Level::Propose => "propose",
            Level::Edit => "edit",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Level> {
        Self::ALL.into_iter().find(|l| l.key() == key)
    }

    /// The level's name in the UI.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Level::Look => "Look",
            Level::Propose => "Propose",
            Level::Edit => "Edit",
        }
    }
}

/// Whether `level` may run `tool`. `call` is a door, checked by the tool it
/// names; Phase 4, the kits and the library, is never a chat's.
pub(crate) fn allowed(level: Level, tool: &str) -> bool {
    if tool == "call" || PHASE_1.contains(&tool) {
        return true;
    }
    match level {
        Level::Look => false,
        Level::Propose => ASSISTANT.contains(&tool),
        Level::Edit => PHASE_2.contains(&tool) || PHASE_3.contains(&tool),
    }
}

/// The bare tool names `tools/list` shows a chat at `level`, which is also the
/// harness's allow-list: the core set within the level, with `describe_tools`
/// and `call` to reach the level's other tools.
pub(crate) fn listed(level: Level) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = CORE
        .iter()
        .chain(&ASSISTANT)
        .copied()
        .filter(|t| allowed(level, t))
        .collect();
    for door in ["describe_tools", "call"] {
        if !names.contains(&door) {
            names.push(door);
        }
    }
    names
}

/// The definitions [`listed`] names.
pub(crate) fn definitions(level: Level) -> Vec<Value> {
    listed(level)
        .into_iter()
        .filter_map(ondin_mcp::schema::definition)
        .collect()
}

/// Every tool `call` reaches: the tool it names (through `call`), and each step
/// of a `batch`. A name that is missing is left to the tool to refuse.
fn reached(tool: &str, args: &Value) -> Vec<String> {
    match tool {
        "call" => match args.get("tool").and_then(Value::as_str) {
            Some(inner) if inner != "call" => {
                reached(inner, args.get("args").unwrap_or(&Value::Null))
            }
            _ => Vec::new(),
        },
        "batch" => {
            let steps = args.get("steps").and_then(Value::as_array);
            std::iter::once(tool.to_owned())
                .chain(
                    steps
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s.get("tool").and_then(Value::as_str))
                        .map(str::to_owned),
                )
                .collect()
        }
        _ => vec![tool.to_owned()],
    }
}

/// Why `call` is refused at `level`, `None` when every tool it reaches is
/// within it.
pub(crate) fn refused(level: Level, call: &Call) -> Option<String> {
    let above = reached(&call.tool, &call.args)
        .into_iter()
        .find(|t| !allowed(level, t))?;
    let instead = match level {
        Level::Propose => " Propose the change with `propose_edit`; the person applies it.",
        Level::Look | Level::Edit => " Ask the person if it is needed.",
    };
    Some(format!(
        "`{above}` is beyond this chat's access ({}): nothing was changed.{instead}",
        level.label()
    ))
}

/// Answers whether a process belongs to a chat: its harness or anything the
/// harness started.
pub(crate) type Owns = Arc<dyn Fn(u32) -> bool + Send + Sync>;

struct Grant {
    key: String,
    level: Level,
    owns: Owns,
}

/// The levels the window granted its chats, shared with every connection's
/// thread, which reads them for `tools/list`.
#[derive(Clone, Default)]
pub(crate) struct Grants(Arc<Mutex<Vec<Grant>>>);

impl Grants {
    /// Give the chat `key` `level`, and every process `owns` claims; replaces
    /// what `key` held.
    pub(crate) fn grant(&self, key: &str, level: Level, owns: Owns) {
        let mut grants = self.0.lock().unwrap_or_else(|e| e.into_inner());
        grants.retain(|g| g.key != key);
        grants.push(Grant {
            key: key.to_owned(),
            level,
            owns,
        });
    }

    pub(crate) fn revoke(&self, key: &str) {
        let mut grants = self.0.lock().unwrap_or_else(|e| e.into_inner());
        grants.retain(|g| g.key != key);
    }

    /// The chat key and level a caller acts under: the chat owning `pid`
    /// first, whatever key the caller gave, else the grant of the key it gave.
    /// `None` for a caller no grant covers.
    pub(crate) fn resolve(&self, key: Option<&str>, pid: Option<u32>) -> Option<(String, Level)> {
        // Copied out, so no `owns` runs under the lock.
        let grants: Vec<(String, Level, Owns)> = {
            let grants = self.0.lock().unwrap_or_else(|e| e.into_inner());
            grants
                .iter()
                .map(|g| (g.key.clone(), g.level, g.owns.clone()))
                .collect()
        };
        if let Some(pid) = pid
            && let Some((key, level, _)) = grants.iter().find(|(_, _, owns)| owns(pid))
        {
            return Some((key.clone(), *level));
        }
        let key = key?;
        grants
            .into_iter()
            .find(|(k, _, _)| k == key)
            .map(|(k, level, _)| (k, level))
    }

    /// The `tools/list` hook for the endpoint: a granted caller's level's list.
    pub(crate) fn lists(&self) -> ondin_mcp::queue::Lists {
        let grants = self.clone();
        Arc::new(move |key, pid| {
            grants
                .resolve(key, pid)
                .map(|(_, level)| definitions(level))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Engine;
    use crate::mcp::tests::{One, keyed, ok};
    use serde_json::json;

    fn nobody() -> Owns {
        Arc::new(|_| false)
    }

    /// Each level lists its own tools and none above it: Look reads, Propose
    /// adds `propose_edit`, Edit the writes; no level lists Phase 4.
    ///
    /// flip: `allowed` answering Phase 2 at Propose → fails at "no write at
    /// Propose".
    #[test]
    fn each_level_lists_only_its_tools() {
        let look = listed(Level::Look);
        let propose = listed(Level::Propose);
        let edit = listed(Level::Edit);
        for list in [&look, &propose, &edit] {
            assert!(list.contains(&"get_node") && list.contains(&"call"));
            assert!(list.contains(&"describe_tools"));
            assert!(
                !list.iter().any(|t| ondin_mcp::tools::PHASE_4.contains(t)),
                "Phase 4 is never listed: {list:?}"
            );
        }
        assert!(!look.contains(&"propose_edit") && !look.contains(&"set"));
        assert!(propose.contains(&"propose_edit"));
        assert!(
            !propose.iter().any(|t| *t != "call" && PHASE_2.contains(t)),
            "no write at Propose: {propose:?}"
        );
        assert!(edit.contains(&"set") && edit.contains(&"batch"));
        assert!(!edit.contains(&"propose_edit"));
        // The allow-list is what the harness lets through: every name listed
        // has a definition, so the list and the allow-list are one.
        for level in Level::ALL {
            assert_eq!(definitions(level).len(), listed(level).len());
            assert_eq!(Level::from_key(level.key()), Some(level));
        }
        assert_eq!(Level::default(), Level::Propose);
    }

    /// A key granted Look is refused a write by name, through `call` and as a
    /// `batch` step, and the document stays as it was; the same key granted
    /// Edit writes. Each call, refused or not, is one line of the agent log.
    ///
    /// flip: `reached` without its `call` arm → fails at "through call".
    #[test]
    fn a_key_above_its_level_is_refused_every_way() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let log = std::env::temp_dir()
            .join(format!("ondin-access-log-{}", std::process::id()))
            .join("agent_log.jsonl");
        let _ = std::fs::remove_file(&log);
        engine.log = crate::mcp::agent_log::Log::at(log.clone(), u64::MAX);
        let grants = engine.grants();
        grants.grant("chat-1", Level::Look, nobody());
        let chat = Some("chat-1");
        ok(&keyed(
            &mut engine,
            &mut docs,
            1,
            chat,
            "use_document",
            json!({ "document": "a" }),
        ));
        let page = docs.session.doc.first_page().to_wire();
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let create = json!({ "parent": page, "kind": kind, "name": "R" });
        let rev = docs.session.revision();
        let direct = keyed(
            &mut engine,
            &mut docs,
            1,
            chat,
            "create_node",
            create.clone(),
        );
        assert!(direct.is_error, "by name");
        let through = keyed(
            &mut engine,
            &mut docs,
            1,
            chat,
            "call",
            json!({ "tool": "create_node", "args": create }),
        );
        assert!(through.is_error, "through call");
        // `batch` is itself a write; at Edit a Phase 4 step inside it is caught.
        grants.grant("chat-1", Level::Edit, nobody());
        let batch = json!({ "steps": [
            { "tool": "create_node", "args": create },
            { "tool": "make_kit", "args": {} },
        ] });
        let stepped = keyed(&mut engine, &mut docs, 1, chat, "batch", batch.clone());
        assert!(
            format!("{:?}", stepped.content).contains("`make_kit` is beyond"),
            "as a step: {:?}",
            stepped.content
        );
        let wrapped = keyed(
            &mut engine,
            &mut docs,
            1,
            chat,
            "call",
            json!({ "tool": "batch", "args": batch }),
        );
        assert!(wrapped.is_error, "a batch through call");
        assert_eq!(docs.session.revision(), rev, "nothing moved");
        assert_eq!(docs.session.history.undo_depth(), 0);
        ok(&keyed(
            &mut engine,
            &mut docs,
            1,
            chat,
            "create_node",
            create,
        ));
        assert_eq!(docs.session.history.undo_depth(), 1, "Edit writes");
        // Every call the chat made is a line of the agent log, refusals too,
        // a call through `call` under the tool it named.
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let seen: Vec<(&str, bool)> = lines
            .iter()
            .map(|l| (l["tool"].as_str().unwrap(), l["ok"].as_bool().unwrap()))
            .collect();
        assert_eq!(
            seen,
            [
                ("use_document", true),
                ("create_node", false),
                ("create_node", false),
                ("batch", false),
                ("batch", false),
                ("create_node", true),
            ]
        );
        assert_eq!(lines[1]["key"], "chat-1");
        assert_eq!(lines[1]["document"], "/a.ondin");
        let _ = std::fs::remove_dir_all(log.parent().unwrap());
    }

    /// A process a chat owns gets that chat's level and key whatever key it
    /// sends, or none: it reads the chat's document and is refused a write.
    ///
    /// flip: `Grants::resolve` checking the key before the process → fails at
    /// "the other key's level".
    #[test]
    fn an_owned_process_gets_the_chats_level_whatever_its_key() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let grants = engine.grants();
        grants.grant("chat-1", Level::Look, Arc::new(|pid| pid == 4242));
        grants.grant("other", Level::Edit, nobody());
        assert_eq!(
            grants.resolve(Some("other"), Some(4242)),
            Some(("chat-1".into(), Level::Look)),
            "the other key's level"
        );
        assert_eq!(grants.resolve(None, Some(1)), None);
        let (reply, _answer) = std::sync::mpsc::sync_channel(1);
        let mut call = Call::new(
            9,
            Some("other".into()),
            "use_document",
            json!({ "document": "a" }),
            reply,
        );
        call.pid = Some(4242);
        ok(&engine.handle(&mut docs, call));
        // The chat's key holds the document now, read by a keyed call.
        ok(&keyed(
            &mut engine,
            &mut docs,
            2,
            Some("chat-1"),
            "get_selection",
            json!({}),
        ));
        let page = docs.session.doc.first_page().to_wire();
        let (reply, _answer) = std::sync::mpsc::sync_channel(1);
        let mut write = Call::new(
            9,
            None,
            "set_name",
            json!({ "node": page, "name": "x" }),
            reply,
        );
        write.pid = Some(4242);
        let out = engine.handle(&mut docs, write);
        assert!(out.is_error, "refused at Look");
        // Listed as the chat is, through the endpoint's hook.
        let listed = (grants.lists())(Some("other"), Some(4242)).unwrap();
        assert!(!listed.iter().any(|d| d["name"] == "set"));
        grants.revoke("chat-1");
        assert_eq!(grants.resolve(None, Some(4242)), None, "revoked");
    }
}
