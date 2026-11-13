//! `propose_edit`: an edit a chat asks for and the person applies.
//!
//! The calls are checked by running them on a copy of the document, so the
//! person's document, its revision and their redo stay as they were; applying
//! runs them again on the document as it is then, as one undo step.

use super::{Connection, write};
use crate::session::EditorSession;
use ondin_core::{Node, NodeId};
use serde_json::{Value, json};
use std::path::PathBuf;

/// An edit a chat proposed, kept until the person applies or drops it.
#[derive(Clone)]
pub(crate) struct Proposal {
    pub id: u64,
    /// The chat's key, when it gave one.
    pub key: Option<String>,
    pub document: PathBuf,
    /// The name the applied undo step takes.
    pub title: String,
    /// One line per call, in the words History uses for its step.
    pub changes: Vec<String>,
    /// Each call's verb and arguments.
    pub calls: Vec<(String, Value)>,
    /// The document's revision when it was proposed.
    pub rev: u64,
    /// The layers it writes that were there when it was proposed.
    pub writes: Vec<NodeId>,
    /// Each of `writes` as it was then, for [`stale`].
    pub before: Vec<Node>,
    /// The client's name, which the applied step's History row shows.
    pub client: String,
}

/// An applied proposal's undo step, for [`is_last`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Applied {
    serial: u64,
    depth: usize,
}

/// The verbs a proposal may call: the write tools, never the kits and library.
fn proposable(verb: &str) -> bool {
    ondin_core::verbs::find(verb).is_some()
        && (ondin_mcp::tools::PHASE_2.contains(&verb) || ondin_mcp::tools::PHASE_3.contains(&verb))
}

/// `propose_edit`'s `calls` read as verbs and arguments, `base_rev` dropped: a
/// proposal is checked again when it is applied.
pub(super) fn calls(args: &Value) -> Result<Vec<(String, Value)>, String> {
    let Some(Value::Array(list)) = args.get("calls") else {
        return Err("`calls` is a list of `{ verb, args }`".into());
    };
    if list.is_empty() {
        return Err("`calls` is empty".into());
    }
    list.iter()
        .enumerate()
        .map(|(i, c)| {
            let n = i + 1;
            if let Some(extra) = c
                .as_object()
                .and_then(|m| m.keys().find(|k| !["verb", "args"].contains(&k.as_str())))
            {
                return Err(format!("call {n}: a call takes no `{extra}`"));
            }
            let verb = c
                .get("verb")
                .and_then(Value::as_str)
                .ok_or(format!("call {n}: a call names its `verb`"))?;
            if !proposable(verb) {
                return Err(format!(
                    "call {n}: `{verb}` is no write tool a proposal can call"
                ));
            }
            let mut args = c.get("args").cloned().unwrap_or_else(|| json!({}));
            if let Some(map) = args.as_object_mut() {
                map.remove("base_rev");
            }
            Ok((verb.to_owned(), args))
        })
        .collect()
}

/// Run `calls` on `s` in turn, each committed as its own step, answering each
/// one's line and the subjects of its step. On a refusal the steps already made
/// are taken back.
fn run(
    s: &mut EditorSession,
    client: &str,
    calls: &[(String, Value)],
) -> Result<(Vec<String>, Vec<NodeId>, usize), String> {
    let conn = Connection::default();
    let mut lines = Vec::new();
    let mut subjects: Vec<NodeId> = Vec::new();
    let mut made = 0;
    for (i, (verb, args)) in calls.iter().enumerate() {
        let depth = s.history.undo_depth();
        let said = match write(&conn, s, client, verb, args) {
            Ok(super::Answer::Json(answer)) => answer
                .get("said")
                .and_then(Value::as_str)
                .map(str::to_owned),
            Ok(_) => None,
            Err(why) => {
                for _ in 0..made {
                    s.take_back();
                }
                return Err(format!("call {} (`{verb}`): {why}", i + 1));
            }
        };
        let pushed = s.history.undo_depth().saturating_sub(depth);
        made += pushed;
        let step = (pushed > 0).then(|| s.history.done().next_back()).flatten();
        let mut line = match step {
            Some(step) => {
                for id in &step.note.subjects {
                    if !subjects.contains(id) {
                        subjects.push(*id);
                    }
                }
                let n = &step.note;
                match (&n.subject, n.subjects.len()) {
                    (None, _) => n.label.clone(),
                    (Some(name), 0 | 1) => format!("{} · {name}", n.label),
                    (Some(name), more) => format!("{} · {name} +{}", n.label, more - 1),
                }
            }
            None => format!("{verb} · changes nothing"),
        };
        if let Some(said) = said {
            line = format!("{line}: {said}");
        }
        lines.push(line);
    }
    Ok((lines, subjects, made))
}

/// A copy of `s`'s document to try an edit on, with ids of its own.
fn scratch(s: &EditorSession) -> EditorSession {
    let mut t = EditorSession::new();
    t.doc = s.doc.clone();
    t.resolved = s.resolved.clone();
    t.page = s.page;
    t.ids = ondin_core::IdSource::new(crate::session::random_actor());
    ondin_core::reserve_existing_ids(&t.doc, &mut t.ids);
    t
}

/// Check `calls` against `s` without changing it: the [`Proposal`] they make,
/// less its id, key, document and client, which the engine knows. Answers the
/// first refusal instead.
pub(super) fn check(
    s: &EditorSession,
    client: &str,
    title: &str,
    calls: Vec<(String, Value)>,
) -> Result<Proposal, String> {
    let mut trial = scratch(s);
    let (changes, subjects, _) = run(&mut trial, client, &calls)?;
    let (writes, before): (Vec<NodeId>, Vec<Node>) = subjects
        .into_iter()
        .filter_map(|id| s.doc.get(id).map(|n| (id, n.clone())))
        .unzip();
    Ok(Proposal {
        id: 0,
        key: None,
        document: PathBuf::new(),
        title: title.to_owned(),
        changes,
        calls,
        rev: s.revision(),
        writes,
        before,
        client: client.to_owned(),
    })
}

/// Apply `p` to `session` as one undo step named by its title, made over MCP so
/// History marks it as the assistant's. Every call is run against the document
/// as it is now; the first refusal takes back the rest and is answered.
pub(crate) fn apply(session: &mut EditorSession, p: &Proposal) -> Result<Applied, String> {
    let mark = session.mark();
    let made = match run(session, &p.client, &p.calls) {
        Ok((_, _, made)) => made,
        Err(why) => {
            session.rewind(&mark);
            return Err(format!("{why}. Nothing was changed."));
        }
    };
    if made == 0 {
        return Err("The proposal changes nothing now.".into());
    }
    session.history.join_last(made);
    if let Some(note) = session.history.current_note_mut() {
        note.label = p.title.clone();
        note.origin = ondin_core::Origin::Mcp {
            client: p.client.clone(),
            tool: "propose_edit".into(),
        };
    }
    let serial = session
        .history
        .done()
        .next_back()
        .map(|s| s.serial)
        .ok_or("the applied step is missing")?;
    Ok(Applied {
        serial,
        depth: session.history.undo_depth(),
    })
}

/// The name of a layer `p` writes that changed since it was proposed, `None`
/// while it still applies as it was checked.
pub(crate) fn stale(session: &EditorSession, p: &Proposal) -> Option<String> {
    if session.revision() == p.rev {
        return None;
    }
    p.writes
        .iter()
        .zip(&p.before)
        .find(|(id, was)| session.doc.get(**id) != Some(*was))
        .map(|(_, was)| was.name().to_owned())
}

/// Whether the step `a` made is still the last one done, so undoing it takes
/// back the proposal alone.
pub(crate) fn is_last(session: &EditorSession, a: &Applied) -> bool {
    session.history.undo_depth() == a.depth
        && session.history.done().next_back().map(|s| s.serial) == Some(a.serial)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Engine;
    use crate::mcp::tests::{One, call, ok, open, rect};

    /// A `propose_edit` of `calls` titled *Rename the card*, through the engine.
    fn propose(engine: &mut Engine, docs: &mut One, calls: Value) -> ondin_mcp::wire::Output {
        call(
            engine,
            docs,
            1,
            "propose_edit",
            json!({ "title": "Rename the card", "calls": calls }),
        )
    }

    /// A proposal answers its id and a line per call, and leaves the document,
    /// its revision, its history and the redo as they were; a refused call is
    /// answered and records nothing.
    ///
    /// flip: `check` running the calls on the person's session, then taking
    /// them back and rewinding → fails at "the redo kept".
    #[test]
    fn a_proposal_leaves_the_document_untouched() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let card = rect(&mut engine, &mut docs, 1, "Card");
        let id = NodeId::from_wire(&card).unwrap();
        // A redo waiting, which a commit would drop.
        let extra = rect(&mut engine, &mut docs, 1, "Extra");
        ok(&call(&mut engine, &mut docs, 1, "undo", json!({})));
        let rev = docs.session.revision();
        let depth = docs.session.history.undo_depth();
        let out = propose(
            &mut engine,
            &mut docs,
            json!([
                { "verb": "set_name", "args": { "node": card, "name": "Hero" } },
                { "verb": "set_opacity", "args": { "nodes": [card], "opacity": 0.5 } },
            ]),
        );
        let answer = ok(&out).clone();
        let changes = answer["changes"].as_array().unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0], "set_name · Hero");
        assert_eq!(docs.session.revision(), rev, "the revision held");
        assert_eq!(docs.session.doc.get(id).unwrap().name(), "Card");
        assert_eq!(docs.session.history.undo_depth(), depth);
        assert_eq!(docs.session.history.undone().count(), 1, "the redo kept");
        let p = engine
            .proposal(answer["proposal"].as_u64().unwrap())
            .unwrap();
        assert_eq!(p.writes, [id]);
        assert_eq!(p.document, docs.path);
        assert!(NodeId::from_wire(&extra).is_some());

        let refused = propose(
            &mut engine,
            &mut docs,
            json!([
                { "verb": "set_name", "args": { "node": card, "name": "Hero" } },
                { "verb": "set_name", "args": { "node": card } },
            ]),
        );
        assert!(
            refused.is_error && format!("{:?}", refused.content).contains("call 2"),
            "{:?}",
            refused.content
        );
        let kit = propose(
            &mut engine,
            &mut docs,
            json!([{ "verb": "make_kit", "args": {} }]),
        );
        assert!(kit.is_error, "Phase 4 is never proposed");
        assert_eq!(docs.session.revision(), rev);
    }

    /// Applying is one undo step named by the title and marked as MCP's, which
    /// `is_last` follows; `stale` names a written layer changed since the
    /// proposal, and is quiet about a change elsewhere.
    ///
    /// flip: `apply` without `join_last` → fails at "one step"; `stale`
    /// answering `None` whenever the layer still exists → fails at "Card".
    #[test]
    fn apply_is_one_mcp_step_and_stale_sees_a_written_layer_change() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let card = rect(&mut engine, &mut docs, 1, "Card");
        let other = rect(&mut engine, &mut docs, 1, "Other");
        let answer = ok(&propose(
            &mut engine,
            &mut docs,
            json!([
                { "verb": "set_name", "args": { "node": card, "name": "Hero" } },
                { "verb": "set_opacity", "args": { "nodes": [card], "opacity": 0.5 } },
            ]),
        ))
        .clone();
        let p = &engine
            .proposal(answer["proposal"].as_u64().unwrap())
            .unwrap()
            .clone();

        // A change elsewhere leaves it applying.
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_name",
            json!({ "node": other, "name": "Moved" }),
        ));
        assert_eq!(stale(&docs.session, p), None);

        let depth = docs.session.history.undo_depth();
        let applied = apply(&mut docs.session, p).unwrap();
        assert_eq!(docs.session.history.undo_depth(), depth + 1, "one step");
        let step = docs.session.history.done().next_back().unwrap();
        assert_eq!(step.note.label, "Rename the card");
        assert!(matches!(
            &step.note.origin,
            ondin_core::Origin::Mcp { tool, .. } if tool == "propose_edit"
        ));
        let id = NodeId::from_wire(&card).unwrap();
        assert_eq!(docs.session.doc.get(id).unwrap().name(), "Hero");
        assert!(is_last(&docs.session, &applied));
        // The written layer changed since: stale, by the name it was proposed on.
        assert_eq!(stale(&docs.session, p).as_deref(), Some("Card"));
        assert!(docs.session.undo());
        assert!(!is_last(&docs.session, &applied), "undone");
    }
}
