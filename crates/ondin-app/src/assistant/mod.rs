//! The assistant (`docs/design/assistant.md`): a chat with a coding agent the person already
//! uses, started as a child process sealed to Ondin's MCP tools and shown in a
//! panel at the rail's foot.
//!
//! Ondin holds no model and no key. This module keeps one conversation per
//! document, starts and stops the harness's session, turns its events into the
//! thread, and decides when the composer can send.

pub(crate) mod canvas;
pub(crate) mod markdown;
pub(crate) mod panel;
pub(crate) mod prompt;
pub(crate) mod thread;

pub(crate) use crate::mcp::access::Level;
pub(crate) use ondin_ai as ai;

use crate::app::OndinApp;
use ondin_core::NodeId;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use thread::{Chip, Msg, Note, Part, Stats, ToolRow, ToolState};

/// The panel's width, and the range its drag edge keeps it in.
pub(crate) const WIDTH: f32 = 400.0;
pub(crate) const MIN_W: f32 = 360.0;
pub(crate) const MAX_W: f32 = 640.0;

/// How many tool rows a turn shows before folding the rest.
pub(crate) const TOOLS_SHOWN: usize = 3;

/// Settings ▸ Assistant, per machine. None of it is a secret.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AssistantPrefs {
    /// The harness's key; empty is *Off*. An unknown key reads as off and is
    /// kept until the person picks, never turned into another harness.
    pub agent: String,
    /// The program's path, empty for found automatically.
    pub program: String,
    /// Passed verbatim; empty omits the flag.
    pub model: String,
    /// In the harness's own words; empty for its default.
    pub effort: String,
    /// The access level a new chat starts at, by key.
    pub access: String,
    /// Added to every system prompt, after Ondin's own.
    pub instructions: String,
    /// The panel's width.
    pub width: f32,
}

impl Default for AssistantPrefs {
    fn default() -> Self {
        Self {
            agent: String::new(),
            program: String::new(),
            model: String::new(),
            effort: String::new(),
            access: Level::default().key().into(),
            instructions: String::new(),
            width: WIDTH,
        }
    }
}

impl AssistantPrefs {
    /// The harness picked, `None` for *Off* or a key this build does not know.
    pub(crate) fn harness(&self) -> Option<ai::Harness> {
        ai::Harness::from_key(&self.agent)
    }

    /// The program override, `None` for found automatically.
    pub(crate) fn program_path(&self) -> Option<PathBuf> {
        let p = self.program.trim();
        (!p.is_empty()).then(|| PathBuf::from(p))
    }

    /// The default access level.
    pub(crate) fn level(&self) -> Level {
        Level::from_key(&self.access).unwrap_or_default()
    }
}

/// What the session was started with; a change to any of it restarts it.
#[derive(Clone, Debug, PartialEq)]
struct Started {
    harness: ai::Harness,
    program: Option<PathBuf>,
    model: String,
    effort: String,
    instructions: String,
    level: Level,
    doc: Option<PathBuf>,
}

/// The running session.
struct Live {
    session: ai::Session,
    started: Started,
    /// The client key the harness's MCP proxy carries.
    key: String,
    /// The host's grants, which hold the key's level until the session ends.
    grants: Option<crate::mcp::access::Grants>,
}

impl Drop for Live {
    /// A key outlives no session: its level is revoked with it.
    fn drop(&mut self) {
        if let Some(g) = &self.grants {
            g.revoke(&self.key);
        }
    }
}

/// The turn in flight.
struct Running {
    /// The agent's message being written.
    msg: usize,
    /// The person's question it answers.
    question: usize,
    /// When it was sent, in egui time.
    sent: f64,
    /// The harness has sent its first event.
    alive: bool,
    harness: ai::Harness,
    /// Lines that were not JSON, for *Show details*.
    diagnostics: String,
    /// *Stop* was pressed; the turn ends when the harness says it has.
    stopping: bool,
}

/// Finding the program, off the UI thread: a probe runs it.
enum Lookup {
    Pending(
        ai::Harness,
        Option<PathBuf>,
        Receiver<Result<ai::Found, ai::NotFound>>,
    ),
    Done(
        ai::Harness,
        Option<PathBuf>,
        Result<ai::Found, ai::NotFound>,
    ),
}

/// Why the composer cannot send, the first that holds; off wins.
#[derive(Clone, Debug)]
pub(crate) enum Why {
    Off,
    Private,
    Looking(ai::Harness),
    NotFound(ai::Harness, ai::NotFound),
    Unsealed(ai::Harness),
    SignedOut(ai::Harness),
    NoDocument,
}

impl Why {
    /// The sentence the composer shows.
    pub(crate) fn sentence(&self) -> String {
        match self {
            Why::Off => "The assistant is off. Choose an agent in Settings ▸ Assistant.".into(),
            Why::Private => "Not available in private mode".into(),
            Why::Looking(h) => format!("Looking for {}…", h.label()),
            Why::NotFound(h, nf) => {
                let places = match nf.looked_in.is_empty() {
                    true => "its install folders and on PATH".to_owned(),
                    false => nf
                        .looked_in
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                };
                format!(
                    "{} was not found. Ondin looked for {} in {places}.",
                    h.label(),
                    nf.program
                )
            }
            Why::Unsealed(h) => format!(
                "{} could not be started with its own tools off, so Ondin will not start it.",
                h.label()
            ),
            Why::SignedOut(h) => format!(
                "{} isn't signed in. Run {} once in a terminal to sign in, then ask again.",
                h.label(),
                h.program()
            ),
            Why::NoDocument => "Save the document to ask about it.".into(),
        }
    }
}

/// The assistant's state in the window.
#[derive(Default)]
pub(crate) struct Assistant {
    pub(crate) open: bool,
    pub(crate) msgs: Vec<Msg>,
    /// Where `msgs` are kept, and the document they belong to.
    file: Option<PathBuf>,
    doc: Option<(Option<String>, Option<PathBuf>)>,
    /// Attached to the next question.
    pub(crate) chips: Vec<Chip>,
    pub(crate) draft: String,
    /// The earlier question `Ctrl+↑` has stepped to, from the newest.
    pub(crate) recall: Option<usize>,
    /// This chat's level; Settings holds the default.
    pub(crate) level: Option<Level>,
    live: Option<Live>,
    running: Option<Running>,
    lookup: Option<Lookup>,
    /// Settings' own lookup, for the agent and path not yet saved, so its
    /// hint answers the moment either changes without moving the panel's.
    settings_lookup: Option<Lookup>,
    /// The harness said it is not signed in, until a turn succeeds.
    signed_out: Option<ai::Harness>,
    /// A reply arrived while the panel was shut.
    pub(crate) unseen: bool,
    /// Move the keyboard to the composer this frame.
    pub(crate) focus: bool,
    /// What the agent was shown at its last turn.
    look: Option<prompt::Look>,
    /// Proposals applied from their cards, by proposal id.
    pub(crate) applied: std::collections::HashMap<u64, crate::mcp::propose::Applied>,
    /// The layer a chip under the pointer outlines.
    pub(crate) hovered: Option<String>,
    /// The panel's drag edge lights after the layers panel's dwell.
    pub(crate) edge: crate::app::EdgeDwell,
    /// Counts sessions, so each gets its own client key.
    sessions: u64,
    /// Each harness's program, model and effort as last set in Settings this
    /// session, which come back when it is picked again.
    pub(crate) remembered: std::collections::HashMap<String, (String, String, String)>,
}

impl Assistant {
    /// Whether a turn is in flight.
    pub(crate) fn running(&self) -> bool {
        self.running.is_some()
    }

    /// The live turn's message, its send time and whether the harness has
    /// spoken yet.
    pub(crate) fn live_turn(&self) -> Option<(usize, f64, bool, ai::Harness)> {
        self.running
            .as_ref()
            .map(|r| (r.msg, r.sent, r.alive, r.harness))
    }

    /// The seal of the program found, for the head.
    pub(crate) fn found(&self) -> Option<&ai::Found> {
        match &self.lookup {
            Some(Lookup::Done(_, _, Ok(f))) => Some(f),
            _ => None,
        }
    }

    /// The person's questions, newest last.
    pub(crate) fn questions(&self) -> Vec<&str> {
        self.msgs
            .iter()
            .filter_map(|m| match m {
                Msg::Person { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Save the thread, leaving out a turn still streaming.
    fn save(&self) {
        if let Some(file) = &self.file {
            let streaming = self.running.as_ref().map(|r| r.msg);
            thread::save(file, &thread::for_saving(&self.msgs, streaming));
        }
    }

    /// End the session; a running turn ends with `why` as its note.
    fn end_session(&mut self, why: Option<&str>) {
        if let Some(mut live) = self.live.take() {
            live.session.stop();
        }
        if let Some(r) = self.running.take() {
            if let Some(why) = why {
                self.msgs.push(Msg::Note(Note::Said(why.into())));
            }
            let _ = r;
        }
        self.look = None;
    }
}

/// The session's client key: unique per session in this window.
fn client_key(n: u64) -> String {
    format!("assistant-{}-{n}", std::process::id())
}

/// Where sessions make their run folders.
pub(crate) fn run_root() -> PathBuf {
    crate::machine_dir::root_or_temp().join("run")
}

/// At launch, off the UI thread: remove what sessions of a closed Ondin left
/// behind, and, only while the assistant is on, probe its program so the
/// panel need not wait. Off starts nothing.
pub(crate) fn at_launch(prefs: &AssistantPrefs) {
    let warm = prefs.harness().map(|h| (h, prefs.program_path()));
    let _ = std::thread::Builder::new()
        .name("assistant-launch".into())
        .spawn(move || {
            ai::sweep(&run_root());
            if let Some((h, program)) = warm {
                ai::warm(h, program);
            }
        });
}

/// A tool call's argument in the agent's terms: a layer id, a query, a name.
fn arg_of(input: &serde_json::Value) -> String {
    const KEYS: [&str; 8] = [
        "id", "ids", "query", "name", "verb", "title", "document", "tool",
    ];
    for k in KEYS {
        match input.get(k) {
            Some(serde_json::Value::String(s)) => return s.clone(),
            Some(serde_json::Value::Array(a)) if !a.is_empty() => {
                let first = a[0].as_str().unwrap_or_default();
                return match a.len() {
                    1 => first.to_owned(),
                    n => format!("{first} +{}", n - 1),
                };
            }
            _ => {}
        }
    }
    String::new()
}

/// A tool result cut short: *image*, the error's first line, or a line count.
fn result_of(text: &str, is_error: bool, image: bool) -> String {
    if image {
        return "image".into();
    }
    if is_error {
        let line = text.lines().next().unwrap_or("failed").trim();
        return match line.char_indices().nth(60) {
            Some((i, _)) => format!("{}…", &line[..i]),
            None => line.to_owned(),
        };
    }
    let lines = text.lines().count().max(1);
    match lines {
        1 => "1 line".into(),
        n => format!("+{n} lines"),
    }
}

/// The proposal id a `propose_edit` result names.
fn proposal_in(text: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()?
        .get("proposal")?
        .as_u64()
}

impl OndinApp {
    /// The level the chat in front runs at.
    pub(crate) fn assistant_level(&self) -> Level {
        self.assistant
            .level
            .unwrap_or_else(|| self.prefs.assistant.level())
    }

    /// Why the composer cannot send now, or `None`.
    pub(crate) fn assistant_blocked(&mut self) -> Option<Why> {
        let Some(h) = self.prefs.assistant.harness() else {
            return Some(Why::Off);
        };
        if self.prefs.private {
            return Some(Why::Private);
        }
        if self.session.path.is_none() {
            return Some(Why::NoDocument);
        }
        if self.assistant.signed_out == Some(h) {
            return Some(Why::SignedOut(h));
        }
        match self.assistant_lookup(h) {
            None => Some(Why::Looking(h)),
            Some(Err(nf)) => Some(Why::NotFound(h, nf)),
            Some(Ok(f)) if f.seal == ai::Seal::Unknown => Some(Why::Unsealed(h)),
            Some(Ok(_)) => None,
        }
    }

    /// The program for `h` as Settings asks for it, looked up off the UI
    /// thread; `None` while the probe runs.
    fn assistant_lookup(&mut self, h: ai::Harness) -> Option<Result<ai::Found, ai::NotFound>> {
        let over = self.prefs.assistant.program_path();
        poll_lookup(&mut self.assistant.lookup, &self.egui_ctx, h, over)
    }

    /// The program Settings' unsaved `h` and `over` name, looked up off the
    /// UI thread; `None` while the probe runs.
    pub(crate) fn assistant_settings_lookup(
        &mut self,
        h: ai::Harness,
        over: Option<PathBuf>,
    ) -> Option<Result<ai::Found, ai::NotFound>> {
        poll_lookup(&mut self.assistant.settings_lookup, &self.egui_ctx, h, over)
    }
}

/// The lookup in `slot` for `h` and `over`, started afresh when either
/// differs from the one it holds; `None` while its probe runs.
fn poll_lookup(
    slot: &mut Option<Lookup>,
    ctx: &egui::Context,
    h: ai::Harness,
    over: Option<PathBuf>,
) -> Option<Result<ai::Found, ai::NotFound>> {
    let stale = match &*slot {
        Some(Lookup::Pending(lh, lo, _)) | Some(Lookup::Done(lh, lo, _)) => *lh != h || *lo != over,
        None => true,
    };
    if stale {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let o = over.clone();
        let spawned = std::thread::Builder::new()
            .name("assistant-find".into())
            .spawn(move || {
                let _ = tx.send(ai::find(h, o.as_deref()));
                ctx.request_repaint();
            });
        if spawned.is_err() {
            return Some(ai::find(h, over.as_deref()));
        }
        *slot = Some(Lookup::Pending(h, over, rx));
    }
    if let Some(Lookup::Pending(lh, lo, rx)) = &*slot {
        match rx.try_recv() {
            Ok(found) => {
                *slot = Some(Lookup::Done(*lh, lo.clone(), found));
            }
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                *slot = None;
                return None;
            }
        }
    }
    match &*slot {
        Some(Lookup::Done(_, _, r)) => Some(r.clone()),
        _ => None,
    }
}

impl OndinApp {
    /// Open or close the panel. Opening closes its siblings, which share its
    /// place; closing keeps the thread, the draft and a running turn.
    pub(crate) fn toggle_assistant(&mut self) {
        if self.assistant.open {
            self.assistant.open = false;
        } else {
            self.open_assistant();
        }
    }

    /// Open the panel, attaching nothing.
    pub(crate) fn open_assistant(&mut self) {
        if self.prefs.private {
            self.session
                .info("The assistant is not available in private mode");
            return;
        }
        self.kit_picker = None;
        self.var_browser = None;
        self.asset_panel = None;
        self.assistant.open = true;
        self.assistant.unseen = false;
        self.assistant.focus = true;
    }

    /// Attach the selection as chips, open the panel and focus the composer:
    /// the context menu's *Ask the assistant* and the ask button.
    pub(crate) fn ask_assistant(&mut self) {
        let chips = self.selection_chips();
        for c in chips {
            if !self.assistant.chips.iter().any(|k| k.id == c.id) {
                self.assistant.chips.push(c);
            }
        }
        self.open_assistant();
    }

    /// The selection as chips.
    pub(crate) fn selection_chips(&self) -> Vec<Chip> {
        let doc = &self.session.doc;
        self.session
            .selection
            .ids()
            .iter()
            .filter_map(|id| {
                let n = doc.get(*id)?;
                let r = self.session.resolved.world_bounds(*id);
                Some(Chip {
                    id: id.to_wire(),
                    name: n.name().to_owned(),
                    kind: ondin_export::snapshot::kind_name(n.kind()).to_owned(),
                    w: r.map_or(0.0, |r| r.width()),
                    h: r.map_or(0.0, |r| r.height()),
                })
            })
            .collect()
    }

    /// Whether every selected layer is attached already: the selection's ask
    /// button gives way to its layers' lit ones.
    pub(crate) fn selection_attached(&self) -> bool {
        let ids = self.session.selection.ids();
        !ids.is_empty()
            && ids
                .iter()
                .all(|id| self.assistant.chips.iter().any(|c| c.id == id.to_wire()))
    }

    /// Whether a chip's layer is gone from the document.
    pub(crate) fn chip_gone(&self, chip: &Chip) -> bool {
        NodeId::from_wire(&chip.id).is_none_or(|id| self.session.doc.get(id).is_none())
    }

    /// Select a layer by its wire id and zoom to it, on its own page.
    pub(crate) fn assistant_select(&mut self, wire: &str) {
        let Some(id) = NodeId::from_wire(wire).filter(|id| self.session.doc.get(*id).is_some())
        else {
            return;
        };
        if let Some(page) = self.session.doc.page_of(id) {
            self.go_to_page(page);
        }
        self.session.selection.set_one(id);
        self.zoom_to(self.selection_bounds());
    }

    /// A layer's name by wire id, for chips in replies.
    pub(crate) fn layer_name(&self, wire: &str) -> Option<String> {
        let id = NodeId::from_wire(wire)?;
        self.session.doc.get(id).map(|n| n.name().to_owned())
    }

    /// The frame's assistant work, whether or not the panel shows: follow the
    /// document in front, turn private mode into off, drain the session's
    /// events.
    pub(crate) fn assistant_pass(&mut self, ctx: &egui::Context) {
        self.assistant_follow_document();
        if self.prefs.private {
            if self.assistant.open {
                self.assistant.open = false;
            }
            if self.assistant.live.is_some() {
                self.assistant
                    .end_session(Some("Stopped: private mode turned the assistant off."));
                self.assistant.save();
            }
            return;
        }
        if self.prefs.assistant.harness().is_none() && self.assistant.live.is_some() {
            self.assistant
                .end_session(Some("Stopped: the assistant was turned off."));
            self.assistant.save();
        }
        let events = match self.assistant.live.as_mut() {
            Some(live) => live.session.poll(),
            None => return,
        };
        if events.is_empty() {
            if self.assistant.running.is_some() {
                // The seconds count.
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
            return;
        }
        for e in events {
            self.assistant_event(e);
        }
        ctx.request_repaint();
    }

    /// Load the chat of the document in front when it changes; a running turn
    /// on the one left stops, with a note.
    fn assistant_follow_document(&mut self) {
        let now = (
            self.session.doc.meta().id.clone(),
            self.session.path.clone(),
        );
        if self.assistant.doc.as_ref() == Some(&now) {
            return;
        }
        // The same document under a new name or a newly minted id keeps its
        // thread and its session, and moves its file.
        let same = self.assistant.doc.as_ref().is_some_and(|(id, path)| {
            (id.is_some() && *id == now.0) || (path.is_some() && *path == now.1)
        });
        if same {
            let file = thread::file_for(now.0.as_deref(), now.1.as_deref());
            if file != self.assistant.file {
                if let Some(old) = &self.assistant.file {
                    let _ = std::fs::remove_file(old);
                }
                self.assistant.file = file;
                self.assistant.save();
            }
            self.assistant.doc = Some(now);
            return;
        }
        if self.assistant.doc.is_some() {
            self.assistant
                .end_session(Some("Stopped: the document in front changed."));
            self.assistant.save();
        }
        let file = thread::file_for(now.0.as_deref(), now.1.as_deref());
        self.assistant.msgs = file.as_deref().map(thread::load).unwrap_or_default();
        self.assistant.file = file;
        self.assistant.doc = Some(now);
        self.assistant.chips.clear();
        self.assistant.applied.clear();
        self.assistant.level = None;
        self.assistant.look = None;
    }

    /// One event from the session, into the live turn.
    fn assistant_event(&mut self, e: ai::Event) {
        if matches!(e, ai::Event::Stopped) {
            self.assistant_stopped();
            return;
        }
        if let ai::Event::Refused(why) = e {
            let Some(r) = self.assistant.running.take() else {
                return;
            };
            self.assistant.msgs.push(Msg::Note(Note::Failed {
                text: why.to_string(),
                question: r.question,
                details: r.diagnostics,
            }));
            self.assistant.save();
            return;
        }
        let a = &mut self.assistant;
        let Some(r) = a.running.as_mut() else {
            return;
        };
        let msg = r.msg;
        let parts = match a.msgs.get_mut(msg) {
            Some(Msg::Agent { parts, .. }) => parts,
            _ => return,
        };
        match e {
            ai::Event::Alive => r.alive = true,
            ai::Event::Text(t) => {
                r.alive = true;
                match parts.last_mut() {
                    Some(Part::Text(s)) => s.push_str(&t),
                    _ => parts.push(Part::Text(t)),
                }
            }
            ai::Event::ToolCall { id, name, input } => {
                r.alive = true;
                parts.push(Part::Tool(ToolRow {
                    id,
                    arg: arg_of(&input),
                    name,
                    state: ToolState::Running,
                    result: String::new(),
                    proposal: None,
                }));
            }
            ai::Event::ToolResult {
                id,
                text,
                is_error,
                image,
            } => {
                let row = parts.iter_mut().rev().find_map(|p| match p {
                    Part::Tool(t) if t.id == id => Some(t),
                    _ => None,
                });
                if let Some(t) = row {
                    t.state = match is_error {
                        true => ToolState::Failed,
                        false => ToolState::Done,
                    };
                    t.result = result_of(&text, is_error, image);
                    if t.name == "propose_edit" && !is_error {
                        t.proposal = proposal_in(&text);
                    }
                }
            }
            // Taken before the borrow above.
            ai::Event::Stopped | ai::Event::Refused(_) => {}
            ai::Event::Diagnostic(line) => {
                r.diagnostics.push_str(&line);
                r.diagnostics.push('\n');
            }
            ai::Event::Done {
                seconds,
                input_tokens,
                output_tokens,
                ..
            } => {
                let tokens = match (input_tokens, output_tokens) {
                    (None, None) => None,
                    (i, o) => Some(i.unwrap_or(0) + o.unwrap_or(0)),
                };
                if let Some(Msg::Agent { stats, .. }) = a.msgs.get_mut(msg) {
                    *stats = Some(Stats { seconds, tokens });
                }
                a.running = None;
                a.signed_out = None;
                if !a.open {
                    a.unseen = true;
                }
                a.save();
            }
            ai::Event::Exited {
                code,
                signed_out,
                diagnostics,
            } => {
                let r = a.running.take().expect("checked above");
                let label = r.harness.label();
                let text = match (signed_out, code) {
                    (true, _) => {
                        a.signed_out = Some(r.harness);
                        format!("{label} isn't signed in.")
                    }
                    (false, Some(c)) => {
                        format!("{label} stopped before answering (exit code {c}).")
                    }
                    (false, None) => format!("{label} stopped before answering."),
                };
                let mut details = r.diagnostics;
                details.push_str(&diagnostics);
                a.msgs.push(Msg::Note(Note::Failed {
                    text,
                    question: r.question,
                    details,
                }));
                // A child that died takes its thread with it; the next question
                // starts a session and replays the prose.
                a.live = None;
                a.look = None;
                if !a.open {
                    a.unseen = true;
                }
                a.save();
            }
        }
    }

    /// Send a question with the chips attached; `text` is the person's words.
    /// Answers whether the question was taken: `false` leaves the draft for the
    /// person, as while the program is still being looked for.
    pub(crate) fn assistant_send(&mut self, text: String, chips: Vec<Chip>) -> bool {
        let text = text.trim().to_owned();
        if text.is_empty() || self.assistant.running.is_some() {
            return false;
        }
        if self.assistant_blocked().is_some() {
            return false;
        }
        let Some(h) = self.prefs.assistant.harness() else {
            return false;
        };
        let chips: Vec<Chip> = chips.into_iter().filter(|c| !self.chip_gone(c)).collect();
        let earlier: Vec<String> = {
            let q = self.assistant.questions();
            q.iter()
                .rev()
                .take(prompt::EARLIER)
                .rev()
                .map(|s| (*s).to_owned())
                .collect()
        };
        self.assistant.msgs.push(Msg::Person {
            text: text.clone(),
            chips: chips.clone(),
        });
        let question = self.assistant.msgs.len() - 1;
        self.assistant.msgs.push(Msg::Agent {
            speaker: h.speaker().into(),
            harness: h.key().into(),
            parts: Vec::new(),
            stats: None,
        });
        let msg = self.assistant.msgs.len() - 1;
        self.assistant.recall = None;
        let now = self.egui_ctx.input(|i| i.time);
        if let Err(why) = self.assistant_ensure_session(h) {
            self.assistant.msgs.push(Msg::Note(Note::Failed {
                text: why,
                question,
                details: String::new(),
            }));
            self.assistant.save();
            return true;
        }
        // The preamble: what moved since the last turn, the page, the selection.
        let snap = ondin_export::snapshot(&self.session.doc, &self.session.resolved);
        let rev = self.session.revision();
        let page_id = self.session.page.to_wire();
        let page = self
            .session
            .doc
            .get(self.session.page)
            .map(|n| n.name().to_owned())
            .unwrap_or_default();
        let selection: Vec<String> = self
            .session
            .selection
            .ids()
            .iter()
            .map(|id| id.to_wire())
            .collect();
        let earlier_refs: Vec<&str> = earlier.iter().map(String::as_str).collect();
        let pre = prompt::preamble(&prompt::Turn {
            rev,
            snap: &snap,
            since: self.assistant.look.as_ref(),
            page: &page,
            selection: &selection,
            attached: &chips,
            earlier: &earlier_refs,
            page_id: &page_id,
        });
        self.assistant.look = Some(prompt::Look::of(rev, &snap));
        let images = match h.takes_images() {
            true => self.chip_renders(&chips),
            false => Vec::new(),
        };
        let turn = ai::Turn {
            text: format!("{pre}{text}"),
            images,
        };
        let sent = match self.assistant.live.as_mut() {
            Some(live) => live.session.send(turn),
            None => return true,
        };
        match sent {
            Ok(()) => {
                self.assistant.running = Some(Running {
                    msg,
                    question,
                    sent: now,
                    alive: false,
                    harness: h,
                    diagnostics: String::new(),
                    stopping: false,
                });
            }
            Err(refusal) => {
                self.assistant.msgs.push(Msg::Note(Note::Failed {
                    text: refusal.to_string(),
                    question,
                    details: String::new(),
                }));
            }
        }
        self.assistant.save();
        true
    }

    /// Each attached layer at 1×, for a harness whose input takes images.
    fn chip_renders(&self, chips: &[Chip]) -> Vec<Vec<u8>> {
        chips
            .iter()
            .filter_map(|c| {
                let id = NodeId::from_wire(&c.id)?;
                let opts = ondin_export::png::RasterOpts {
                    scale: 1.0,
                    ..Default::default()
                };
                let r = ondin_export::png::raster_of(
                    &self.session.doc,
                    &self.session.resolved,
                    &[id],
                    &opts,
                )?;
                Some(ondin_export::png::encode_rgba8(&r.rgba, r.width, r.height))
            })
            .collect()
    }

    /// What a session for `h` would be started with now.
    fn assistant_started(&self, h: ai::Harness) -> Started {
        let p = &self.prefs.assistant;
        Started {
            harness: h,
            program: p.program_path(),
            model: p.model.trim().to_owned(),
            effort: p.effort.clone(),
            instructions: p.instructions.clone(),
            level: self.assistant_level(),
            doc: self.session.path.clone(),
        }
    }

    /// Start a session unless the live one was started with what Settings and
    /// the head ask for now.
    fn assistant_ensure_session(&mut self, h: ai::Harness) -> Result<(), String> {
        let want = self.assistant_started(h);
        if self
            .assistant
            .live
            .as_ref()
            .is_some_and(|l| l.started == want)
        {
            return Ok(());
        }
        self.assistant.end_session(None);
        let found = match self.assistant_lookup(h) {
            Some(Ok(f)) => f,
            Some(Err(nf)) => return Err(Why::NotFound(h, nf).sentence()),
            None => return Err(Why::Looking(h).sentence()),
        };
        // The thread so far, minus the question just asked and its empty answer.
        let earlier = &self.assistant.msgs[..self.assistant.msgs.len().saturating_sub(2)];
        let replay = match prompt::replay(earlier, h.key()) {
            Some(r) => r,
            None => {
                self.assistant.msgs.insert(
                    self.assistant.msgs.len() - 2,
                    Msg::Note(Note::Said(
                        "The earlier thread was written by another agent and is not replayed."
                            .into(),
                    )),
                );
                String::new()
            }
        };
        self.assistant.sessions += 1;
        let key = client_key(self.assistant.sessions);
        let Ok(exe) = std::env::current_exe() else {
            return Err("Ondin could not find its own program to serve the agent.".into());
        };
        let mut mcp_args = vec!["mcp-proxy".to_owned(), "--client".to_owned(), key.clone()];
        if let Some(doc) = &want.doc {
            mcp_args.push("--document".into());
            mcp_args.push(doc.display().to_string());
        }
        let config = ai::Config {
            harness: h,
            program: found.path.clone(),
            model: want.model.clone(),
            effort: want.effort.clone(),
            allowed_tools: self.assistant_tools(want.level),
            system_prompt: prompt::system(want.level, &want.instructions, &replay),
            mcp_exe: exe,
            mcp_args,
            run_root: run_root(),
        };
        let ctx = self.egui_ctx.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        // Granted by key before the child can connect, then by process too.
        self.assistant_grant(&key, want.level, Arc::new(|_| false));
        let session = match ai::Session::start(config, wake) {
            Ok(s) => s,
            Err(r) => {
                if let Some(host) = &self.mcp {
                    host.grants().revoke(&key);
                }
                return Err(r.to_string());
            }
        };
        self.assistant_grant(&key, want.level, session.owner());
        self.assistant.live = Some(Live {
            session,
            started: want,
            key,
            grants: self.mcp.as_ref().map(|h| h.grants()),
        });
        self.assistant.look = None;
        Ok(())
    }

    /// The bare MCP tool names a level lists to the harness.
    fn assistant_tools(&self, level: Level) -> Vec<String> {
        crate::mcp::access::listed(level)
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    /// Tell the MCP host which level a chat's key and its processes hold.
    fn assistant_grant(&self, key: &str, level: Level, owner: crate::mcp::access::Owns) {
        if let Some(host) = &self.mcp {
            host.grants().grant(key, level, owner);
        }
    }

    /// Stop the running turn; it ends when the harness says it has, and the
    /// next question resumes the session.
    pub(crate) fn assistant_stop(&mut self) {
        let Some(r) = self.assistant.running.as_mut() else {
            return;
        };
        r.stopping = true;
        match self.assistant.live.as_mut() {
            Some(live) => live.session.stop(),
            None => self.assistant_stopped(),
        }
    }

    /// The stopped turn has ended.
    fn assistant_stopped(&mut self) {
        if self.assistant.running.take().is_some() {
            self.assistant.msgs.push(Msg::Note(Note::Said(
                "Stopped. The next question resumes the session.".into(),
            )));
            self.assistant.save();
        }
    }

    /// Whether *Stop* was pressed and the harness has not yet ended the turn.
    pub(crate) fn assistant_stopping(&self) -> bool {
        self.assistant.running.as_ref().is_some_and(|r| r.stopping)
    }

    /// Start the thread over.
    pub(crate) fn assistant_new_chat(&mut self) {
        self.assistant.end_session(None);
        self.assistant.msgs.clear();
        self.assistant.applied.clear();
        self.assistant.signed_out = None;
        self.assistant.save();
        self.assistant.focus = true;
    }

    /// Switch the chat's level; the session restarts on the next question with
    /// the prose replayed.
    pub(crate) fn assistant_set_level(&mut self, level: Level) {
        if self.assistant_level() == level {
            return;
        }
        self.assistant.level = Some(level);
        if self.assistant.live.is_some() {
            self.assistant
                .end_session(Some("Stopped: the access level changed."));
            self.assistant.save();
        }
    }

    /// Ask the question a failure note answered again, with its chips.
    pub(crate) fn assistant_ask_again(&mut self, question: usize) {
        if let Some(Msg::Person { text, chips }) = self.assistant.msgs.get(question).cloned() {
            self.assistant_send(text, chips);
        }
    }

    /// Drop the last turn and ask its question again.
    pub(crate) fn assistant_regenerate(&mut self) {
        let Some(q) = self
            .assistant
            .msgs
            .iter()
            .rposition(|m| matches!(m, Msg::Person { .. }))
        else {
            return;
        };
        let Some(Msg::Person { text, chips }) = self.assistant.msgs.get(q).cloned() else {
            return;
        };
        self.assistant.msgs.truncate(q);
        self.assistant_send(text, chips);
    }

    /// After Settings saves: a change to what the session was started with
    /// stops it, with a note when a turn was running, and finding the program
    /// starts again.
    pub(crate) fn assistant_settings_saved(&mut self, before: &AssistantPrefs) {
        let now = &self.prefs.assistant;
        if before.agent != now.agent || before.program != now.program {
            self.assistant.lookup = None;
            self.assistant.signed_out = None;
        }
        let changed = before.agent != now.agent
            || before.program != now.program
            || before.model != now.model
            || before.effort != now.effort
            || before.instructions != now.instructions;
        if changed && self.assistant.live.is_some() {
            self.assistant
                .end_session(Some("Stopped: the assistant's settings changed."));
            self.assistant.save();
        }
        if let Some(h) = now.harness() {
            ai::warm(h, now.program_path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A tool's argument is the id, query or name it was given.
    #[test]
    fn a_tool_s_argument_is_what_it_was_given() {
        assert_eq!(arg_of(&json!({ "id": "4:31", "depth": 2 })), "4:31");
        assert_eq!(
            arg_of(&json!({ "ids": ["4:31", "4:32", "4:33"] })),
            "4:31 +2"
        );
        assert_eq!(arg_of(&json!({ "depth": 2 })), "");
    }

    /// A result reads as an image, its error's first line, or a line count.
    #[test]
    fn a_result_is_cut_short() {
        assert_eq!(result_of("", false, true), "image");
        assert_eq!(
            result_of("no layer 4:12\nmore", true, false),
            "no layer 4:12"
        );
        assert_eq!(result_of("a\nb\nc", false, false), "+3 lines");
    }

    /// A proposal's id is read from `propose_edit`'s answer.
    #[test]
    fn a_proposal_id_is_read_from_its_answer() {
        assert_eq!(proposal_in(r#"{"proposal": 7, "changes": []}"#), Some(7));
        assert_eq!(proposal_in("refused"), None);
    }

    /// A window with one rectangle, *Card*, selected.
    fn app_with_card(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let id = crate::palette::tests::add_rect(&mut app);
        app.session.selection.set_one(id);
        (app, id)
    }

    /// *Ask the assistant* attaches the selection once, opens the panel and
    /// closes its siblings; opening a sibling closes it again.
    /// flip: `ask_assistant` dropping its already-attached check → fails at the
    /// chip count.
    #[test]
    fn asking_attaches_the_selection_once_and_shares_the_place() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_card(&ctx);
        app.asset_panel = Some(Default::default());
        app.ask_assistant();
        app.ask_assistant();
        assert_eq!(app.assistant.chips.len(), 1);
        assert_eq!(app.assistant.chips[0].id, id.to_wire());
        assert_eq!(app.assistant.chips[0].name, "Card");
        assert!(app.assistant.open && app.assistant.focus);
        assert!(
            app.asset_panel.is_none(),
            "the assets panel shares its place"
        );
        assert!(app.selection_attached(), "the ask buttons are lit");
        app.toggle_assets();
        assert!(!app.assistant.open, "opening a sibling closes it");
    }

    /// Private mode closes the panel and refuses to open it.
    /// flip: `assistant_pass` leaving the panel open → fails at the first assert.
    #[test]
    fn private_mode_turns_the_assistant_off() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_card(&ctx);
        app.assistant.open = true;
        app.prefs.private = true;
        app.assistant_pass(&ctx);
        assert!(!app.assistant.open);
        app.toggle_assistant();
        assert!(!app.assistant.open, "refused in private mode");
        app.prefs.private = false;
        app.toggle_assistant();
        assert!(app.assistant.open, "the fixture can open it otherwise");
    }

    /// A document whose id is minted after the chat began keeps its thread; a
    /// different document loads its own.
    /// flip: `assistant_follow_document` without its `same` branch → fails at
    /// the kept thread.
    #[test]
    fn a_newly_minted_id_keeps_the_thread() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_card(&ctx);
        // Per run, so a run that panicked leaves nothing this one loads.
        let path = std::env::temp_dir().join(format!(
            "ondin-assistant-mint-{}-{:?}.ondin",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        app.session.path = Some(path.clone());
        app.assistant_pass(&ctx);
        assert!(app.assistant.msgs.is_empty(), "the fixture starts empty");
        app.assistant.msgs.push(Msg::Person {
            text: "q".into(),
            chips: Vec::new(),
        });
        let mut meta = app.session.doc.meta().clone();
        meta.id = Some("minted-id-test".into());
        app.session.doc.set_meta(meta);
        app.assistant_pass(&ctx);
        assert_eq!(app.assistant.msgs.len(), 1, "the thread is kept");
        // Leave nothing behind in the machine folder.
        app.assistant.msgs.clear();
        app.assistant.save();
        app.session.path = Some(path.with_file_name("ondin-assistant-other.ondin"));
        app.session
            .doc
            .set_meta(ondin_core::DocumentMeta::default());
        app.assistant.msgs.push(Msg::Note(Note::Said("x".into())));
        app.assistant_pass(&ctx);
        assert!(
            app.assistant.msgs.is_empty(),
            "another document has its own"
        );
    }

    /// A chip whose layer was deleted reads as gone, and is not sent.
    #[test]
    fn a_deleted_layer_s_chip_is_gone() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_card(&ctx);
        app.ask_assistant();
        let chip = app.assistant.chips[0].clone();
        assert!(!app.chip_gone(&chip));
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::DeleteNode { id }
        ])));
        assert!(app.chip_gone(&chip));
    }

    /// With the assistant off, the composer says so and nothing is sent.
    /// flip: `assistant_blocked` answering `None` while off → fails at `Off`.
    #[test]
    fn off_blocks_the_composer_and_sends_nothing() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_card(&ctx);
        assert!(matches!(app.assistant_blocked(), Some(Why::Off)));
        app.assistant_send("Why does it wrap?".into(), Vec::new());
        assert!(app.assistant.msgs.is_empty());
    }

    /// The panel draws a thread holding every kind of message, and its marks.
    #[test]
    fn the_panel_draws_every_kind_of_message() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_card(&ctx);
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(46.0, 40.0), egui::vec2(1200.0, 800.0));
        app.assistant_button =
            egui::Rect::from_min_size(egui::pos2(8.0, 700.0), egui::vec2(30.0, 30.0));
        app.ask_assistant();
        app.assistant.msgs = vec![
            Msg::Person {
                text: "Why does it wrap?".into(),
                chips: app.assistant.chips.clone(),
            },
            Msg::Agent {
                speaker: "Claude".into(),
                harness: "claude".into(),
                parts: (0..5)
                    .map(|i| {
                        Part::Tool(ToolRow {
                            id: format!("t{i}"),
                            name: "get_node".into(),
                            arg: id.to_wire(),
                            state: ToolState::Done,
                            result: "+24 lines".into(),
                            proposal: None,
                        })
                    })
                    .chain([Part::Text(format!(
                        "It is **Fixed**: set {} to Fill.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```options\n- Fill\n- Hug\n```",
                        id.to_wire()
                    ))])
                    .collect(),
                stats: Some(Stats {
                    seconds: 14.0,
                    tokens: Some(3100),
                }),
            },
            Msg::Note(Note::Failed {
                text: "Claude Code stopped before answering (exit code 1).".into(),
                question: 0,
                details: "boom".into(),
            }),
        ];
        let mut texts = Vec::new();
        for _ in 0..3 {
            let out = ctx.run_ui(egui::RawInput::default(), |ui| app.assistant_ui(ui.ctx()));
            texts = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>();
        }
        let all = texts.join("|");
        assert!(
            texts.iter().any(|t| t == "+2 calls"),
            "the fourth call on folds, its count said once: {all}"
        );
        assert!(all.contains("Ask again"), "{all}");
        assert!(all.contains("14 s · 3.1k tokens"), "{all}");
        assert!(all.contains("Fill"), "the options draw as buttons: {all}");
        assert!(all.contains("The assistant is off"), "{all}");
    }
}
