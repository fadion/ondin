//! One document's conversation with the assistant: its messages, and where they
//! are kept. A chat is the person's, not the work's, so it lives in the machine
//! data folder keyed by the document's id, never in the `.ondin` file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The most messages a saved thread keeps; the oldest go first.
pub(crate) const KEPT: usize = 100;

/// What a saved tool result reads as: results can be large and are the
/// document's content, so only the call is kept.
pub(crate) const RESULT_NOT_SAVED: &str = "(result not saved)";

/// A layer attached to a question: a reference the agent reads through the
/// tools, never a copy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Chip {
    /// The layer's wire id (`4:31`).
    pub id: String,
    pub name: String,
    /// The node kind's word (`frame`, `text`).
    pub kind: String,
    pub w: f64,
    pub h: f64,
}

/// A tool call's state on its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ToolState {
    Running,
    Done,
    Failed,
}

/// One tool call in an agent's turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ToolRow {
    /// The harness's own id, which pairs the result with its call.
    pub id: String,
    /// The bare tool name.
    pub name: String,
    /// The argument it was given, in the agent's terms: a layer's id or a query.
    pub arg: String,
    pub state: ToolState,
    /// The result cut short: a line count, *image*, or the error.
    pub result: String,
    /// The proposal a `propose_edit` call recorded.
    #[serde(default)]
    pub proposal: Option<u64>,
}

/// A part of an agent's turn, in the order it arrived.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Part {
    Text(String),
    Tool(ToolRow),
}

/// A finished turn's footing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Stats {
    pub seconds: f64,
    pub tokens: Option<u64>,
}

/// The app's own words beside a turn, never sent back to the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Note {
    /// A line of plain fact: a stopped turn, a replay withheld.
    Said(String),
    /// The harness ended before answering. `question` is the person's message it
    /// was answering, for *Ask again*; `details` the lines that were not JSON.
    Failed {
        text: String,
        question: usize,
        details: String,
    },
}

/// One message in the thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Msg {
    Person {
        text: String,
        chips: Vec<Chip>,
    },
    Agent {
        /// The speaker the turn was written by, kept with it.
        speaker: String,
        /// The harness's key, so a restored thread knows whose prose it holds.
        harness: String,
        parts: Vec<Part>,
        /// `None` while it streams, or when it was stopped.
        stats: Option<Stats>,
    },
    Note(Note),
}

impl Msg {
    /// The agent's prose in this message, its text parts joined.
    pub(crate) fn prose(&self) -> String {
        match self {
            Msg::Agent { parts, .. } => parts
                .iter()
                .filter_map(|p| match p {
                    Part::Text(t) => Some(t.as_str()),
                    Part::Tool(_) => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            Msg::Person { text, .. } => text.clone(),
            Msg::Note(_) => String::new(),
        }
    }
}

/// The thread as saved: tool results replaced, at most [`KEPT`] messages, and
/// nothing after a turn still streaming.
pub(crate) fn for_saving(msgs: &[Msg], streaming: Option<usize>) -> Vec<Msg> {
    let end = streaming.map_or(msgs.len(), |i| i.min(msgs.len()));
    let mut kept: Vec<Msg> = msgs[..end]
        .iter()
        .map(|m| match m {
            Msg::Agent {
                speaker,
                harness,
                parts,
                stats,
            } => Msg::Agent {
                speaker: speaker.clone(),
                harness: harness.clone(),
                parts: parts
                    .iter()
                    .map(|p| match p {
                        Part::Tool(t) => Part::Tool(ToolRow {
                            result: RESULT_NOT_SAVED.into(),
                            state: match t.state {
                                ToolState::Running => ToolState::Done,
                                s => s,
                            },
                            ..t.clone()
                        }),
                        p => p.clone(),
                    })
                    .collect(),
                stats: *stats,
            },
            m => m.clone(),
        })
        .collect();
    if kept.len() > KEPT {
        kept.drain(..kept.len() - KEPT);
    }
    // A thread opening on an answer or a note has lost its question; start at a
    // question so *Ask again* indices hold.
    while kept
        .first()
        .is_some_and(|m| !matches!(m, Msg::Person { .. }))
    {
        kept.remove(0);
    }
    kept
}

/// The folder the chats live in.
fn dir() -> Option<PathBuf> {
    Some(crate::machine_dir::root()?.join("chats"))
}

/// The file a document's chat is kept in: by its library id, which survives a
/// rename, else by a hash of its path.
pub(crate) fn file_for(doc_id: Option<&str>, path: Option<&Path>) -> Option<PathBuf> {
    let name = match (doc_id, path) {
        (Some(id), _) if is_plain(id) => id.to_owned(),
        (_, Some(p)) => format!("p{:016x}", path_hash(p)),
        _ => return None,
    };
    Some(dir()?.join(format!("{name}.json")))
}

/// An id safe as a file name.
fn is_plain(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// FNV-1a over the path's text: stable across runs, unlike `DefaultHasher`.
fn path_hash(p: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in p.to_string_lossy().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Read a saved chat; an unreadable one reads as empty.
pub(crate) fn load(file: &Path) -> Vec<Msg> {
    std::fs::read(file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Write a chat, or remove its file when it is empty.
pub(crate) fn save(file: &Path, msgs: &[Msg]) {
    if msgs.is_empty() {
        let _ = std::fs::remove_file(file);
        return;
    }
    let Ok(bytes) = serde_json::to_vec(msgs) else {
        return;
    };
    if let Some(parent) = file.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let tmp = file.with_extension("json.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(text: &str, tool: Option<&str>) -> Msg {
        let mut parts = vec![Part::Text(text.into())];
        if let Some(name) = tool {
            parts.push(Part::Tool(ToolRow {
                id: "t1".into(),
                name: name.into(),
                arg: "4:31".into(),
                state: ToolState::Done,
                result: "+24 lines".into(),
                proposal: None,
            }));
        }
        Msg::Agent {
            speaker: "Claude".into(),
            harness: "claude".into(),
            parts,
            stats: None,
        }
    }

    fn person(text: &str) -> Msg {
        Msg::Person {
            text: text.into(),
            chips: Vec::new(),
        }
    }

    /// A saved thread keeps the calls but not their results.
    /// flip: `for_saving` keeping `t.result` → fails at the result.
    #[test]
    fn tool_results_are_not_saved() {
        let saved = for_saving(&[person("q"), agent("a", Some("get_node"))], None);
        let Msg::Agent { parts, .. } = &saved[1] else {
            panic!("not an agent turn");
        };
        let Part::Tool(t) = &parts[1] else {
            panic!("not a tool row");
        };
        assert_eq!(t.name, "get_node");
        assert_eq!(t.result, RESULT_NOT_SAVED);
    }

    /// The oldest messages go past the cap, and the thread starts at a question.
    /// flip: dropping the `while` → fails at the first message.
    #[test]
    fn the_cap_drops_the_oldest_and_starts_on_a_question() {
        let mut msgs = Vec::new();
        for i in 0..60 {
            msgs.push(person(&format!("q{i}")));
            msgs.push(agent("a", None));
        }
        msgs.insert(1, Msg::Note(Note::Said("x".into())));
        let saved = for_saving(&msgs, None);
        assert!(saved.len() <= KEPT);
        assert!(matches!(saved[0], Msg::Person { .. }));
    }

    /// A turn still streaming is not saved, nor anything after it.
    #[test]
    fn a_streaming_turn_is_not_saved() {
        let saved = for_saving(&[person("q"), agent("half", None)], Some(1));
        assert_eq!(saved, vec![person("q")]);
    }

    /// A document's chat file is named by its id, else by its path.
    #[test]
    fn the_file_is_named_by_id_then_path() {
        let by_id = file_for(Some("abc-123"), Some(Path::new("a.ondin")));
        let by_path = file_for(None, Some(Path::new("a.ondin")));
        let odd = file_for(Some("../x"), Some(Path::new("a.ondin")));
        if let (Some(a), Some(b), Some(c)) = (by_id, by_path, odd) {
            assert!(a.ends_with("abc-123.json"));
            assert_eq!(b, c, "an id unsafe as a name falls back to the path");
        }
        assert!(file_for(None, None).is_none());
    }
}
