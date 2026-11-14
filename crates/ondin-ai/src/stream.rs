//! Seven dialects decoded into one vocabulary.
//!
//! The harnesses disagree on where the discriminator lives (`type`, or
//! Antigravity's `event`), on whether text arrives as deltas, cumulative
//! restatements, deltas *then* restatements, or whole parts, and on whether a
//! turn's end is announced at all (OpenCode's is read off `step_finish.reason`).
//! The fixtures in the tests are captured from the real programs.
//!
//! The parser is stateful: it turns restated text into deltas, announces a
//! restated tool call once, and clears that state at every turn's end, because
//! call ids are unique only within a turn (Antigravity numbers steps from zero).
//! A side-effecting tool a harness reports is shown, never dropped: if a seal
//! ever fails to hold, the person watches it happen.

use crate::harness::Harness;
use std::collections::{HashMap, HashSet};

/// One decoded event, before names are made bare.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Parsed {
    Text(String),
    /// `name` is qualified (`mcp__<server>__<tool>`) or a built-in's own.
    ToolUse {
        id: Option<String>,
        name: String,
        input: serde_json::Value,
    },
    /// Paired with its call by `id`, never by recency, which mis-pairs
    /// parallel calls.
    ToolResult {
        id: Option<String>,
        text: String,
        is_error: bool,
        image: bool,
    },
    Done {
        is_error: bool,
        duration_ms: Option<u64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    /// The conversation id the next turn resumes with.
    Session(String),
}

/// What one line of stdout turned out to be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LineKind {
    #[default]
    Blank,
    Json,
    /// Not JSON: prose the CLI printed, often the only explanation of a failure.
    Plain,
}

/// Emits only the part of a restated string not yet sent.
#[derive(Default)]
struct Coalescer {
    sent: HashMap<String, String>,
}

impl Coalescer {
    /// A cumulative restatement's unseen tail; a text that is not an extension
    /// of what was sent (a rewrite) is sent whole.
    fn advance(&mut self, key: &str, full: &str) -> Option<String> {
        match self.sent.get_mut(key) {
            Some(p) if full.starts_with(p.as_str()) => {
                if full.len() == p.len() {
                    return None;
                }
                let out = full[p.len()..].to_string();
                p.push_str(&out);
                Some(out)
            }
            Some(p) => {
                p.clear();
                p.push_str(full);
                (!full.is_empty()).then(|| full.to_string())
            }
            None => {
                self.sent.insert(key.to_string(), full.to_string());
                (!full.is_empty()).then(|| full.to_string())
            }
        }
    }

    /// Record a true delta as sent.
    fn extend(&mut self, key: &str, delta: &str) -> Option<String> {
        if delta.is_empty() {
            return None;
        }
        self.sent
            .entry(key.to_string())
            .or_default()
            .push_str(delta);
        Some(delta.to_string())
    }

    /// Reconcile a restatement with the deltas already sent. For the dialects
    /// that stream and then restate (Copilot, Cursor), a restatement that is not
    /// an extension is a trimmed copy of text on screen and adds nothing.
    fn settle(&mut self, key: &str, full: &str) -> Option<String> {
        match self.sent.get(key) {
            Some(p) if !full.starts_with(p.as_str()) => None,
            _ => self.advance(key, full),
        }
    }

    fn clear(&mut self, key: &str) {
        self.sent.remove(key);
    }
}

/// OpenCode reports usage per step and never says a turn is over, so the
/// totals and the wall clock (from event timestamps) are assembled here.
#[derive(Default)]
struct OpenCodeTurn {
    session_announced: bool,
    first_ts: Option<u64>,
    last_ts: Option<u64>,
    input: u64,
    output: u64,
    saw_tokens: bool,
}

/// A DeepSeek turn in flight: usage summed over its steps, whether the last
/// thing printed was prose, why it failed, and a step's prose cut by `--json`'s
/// 8 KiB cap, which `final` (uncapped) then finishes.
#[derive(Default)]
struct DshTurn {
    input: Option<u64>,
    output: Option<u64>,
    spoke: bool,
    failure: Option<String>,
    cut: Option<String>,
}

impl DshTurn {
    /// `text` as its own paragraph when the turn has already spoken.
    fn say(&mut self, text: &str) -> Parsed {
        let out = match self.spoke {
            true => format!("\n\n{}", text.trim_start()),
            false => text.trim_start().to_string(),
        };
        self.spoke = true;
        Parsed::Text(out)
    }
}

/// Decodes one harness's stream. One per turn for a per-turn harness, one per
/// process for a persistent one; never shared between two streams.
pub(crate) struct StreamParser {
    harness: Harness,
    text: Coalescer,
    /// Calls already announced, and results already given (`id` + NUL +
    /// `done`), by the dialect's own id: a restatement is not a second call.
    seen: HashSet<String>,
    oc: OpenCodeTurn,
    dsh: DshTurn,
    /// Claude only: the turn has streamed text, so an error's reason starts a
    /// paragraph of its own.
    claude_spoke: bool,
    /// Codex only: the last `error` notice, the reason of a `turn.failed`
    /// that gives none.
    codex_error: Option<String>,
    /// The turn has said something, so a failure at its end is not silent.
    turn_said: bool,
    last_line: LineKind,
}

impl StreamParser {
    pub(crate) fn new(harness: Harness) -> Self {
        Self {
            harness,
            text: Coalescer::default(),
            seen: HashSet::new(),
            oc: OpenCodeTurn::default(),
            dsh: DshTurn::default(),
            claude_spoke: false,
            codex_error: None,
            turn_said: false,
            last_line: LineKind::Blank,
        }
    }

    /// What the line last pushed was.
    pub(crate) fn last_line(&self) -> LineKind {
        self.last_line
    }

    /// First time this key is seen. An empty id is always announced: silently
    /// dropping a call is the worse failure.
    fn first_sight(&mut self, id: &str) -> bool {
        if id.is_empty() || id == "\u{0}done" {
            return true;
        }
        self.seen.insert(id.to_string())
    }

    /// One call's announce-once, resolve-once pair. A completion with no start
    /// still opens its own chip rather than landing on another call's.
    fn call(
        &mut self,
        id: &str,
        name: impl FnOnce() -> String,
        input: impl FnOnce() -> serde_json::Value,
        result: Option<(String, bool, bool)>,
    ) -> Vec<Parsed> {
        let mut out = Vec::new();
        if self.first_sight(id) {
            out.push(Parsed::ToolUse {
                id: Some(id.to_string()),
                name: name(),
                input: input(),
            });
        }
        if let Some((text, is_error, image)) = result
            && self.first_sight(&format!("{id}\u{0}done"))
        {
            out.push(Parsed::ToolResult {
                id: Some(id.to_string()),
                text,
                is_error,
                image,
            });
        }
        out
    }

    /// Decode one line of stdout.
    pub(crate) fn push(&mut self, line: &str) -> Vec<Parsed> {
        // A byte-order mark survives `trim` and would file the opening line,
        // which carries the session id, as prose.
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            self.last_line = LineKind::Blank;
            return Vec::new();
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            self.last_line = LineKind::Plain;
            return Vec::new();
        };
        self.last_line = LineKind::Json;
        let mut out = match self.harness {
            Harness::Claude => self.push_claude(&v),
            Harness::Codex => self.push_codex(&v),
            Harness::Antigravity => self.push_antigravity(&v),
            Harness::OpenCode => self.push_opencode(&v),
            Harness::Copilot => self.push_copilot(&v),
            Harness::Cursor => self.push_cursor(&v),
            Harness::DeepSeek => self.push_deepseek(&v),
        };
        self.turn_said |= out
            .iter()
            .any(|e| matches!(e, Parsed::Text(t) if !t.trim().is_empty()));
        // A failure with no reason in any dialect still says it failed:
        // otherwise it reads as an empty, successful answer.
        if let Some(at) = out
            .iter()
            .position(|e| matches!(e, Parsed::Done { is_error: true, .. }))
            && !self.turn_said
        {
            out.insert(
                at,
                Parsed::Text(format!(
                    "{} reported that this turn failed, without saying why.",
                    self.harness.label()
                )),
            );
        }
        if out.iter().any(|e| matches!(e, Parsed::Done { .. })) {
            self.seen.clear();
            self.text = Coalescer::default();
            self.oc = OpenCodeTurn::default();
            self.claude_spoke = false;
            self.codex_error = None;
            self.turn_said = false;
        }
        out
    }

    /// Claude Code's `stream-json`: text from the partial-message deltas, calls
    /// from the whole `assistant` message, results from the synthetic `user`
    /// message. A failure before the model was called arrives only as an error
    /// `result`, so its reason is said ahead of the `Done`.
    fn push_claude(&mut self, v: &serde_json::Value) -> Vec<Parsed> {
        let mut out = Vec::new();
        match v.get("type").and_then(|t| t.as_str()) {
            Some("system") if v.get("subtype").and_then(|s| s.as_str()) == Some("init") => {
                if let Some(id) = v.get("session_id").and_then(|s| s.as_str()) {
                    out.push(Parsed::Session(id.to_string()));
                }
            }
            Some("stream_event") => {
                if v.pointer("/event/type").and_then(|t| t.as_str()) == Some("content_block_delta")
                    && v.pointer("/event/delta/type").and_then(|t| t.as_str()) == Some("text_delta")
                    && let Some(t) = v.pointer("/event/delta/text").and_then(|t| t.as_str())
                {
                    self.claude_spoke = true;
                    out.push(Parsed::Text(t.to_string()));
                }
            }
            Some("assistant") => {
                for b in blocks(v.pointer("/message/content")) {
                    if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        let id = str_at(b, "id");
                        out.extend(self.call(
                            id,
                            || str_at(b, "name").to_string(),
                            || b.get("input").cloned().unwrap_or_default(),
                            None,
                        ));
                    }
                }
            }
            Some("user") => {
                for b in blocks(v.pointer("/message/content")) {
                    if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                        let id = str_at(b, "tool_use_id");
                        let (text, image) = content_text(b.get("content"));
                        let is_error = b.get("is_error").and_then(|e| e.as_bool()) == Some(true);
                        out.extend(self.call(
                            id,
                            || "tool".to_string(),
                            || serde_json::Value::Null,
                            Some((text, is_error, image)),
                        ));
                    }
                }
            }
            Some("result") => {
                let is_error = v.get("is_error").and_then(|b| b.as_bool()) == Some(true);
                // `error_during_execution` and its kin carry `errors`, not `result`.
                let errors = blocks(v.get("errors"))
                    .filter_map(|e| e.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if is_error
                    && let Some(why) = v
                        .get("result")
                        .and_then(|r| r.as_str())
                        .map(str::trim)
                        .filter(|r| !r.is_empty())
                        .or(Some(errors.trim()).filter(|e| !e.is_empty()))
                {
                    out.push(Parsed::Text(match self.claude_spoke {
                        true => format!("\n\n{why}"),
                        false => why.to_string(),
                    }));
                }
                let u = |p: &str| v.pointer(p).and_then(|n| n.as_u64());
                // `input_tokens` leaves out cached input, which on a resumed
                // turn is nearly all of it; the other dialects count it.
                let input = [
                    "input_tokens",
                    "cache_read_input_tokens",
                    "cache_creation_input_tokens",
                ]
                .iter()
                .filter_map(|k| u(&format!("/usage/{k}")))
                .reduce(|a, b| a + b);
                out.push(Parsed::Done {
                    is_error,
                    duration_ms: u("/duration_ms"),
                    input_tokens: input,
                    output_tokens: u("/usage/output_tokens"),
                });
            }
            _ => {}
        }
        out
    }

    /// `codex exec --json`. Messages may be restated cumulatively (coalesced);
    /// measured builds send each whole, once.
    fn push_codex(&mut self, v: &serde_json::Value) -> Vec<Parsed> {
        match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "thread.started" => v
                .get("thread_id")
                .and_then(|t| t.as_str())
                .map(|id| vec![Parsed::Session(id.to_string())])
                .unwrap_or_default(),
            ty @ ("item.started" | "item.updated" | "item.completed") => match v.get("item") {
                Some(item) => self.codex_item(item, ty == "item.completed"),
                None => Vec::new(),
            },
            "turn.completed" => {
                // Cached input nests inside `input_tokens` here, so it is not
                // added: there is no total to reconcile, and adding would invent
                // tokens.
                let u = |k: &str| v.pointer(&format!("/usage/{k}")).and_then(|n| n.as_u64());
                vec![Parsed::Done {
                    is_error: false,
                    duration_ms: None,
                    input_tokens: u("input_tokens"),
                    output_tokens: u("output_tokens"),
                }]
            }
            "turn.failed" => {
                let said = v.pointer("/error/message").and_then(|m| m.as_str());
                let why = said
                    .filter(|m| !m.is_empty())
                    .map(str::to_string)
                    .or(self.codex_error.take());
                failed(why.as_deref())
            }
            // Not the turn's end: Codex says it while it reconnects, and a
            // fatal one is followed by `turn.failed` or by its exit. Kept for
            // "Show details" and for a `turn.failed` that names nothing.
            "error" => {
                self.codex_error = Some(str_at(v, "message").to_string());
                self.last_line = LineKind::Plain;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn codex_item(&mut self, item: &serde_json::Value, completed: bool) -> Vec<Parsed> {
        let id = str_at(item, "id");
        match str_at(item, "type") {
            "agent_message" => {
                let out = self
                    .text
                    .advance(id, str_at(item, "text"))
                    .map(|t| vec![Parsed::Text(t)])
                    .unwrap_or_default();
                if completed {
                    self.text.clear(id);
                }
                out
            }
            "mcp_tool_call" => {
                let name = format!("mcp__{}__{}", str_at(item, "server"), str_at(item, "tool"));
                let result = completed.then(|| {
                    let err = item.pointer("/error/message").and_then(|m| m.as_str());
                    let is_error = err.is_some() || str_at(item, "status") == "failed";
                    let (text, image) = content_text(item.pointer("/result/content"));
                    (err.map_or(text, str::to_string), is_error, image)
                });
                self.call(
                    id,
                    || name,
                    || item.get("arguments").cloned().unwrap_or_default(),
                    result,
                )
            }
            // Side effects: they should not happen under the sandbox, and are
            // shown if they do.
            "command_execution" => {
                let code = item.get("exit_code").and_then(|c| c.as_i64());
                let result = completed.then(|| {
                    (
                        format!(
                            "{}\n{}",
                            str_at(item, "command"),
                            str_at(item, "aggregated_output")
                        ),
                        str_at(item, "status") == "failed" || code.is_some_and(|c| c != 0),
                        false,
                    )
                });
                self.call(
                    id,
                    || "shell".to_string(),
                    || serde_json::json!({ "command": str_at(item, "command") }),
                    result,
                )
            }
            "file_change" => {
                let paths: Vec<&str> = blocks(item.get("changes"))
                    .filter_map(|c| c.get("path").and_then(|p| p.as_str()))
                    .collect();
                let result = completed
                    .then(|| (paths.join("\n"), str_at(item, "status") == "failed", false));
                self.call(
                    id,
                    || "file_change".to_string(),
                    || serde_json::json!({ "paths": paths }),
                    result,
                )
            }
            "error" => vec![Parsed::Text(format!("\n{}\n", str_at(item, "message")))],
            // `reasoning`: thinking is not shown.
            _ => Vec::new(),
        }
    }

    /// Antigravity's stream: discriminated by `event`, its payload nested under
    /// a key of the same name. An MCP call arrives as the built-in
    /// `call_mcp_tool` with the real name in its parameters.
    fn push_antigravity(&mut self, v: &serde_json::Value) -> Vec<Parsed> {
        match v.get("event").and_then(|t| t.as_str()).unwrap_or("") {
            "init" => v
                .get("conversation_id")
                .and_then(|s| s.as_str())
                .map(|id| vec![Parsed::Session(id.to_string())])
                .unwrap_or_default(),
            "step_update" => {
                let s = v.get("step_update").unwrap_or(&serde_json::Value::Null);
                match str_at(s, "step_type") {
                    // True deltas, the terminal update included.
                    "agent_response" => s
                        .get("text_delta")
                        .and_then(|t| t.as_str())
                        .filter(|t| !t.is_empty())
                        .map(|t| vec![Parsed::Text(t.to_string())])
                        .unwrap_or_default(),
                    "tool" => self.agy_tool(s),
                    // `user_input` is the prompt echoed back.
                    _ => Vec::new(),
                }
            }
            "result" => {
                let r = v.get("result").unwrap_or(&serde_json::Value::Null);
                let mut out = Vec::new();
                // A refused tool leaves `status: SUCCESS` and an empty answer;
                // only `denied_actions` says so.
                if blocks(r.get("denied_actions")).next().is_some() {
                    out.push(Parsed::Text(
                        "\nAntigravity refused a tool this turn needed: it cannot ask for \
                         permission when it is not running interactively.\n"
                            .to_string(),
                    ));
                }
                let is_error = str_at(r, "status") != "SUCCESS";
                let why = r
                    .get("error")
                    .and_then(|e| e.as_str().or_else(|| e.get("message")?.as_str()))
                    .filter(|w| !w.trim().is_empty());
                if is_error && let Some(why) = why {
                    out.push(Parsed::Text(format!("\n{}\n", why.trim())));
                }
                let u = |k: &str| r.pointer(&format!("/usage/{k}")).and_then(|n| n.as_u64());
                out.push(Parsed::Done {
                    is_error,
                    duration_ms: r
                        .get("duration_seconds")
                        .and_then(|d| d.as_f64())
                        .filter(|d| d.is_finite() && *d >= 0.0)
                        .map(|d| (d * 1000.0).round() as u64),
                    input_tokens: u("input_tokens"),
                    output_tokens: u("output_tokens"),
                });
                out
            }
            _ => Vec::new(),
        }
    }

    fn agy_tool(&mut self, s: &serde_json::Value) -> Vec<Parsed> {
        let info = s.get("tool_info").unwrap_or(&serde_json::Value::Null);
        let params = info.get("parameters").unwrap_or(&serde_json::Value::Null);
        let raw = str_at(s, "tool_name");
        let (name, input) = match raw {
            "call_mcp_tool" => (
                format!(
                    "mcp__{}__{}",
                    str_at(params, "ServerName"),
                    str_at(params, "ToolName")
                ),
                params.get("Arguments").cloned().unwrap_or_default(),
            ),
            _ => (raw.to_string(), params.clone()),
        };
        let id = s
            .get("step_index")
            .map(|i| i.to_string())
            .unwrap_or_default();
        let result = match str_at(s, "state") {
            "ERROR" => Some((
                info.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Antigravity refused this tool")
                    .to_string(),
                true,
                false,
            )),
            "DONE" => Some((str_at(info, "output").to_string(), false, false)),
            _ => None,
        };
        self.call(&id, || name, || input, result)
    }

    /// `opencode run --format json`: whole parts, a call and its result in one
    /// event, and the turn's end inferred from a `step_finish` whose reason is
    /// not `tool-calls`.
    fn push_opencode(&mut self, v: &serde_json::Value) -> Vec<Parsed> {
        let mut out = Vec::new();
        if let Some(id) = v.get("sessionID").and_then(|s| s.as_str())
            && !id.is_empty()
            && !self.oc.session_announced
        {
            self.oc.session_announced = true;
            out.push(Parsed::Session(id.to_string()));
        }
        if let Some(ts) = v.get("timestamp").and_then(|t| t.as_u64()) {
            self.oc.first_ts.get_or_insert(ts);
            self.oc.last_ts = Some(ts);
        }
        let part = v.get("part").unwrap_or(&serde_json::Value::Null);
        match str_at(v, "type") {
            "text" => {
                let t = str_at(part, "text");
                if !t.is_empty() {
                    out.push(Parsed::Text(t.to_string()));
                }
            }
            "tool_use" => {
                let raw = str_at(part, "tool");
                let state = part.get("state").unwrap_or(&serde_json::Value::Null);
                let status = str_at(state, "status");
                let id = match str_at(part, "callID") {
                    "" => raw,
                    c => c,
                }
                .to_string();
                let result = (!matches!(status, "running" | "pending" | "")).then(|| {
                    let text = state
                        .get("error")
                        .and_then(|e| e.as_str())
                        .unwrap_or_else(|| str_at(state, "output"));
                    (text.to_string(), status == "error", false)
                });
                // Ondin's server is flattened into the name as `ondin_<tool>`.
                let name = match raw.strip_prefix(&format!("{}_", crate::MCP_SERVER)) {
                    Some(bare) if !bare.is_empty() => format!("mcp__{}__{bare}", crate::MCP_SERVER),
                    _ => raw.to_string(),
                };
                out.extend(self.call(
                    &id,
                    || name,
                    || state.get("input").cloned().unwrap_or_default(),
                    result,
                ));
            }
            "step_finish" => {
                let at = |k: &str| {
                    part.pointer(&format!("/tokens/{k}"))
                        .and_then(|n| n.as_u64())
                };
                // Input, reasoning and cache reads partition the step's total;
                // cache writes are not part of it.
                for k in ["input", "reasoning", "cache/read"] {
                    if let Some(n) = at(k) {
                        self.oc.input += n;
                        self.oc.saw_tokens = true;
                    }
                }
                if let Some(o) = at("output") {
                    self.oc.output += o;
                    self.oc.saw_tokens = true;
                }
                let reason = part.get("reason").and_then(|r| r.as_str());
                if reason != Some("tool-calls") {
                    match reason {
                        Some("length") => out.push(Parsed::Text(
                            "\n\nThe answer stops here: the model reached its output limit.\n"
                                .into(),
                        )),
                        Some("content-filter") => out.push(Parsed::Text(
                            "\n\nThe rest of the answer was withheld by a content filter.\n".into(),
                        )),
                        _ => {}
                    }
                    out.push(self.opencode_done(reason == Some("error")));
                }
            }
            // `session.error`, its only failure event; nothing follows it.
            "error" => {
                let e = v.get("error").unwrap_or(&serde_json::Value::Null);
                let msg = e
                    .pointer("/data/message")
                    .and_then(|m| m.as_str())
                    .unwrap_or_else(|| str_at(e, "name"));
                if !msg.is_empty() {
                    out.push(Parsed::Text(format!("\n{msg}\n")));
                }
                out.push(self.opencode_done(true));
            }
            _ => {}
        }
        out
    }

    fn opencode_done(&self, is_error: bool) -> Parsed {
        Parsed::Done {
            is_error,
            duration_ms: match (self.oc.first_ts, self.oc.last_ts) {
                (Some(a), Some(b)) if b >= a => Some(b - a),
                _ => None,
            },
            input_tokens: self.oc.saw_tokens.then_some(self.oc.input),
            output_tokens: self.oc.saw_tokens.then_some(self.oc.output),
        }
    }

    /// `copilot --output-format json`. Deltas, then the whole message again;
    /// `assistant.turn_end` closes one model call, not the turn; `result`, with
    /// no `data`, is the end and the only place the session id appears.
    fn push_copilot(&mut self, v: &serde_json::Value) -> Vec<Parsed> {
        let d = v.get("data").unwrap_or(&serde_json::Value::Null);
        let name = || match (str_at(d, "mcpServerName"), str_at(d, "mcpToolName")) {
            (s, t) if !s.is_empty() && !t.is_empty() => format!("mcp__{s}__{t}"),
            _ => match str_at(d, "toolName") {
                "" => "tool".to_string(),
                n => n.to_string(),
            },
        };
        match str_at(v, "type") {
            "assistant.message_delta" => self
                .text
                .extend(str_at(d, "messageId"), str_at(d, "deltaContent"))
                .map(|t| vec![Parsed::Text(t)])
                .unwrap_or_default(),
            "assistant.message" => {
                let id = str_at(d, "messageId");
                let out = self
                    .text
                    .settle(id, str_at(d, "content"))
                    .map(|t| vec![Parsed::Text(t)])
                    .unwrap_or_default();
                self.text.clear(id);
                out
            }
            "tool.execution_start" => {
                let id = str_at(d, "toolCallId").to_string();
                self.call(
                    &id,
                    name,
                    || d.get("arguments").cloned().unwrap_or_default(),
                    None,
                )
            }
            "tool.execution_complete" => {
                let id = str_at(d, "toolCallId").to_string();
                let ok = d.get("success").and_then(|x| x.as_bool()) == Some(true);
                let (flat, image) = content_text(d.pointer("/result/contents"));
                let text = d
                    .pointer("/error/message")
                    .or_else(|| d.pointer("/result/content"))
                    .and_then(|m| m.as_str())
                    .map_or(flat, str::to_string);
                self.call(
                    &id,
                    name,
                    || serde_json::Value::Null,
                    Some((text, !ok, image)),
                )
            }
            "result" => {
                let mut out = Vec::new();
                if let Some(id) = v.get("sessionId").and_then(|x| x.as_str())
                    && !id.is_empty()
                {
                    out.push(Parsed::Session(id.to_string()));
                }
                // Its usage figures are per session, not this turn's: none sent.
                out.push(Parsed::Done {
                    is_error: v.get("exitCode").and_then(|x| x.as_i64()).unwrap_or(0) != 0,
                    duration_ms: None,
                    input_tokens: None,
                    output_tokens: None,
                });
                out
            }
            _ => Vec::new(),
        }
    }

    /// `cursor-agent … --stream-partial-output`. An `assistant` line is a delta
    /// when it has `timestamp_ms` and no `model_call_id`; otherwise it restates
    /// the segment, which a tool call ends.
    fn push_cursor(&mut self, v: &serde_json::Value) -> Vec<Parsed> {
        const SEG: &str = "cursor-segment";
        match str_at(v, "type") {
            "system" if str_at(v, "subtype") == "init" => match str_at(v, "session_id") {
                "" => Vec::new(),
                id => vec![Parsed::Session(id.to_string())],
            },
            "assistant" => {
                let text: String = blocks(v.pointer("/message/content"))
                    .filter(|b| str_at(b, "type") == "text")
                    .map(|b| str_at(b, "text"))
                    .collect();
                let delta = v.get("timestamp_ms").is_some() && v.get("model_call_id").is_none();
                let out = match delta {
                    true => self.text.extend(SEG, &text),
                    false => {
                        let tail = self.text.settle(SEG, &text);
                        self.text.clear(SEG);
                        tail
                    }
                };
                out.map(|t| vec![Parsed::Text(t)]).unwrap_or_default()
            }
            "tool_call" => {
                self.text.clear(SEG);
                self.cursor_tool(v)
            }
            "result" => {
                let at = |k: &str| v.pointer(&format!("/usage/{k}")).and_then(|n| n.as_u64());
                // Cached input is input: a resumed turn's `inputTokens` alone
                // cannot hold the earlier context.
                let input = match (at("inputTokens"), at("cacheReadTokens")) {
                    (None, None) => None,
                    (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
                };
                let is_error = v.get("is_error").and_then(|e| e.as_bool()) == Some(true)
                    || v.get("subtype")
                        .and_then(|s| s.as_str())
                        .is_some_and(|s| s != "success");
                let mut out = Vec::new();
                // On success `result` restates the answer; on failure it is the reason.
                if is_error && !str_at(v, "result").trim().is_empty() {
                    out.push(Parsed::Text(format!("\n{}\n", str_at(v, "result").trim())));
                }
                out.push(Parsed::Done {
                    is_error,
                    duration_ms: v.get("duration_ms").and_then(|d| d.as_u64()),
                    input_tokens: input,
                    output_tokens: at("outputTokens"),
                });
                out
            }
            // `thinking`, and `user` (the prompt echoed).
            _ => Vec::new(),
        }
    }

    /// One `tool_call`: the kind is a key (`mcpToolCall`, `readToolCall`), and
    /// so is the verdict under `result` (`success`, or a refusal). The
    /// catalogue lookup `getMcpToolsToolCall` precedes nearly every call and is
    /// not shown.
    fn cursor_tool(&mut self, v: &serde_json::Value) -> Vec<Parsed> {
        let Some(tc) = v.get("tool_call").and_then(|t| t.as_object()) else {
            return Vec::new();
        };
        let Some((kind, body)) = tc.iter().find(|(k, _)| k.ends_with("ToolCall")) else {
            return Vec::new();
        };
        if kind == "getMcpToolsToolCall" {
            return Vec::new();
        }
        let id = str_at(v, "call_id").to_string();
        let name = || {
            let a = |k: &str| body.pointer(&format!("/args/{k}")).and_then(|x| x.as_str());
            if kind == "mcpToolCall"
                && let (Some(server), Some(tool)) = (
                    a("providerIdentifier").or_else(|| a("serverIdentifier")),
                    a("toolName"),
                )
                && !server.is_empty()
                && !tool.is_empty()
            {
                return format!("mcp__{server}__{tool}");
            }
            kind.strip_suffix("ToolCall").unwrap_or(kind).to_string()
        };
        let input = || match kind.as_str() {
            "mcpToolCall" => body.pointer("/args/args").cloned().unwrap_or_default(),
            _ => body.get("args").cloned().unwrap_or_default(),
        };
        let result =
            (str_at(v, "subtype") == "completed").then(|| cursor_result(body.get("result")));
        self.call(&id, name, input, result)
    }

    /// `dsh --profile headless --json`. `text` is a whole committed step;
    /// `final` restates the answer and is printed only past a cut. A failure
    /// in a turn writes its reason on `status`/`turn_end` and an empty `final`;
    /// one outside a turn writes `error` and no `final`.
    fn push_deepseek(&mut self, v: &serde_json::Value) -> Vec<Parsed> {
        let s = |k: &str| str_at(v, k);
        match s("type") {
            "session" => {
                self.dsh = DshTurn::default();
                match s("sessionId") {
                    "" => Vec::new(),
                    id => vec![Parsed::Session(id.to_string())],
                }
            }
            "text" => {
                let t = s("text");
                let truncated = v.get("truncated").and_then(|x| x.as_bool()) == Some(true);
                if t.trim().is_empty() {
                    self.dsh.cut = truncated.then(String::new);
                    return Vec::new();
                }
                let said = self.dsh.say(t);
                self.dsh.cut = truncated.then(|| t.trim_start().to_string());
                vec![said]
            }
            "status" if s("phase") == "turn_end" => {
                if v.pointer("/reason/kind").and_then(|k| k.as_str()) == Some("error") {
                    self.dsh.failure = dsh_failure_text(v.pointer("/reason/error"));
                }
                Vec::new()
            }
            "status" if s("phase") == "step_end" => {
                let at = |k: &str| v.pointer(&format!("/usage/{k}")).and_then(|n| n.as_u64());
                let add = |a: Option<u64>, b: Option<u64>| match (a, b) {
                    (None, None) => None,
                    (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
                };
                let step_in = add(at("inputTokens"), at("cacheReadTokens"));
                self.dsh.input = add(self.dsh.input, step_in);
                self.dsh.output = add(self.dsh.output, at("outputTokens"));
                Vec::new()
            }
            "tool_call" => {
                self.dsh.spoke = false;
                self.dsh.cut = None;
                let id = s("callId").to_string();
                let name = match s("tool") {
                    "" => "tool",
                    t => t,
                }
                .to_string();
                self.call(
                    &id,
                    || name,
                    || v.get("input").cloned().unwrap_or_default(),
                    None,
                )
            }
            "tool_result" => {
                self.dsh.spoke = false;
                self.dsh.cut = None;
                let id = s("callId").to_string();
                let text = match v.get("result") {
                    Some(serde_json::Value::String(t)) => t.clone(),
                    Some(serde_json::Value::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                };
                let is_error = s("status") != "completed";
                self.call(
                    &id,
                    || "tool".to_string(),
                    || serde_json::Value::Null,
                    Some((text, is_error, false)),
                )
            }
            "final" => {
                let mut out = Vec::new();
                if let Some(id) = v.get("sessionId").and_then(|x| x.as_str())
                    && !id.is_empty()
                {
                    out.push(Parsed::Session(id.to_string()));
                }
                let completed =
                    v.pointer("/reason/kind").and_then(|k| k.as_str()) == Some("completed");
                if !completed && let Some(why) = self.dsh.failure.take() {
                    out.push(self.dsh.say(&why));
                }
                if completed && let Some(printed) = self.dsh.cut.take() {
                    let whole = s("text").trim_start();
                    if printed.is_empty() {
                        if !whole.trim().is_empty() {
                            out.push(self.dsh.say(whole));
                        }
                    } else if let Some(rest) = whole.strip_prefix(printed.as_str())
                        && !rest.is_empty()
                    {
                        out.push(Parsed::Text(rest.to_string()));
                    }
                }
                out.push(Parsed::Done {
                    // An absent reason is a failure: every success said
                    // `completed`.
                    is_error: !completed,
                    duration_ms: None,
                    input_tokens: self.dsh.input,
                    output_tokens: self.dsh.output,
                });
                out
            }
            "error" => {
                let mut out = Vec::new();
                if let Some(why) = dsh_failure_text(Some(v)) {
                    out.push(self.dsh.say(&why));
                }
                out.push(Parsed::Done {
                    is_error: true,
                    duration_ms: None,
                    input_tokens: None,
                    output_tokens: None,
                });
                out
            }
            _ => Vec::new(),
        }
    }
}

/// `v[key]` as a string, `""` when absent.
fn str_at<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

/// The elements of an optional JSON array.
fn blocks(v: Option<&serde_json::Value>) -> impl Iterator<Item = &serde_json::Value> {
    v.and_then(|c| c.as_array()).into_iter().flatten()
}

/// A tool result's text and whether it carried an image: a string, or MCP
/// content blocks (Cursor nests the text one level deeper).
fn content_text(content: Option<&serde_json::Value>) -> (String, bool) {
    match content {
        Some(serde_json::Value::String(s)) => (s.clone(), false),
        Some(serde_json::Value::Array(items)) => {
            let image = items.iter().any(|i| str_at(i, "type") == "image");
            let text = items
                .iter()
                .filter_map(|i| match i.get("text") {
                    Some(serde_json::Value::String(s)) => Some(s.as_str()),
                    Some(t) => t.get("text").and_then(|x| x.as_str()),
                    None => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (text, image)
        }
        _ => (String::new(), false),
    }
}

/// A failed turn's reason, then its end.
fn failed(message: Option<&str>) -> Vec<Parsed> {
    let mut out = Vec::new();
    if let Some(m) = message.filter(|m| !m.is_empty()) {
        out.push(Parsed::Text(format!("\n{m}\n")));
    }
    out.push(Parsed::Done {
        is_error: true,
        duration_ms: None,
        input_tokens: None,
        output_tokens: None,
    });
    out
}

/// A Cursor call's text and verdict. `success` is looked up, not taken as the
/// first key: the map iterates alphabetically and a sibling sorting first would
/// read as a refusal.
fn cursor_result(r: Option<&serde_json::Value>) -> (String, bool, bool) {
    let Some(o) = r.and_then(|r| r.as_object()) else {
        return (String::new(), false, false);
    };
    let Some((key, val)) = o.get_key_value("success").or_else(|| o.iter().next()) else {
        return (String::new(), false, false);
    };
    if key != "success" {
        let why = ["reason", "error", "errorMessage", "clientVisibleError"]
            .iter()
            .filter_map(|k| val.get(*k).and_then(|x| x.as_str()))
            .find(|s| !s.is_empty())
            .unwrap_or(key.as_str());
        return (why.to_string(), true, false);
    }
    let (text, image) = content_text(val.get("content"));
    (
        text,
        val.get("isError").and_then(|e| e.as_bool()) == Some(true),
        image,
    )
}

/// A dsh failure's `message (code)`, or `None` when it names nothing.
fn dsh_failure_text(error: Option<&serde_json::Value>) -> Option<String> {
    let e = error?;
    let message = e.get("message").and_then(|m| m.as_str())?.trim();
    if message.is_empty() {
        return None;
    }
    Some(match e.get("code").and_then(|c| c.as_str()) {
        Some(code) if !code.is_empty() => format!("{message} ({code})"),
        _ => message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(h: Harness, lines: &[&str]) -> Vec<Parsed> {
        let mut p = StreamParser::new(h);
        lines.iter().flat_map(|l| p.push(l)).collect()
    }

    fn text_of(evs: &[Parsed]) -> String {
        evs.iter()
            .filter_map(|e| match e {
                Parsed::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    fn calls(evs: &[Parsed]) -> Vec<(Option<String>, String)> {
        evs.iter()
            .filter_map(|e| match e {
                Parsed::ToolUse { id, name, .. } => Some((id.clone(), name.clone())),
                _ => None,
            })
            .collect()
    }

    fn results(evs: &[Parsed]) -> Vec<(Option<String>, String, bool)> {
        evs.iter()
            .filter_map(|e| match e {
                Parsed::ToolResult {
                    id, text, is_error, ..
                } => Some((id.clone(), text.clone(), *is_error)),
                _ => None,
            })
            .collect()
    }

    fn done(evs: &[Parsed]) -> Option<&Parsed> {
        evs.iter().find(|e| matches!(e, Parsed::Done { .. }))
    }

    /// Blank and malformed lines produce nothing, and a non-JSON line is Plain.
    #[test]
    fn every_harness_ignores_blank_and_malformed_lines() {
        for h in Harness::ALL {
            let mut p = StreamParser::new(h);
            assert!(p.push("").is_empty());
            assert_eq!(p.last_line(), LineKind::Blank);
            assert!(p.push("Error: not logged in").is_empty());
            assert_eq!(p.last_line(), LineKind::Plain);
            assert!(p.push("{\"type\":\"unknown\"}").is_empty());
            assert_eq!(p.last_line(), LineKind::Json);
        }
    }

    /// A byte-order mark on the opening line does not lose the session id.
    #[test]
    fn a_byte_order_mark_does_not_swallow_the_session_id() {
        let evs = drive(
            Harness::Codex,
            &["\u{feff}{\"type\":\"thread.started\",\"thread_id\":\"t1\"}"],
        );
        assert_eq!(evs, vec![Parsed::Session("t1".into())]);
    }

    /// Cumulative restatements become deltas; a rewrite is sent whole.
    #[test]
    fn the_coalescer_emits_only_what_is_new() {
        let mut c = Coalescer::default();
        assert_eq!(c.advance("a", "He").as_deref(), Some("He"));
        assert_eq!(c.advance("a", "Hello").as_deref(), Some("llo"));
        assert_eq!(c.advance("a", "Hello"), None);
        assert_eq!(c.advance("a", "Bye").as_deref(), Some("Bye"));
        assert_eq!(c.extend("b", "Both ").as_deref(), Some("Both "));
        assert_eq!(c.settle("b", "Both"), None, "a trimmed copy adds nothing");
    }

    /// Claude Code: deltas, a call with its input, its result by id, the end.
    #[test]
    fn claude_turn_decodes() {
        let evs = drive(
            Harness::Claude,
            &[
                r#"{"type":"system","subtype":"init","session_id":"s-1"}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Looking"}}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu1","name":"mcp__ondin__get_node","input":{"id":"1:2"}}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":[{"type":"text","text":"a frame"},{"type":"image","source":{}}]}]}}"#,
                r#"{"type":"result","is_error":false,"duration_ms":1200,"usage":{"input_tokens":10,"output_tokens":3},"result":"Looking"}"#,
            ],
        );
        assert_eq!(evs[0], Parsed::Session("s-1".into()));
        assert_eq!(
            text_of(&evs),
            "Looking",
            "a success does not repeat the answer"
        );
        assert!(evs.contains(&Parsed::ToolUse {
            id: Some("tu1".into()),
            name: "mcp__ondin__get_node".into(),
            input: serde_json::json!({ "id": "1:2" }),
        }));
        assert!(evs.contains(&Parsed::ToolResult {
            id: Some("tu1".into()),
            text: "a frame".into(),
            is_error: false,
            image: true,
        }));
        assert_eq!(
            done(&evs),
            Some(&Parsed::Done {
                is_error: false,
                duration_ms: Some(1200),
                input_tokens: Some(10),
                output_tokens: Some(3)
            })
        );
    }

    /// An error result that streamed nothing says why; after text, apart.
    #[test]
    fn a_claude_error_result_says_why() {
        let err = r#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"}"#;
        let evs = drive(Harness::Claude, &[err]);
        assert_eq!(text_of(&evs), "Invalid API key · Please run /login");
        let evs = drive(
            Harness::Claude,
            &[
                r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}}"#,
                err,
            ],
        );
        assert!(text_of(&evs).starts_with("Hi\n\nInvalid"));
    }

    const CODEX_REAL_TOOL_CYCLE: &[&str] = &[
        r#"{"type":"thread.started","thread_id":"01a07261-62e9-7073-a8bb-405b9c4a16e5"}"#,
        r#"{"type":"item.started","item":{"id":"item_1","type":"mcp_tool_call","server":"ondin","tool":"get_tree","arguments":{},"result":null,"error":null,"status":"in_progress"}}"#,
        r###"{"type":"item.completed","item":{"id":"item_1","type":"mcp_tool_call","server":"ondin","tool":"get_tree","arguments":{},"result":{"content":[{"type":"text","text":"## Page 1\n- Frame\n"}],"structured_content":null},"error":null,"status":"completed"}}"###,
        r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"One frame."}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":41065,"cached_input_tokens":33024,"cache_write_input_tokens":0,"output_tokens":128,"reasoning_output_tokens":14}}"#,
    ];

    /// A real Codex tool cycle: the call, its result, the answer, cached input
    /// not added.
    #[test]
    fn a_real_codex_tool_cycle_decodes() {
        let evs = drive(Harness::Codex, CODEX_REAL_TOOL_CYCLE);
        assert_eq!(
            calls(&evs),
            vec![(Some("item_1".into()), "mcp__ondin__get_tree".into())]
        );
        assert_eq!(
            results(&evs),
            vec![(Some("item_1".into()), "## Page 1\n- Frame\n".into(), false)]
        );
        assert_eq!(text_of(&evs), "One frame.");
        assert!(matches!(
            done(&evs),
            Some(Parsed::Done {
                input_tokens: Some(41065),
                duration_ms: None,
                ..
            })
        ));
    }

    /// A Codex call seen only as `item.completed` still opens its own chip, and
    /// a restated completion resolves once.
    #[test]
    fn a_codex_completion_alone_opens_and_resolves_once() {
        let line = CODEX_REAL_TOOL_CYCLE[2];
        let evs = drive(Harness::Codex, &[line, line]);
        assert_eq!(calls(&evs).len(), 1);
        assert_eq!(results(&evs).len(), 1);
    }

    /// A shell command under the sandbox is shown, not swallowed.
    #[test]
    fn a_codex_side_effect_is_shown() {
        let evs = drive(
            Harness::Codex,
            &[
                r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"ls","aggregated_output":"a\n","exit_code":1,"status":"failed"}}"#,
            ],
        );
        assert_eq!(calls(&evs), vec![(Some("c1".into()), "shell".into())]);
        assert!(results(&evs)[0].2);
    }

    /// A failed turn ends the turn saying why, the reason taken from the last
    /// `error` notice when `turn.failed` names none.
    #[test]
    fn codex_failures_end_the_turn() {
        for lines in [
            &[r#"{"type":"turn.failed","error":{"message":"quota"}}"#][..],
            &[
                r#"{"type":"error","message":"quota"}"#,
                r#"{"type":"turn.failed","error":{}}"#,
            ][..],
        ] {
            let evs = drive(Harness::Codex, lines);
            assert_eq!(text_of(&evs), "\nquota\n");
            assert!(matches!(
                done(&evs),
                Some(Parsed::Done { is_error: true, .. })
            ));
        }
    }

    /// Codex's `error` while it reconnects is a diagnostic, not the turn's end:
    /// what follows is the answer, ended once by `turn.completed`.
    /// flip: `push_codex`'s `error` arm returning `failed(…)` → fails at the `Done` count.
    #[test]
    fn a_codex_retry_notice_does_not_end_the_turn() {
        let mut p = StreamParser::new(Harness::Codex);
        let mut evs = p.push(r#"{"type":"error","message":"Reconnecting... 1/5"}"#);
        assert_eq!(p.last_line(), LineKind::Plain);
        evs.extend(p.push(CODEX_REAL_TOOL_CYCLE[3]));
        evs.extend(p.push(CODEX_REAL_TOOL_CYCLE[4]));
        let ends = evs
            .iter()
            .filter(|e| matches!(e, Parsed::Done { .. }))
            .count();
        assert_eq!(ends, 1, "{evs:?}");
        assert_eq!(text_of(&evs), "One frame.");
        assert!(matches!(
            done(&evs),
            Some(Parsed::Done {
                is_error: false,
                ..
            })
        ));
    }

    /// A failure with no reason text says that it failed, in every dialect
    /// that can send one; Claude's `errors` and Cursor's `result` are read.
    /// flip: `push` without its silent-failure insert → fails at the Copilot text.
    #[test]
    fn a_failure_without_a_reason_still_says_it_failed() {
        let evs = drive(
            Harness::Copilot,
            &[r#"{"type":"result","sessionId":"s","exitCode":1}"#],
        );
        assert_eq!(
            text_of(&evs),
            "GitHub Copilot reported that this turn failed, without saying why."
        );
        let evs = drive(
            Harness::Claude,
            &[
                r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["tool crashed"]}"#,
            ],
        );
        assert_eq!(text_of(&evs), "tool crashed");
        let evs = drive(
            Harness::Cursor,
            &[r#"{"type":"result","subtype":"error","is_error":true,"result":"rate limited"}"#],
        );
        assert_eq!(text_of(&evs), "\nrate limited\n");
        let evs = drive(
            Harness::Antigravity,
            &[r#"{"event":"result","result":{"status":"ERROR","error":{"message":"quota"}}}"#],
        );
        assert_eq!(text_of(&evs), "\nquota\n");
        // Something said earlier in the turn is enough.
        let evs = drive(
            Harness::Copilot,
            &[
                r#"{"type":"assistant.message_delta","data":{"messageId":"m","deltaContent":"Hi"}}"#,
                r#"{"type":"result","exitCode":1}"#,
            ],
        );
        assert_eq!(text_of(&evs), "Hi");
    }

    /// Claude's input tokens include cached input, as the other dialects' do.
    /// flip: reading only `input_tokens` → fails at the assertion.
    #[test]
    fn claude_input_tokens_include_the_cache() {
        let evs = drive(
            Harness::Claude,
            &[
                r#"{"type":"result","is_error":false,"usage":{"input_tokens":4,"cache_read_input_tokens":38000,"cache_creation_input_tokens":2000,"output_tokens":300}}"#,
            ],
        );
        assert!(
            matches!(
                done(&evs),
                Some(Parsed::Done {
                    input_tokens: Some(40_004),
                    output_tokens: Some(300),
                    ..
                })
            ),
            "{evs:?}"
        );
    }

    const AGY_REAL_TOOL_CYCLE: &[&str] = &[
        r#"{"event":"init","conversation_id":"d0994762-e805-4c81-9298-3787bc9df864","init":{"cwd":"C:\\tmp","tools":["run_command","write_to_file"],"permission_mode":"request-review"}}"#,
        r#"{"event":"step_update","step_update":{"step_index":4,"state":"ACTIVE","step_type":"tool","tool_name":"call_mcp_tool","tool_info":{"name":"call_mcp_tool","parameters":{"Arguments":{"id":"1:2"},"ServerName":"ondin","ToolName":"get_node"}}}}"#,
        r#"{"event":"step_update","step_update":{"step_index":4,"state":"ACTIVE","step_type":"tool","tool_name":"call_mcp_tool","tool_info":{"name":"call_mcp_tool","parameters":{"Arguments":{"id":"1:2"},"ServerName":"ondin","ToolName":"get_node"}}}}"#,
        r###"{"event":"step_update","step_update":{"step_index":4,"state":"DONE","step_type":"tool","tool_name":"call_mcp_tool","duration_seconds":0.1246735,"tool_info":{"name":"call_mcp_tool","parameters":{"Arguments":{"id":"1:2"},"ServerName":"ondin","ToolName":"get_node"},"output":"## Frame\n"}}}"###,
        r#"{"event":"step_update","step_update":{"step_index":5,"state":"ACTIVE","step_type":"agent_response","text_delta":"A frame"}}"#,
        r#"{"event":"step_update","step_update":{"step_index":5,"state":"DONE","step_type":"agent_response","text_delta":"\n","duration_seconds":1.7201625,"usage":{"input_tokens":15786,"output_tokens":104}}}"#,
        r#"{"event":"result","result":{"status":"SUCCESS","response":"A frame\n","duration_seconds":4.6566974,"num_turns":1,"usage":{"input_tokens":38040,"output_tokens":597,"total_tokens":38637}}}"#,
    ];

    /// A real Antigravity turn: the nested MCP name, a restated step announced
    /// once, deltas appended, seconds turned to milliseconds.
    #[test]
    fn a_real_antigravity_turn_decodes() {
        let evs = drive(Harness::Antigravity, AGY_REAL_TOOL_CYCLE);
        assert_eq!(
            evs[0],
            Parsed::Session("d0994762-e805-4c81-9298-3787bc9df864".into())
        );
        assert_eq!(
            calls(&evs),
            vec![(Some("4".into()), "mcp__ondin__get_node".into())]
        );
        assert_eq!(
            results(&evs),
            vec![(Some("4".into()), "## Frame\n".into(), false)]
        );
        assert_eq!(text_of(&evs), "A frame\n");
        assert!(matches!(
            done(&evs),
            Some(Parsed::Done {
                duration_ms: Some(4657),
                is_error: false,
                ..
            })
        ));
    }

    /// The second turn on a persistent Antigravity reuses step ids and still
    /// gets its chips: the seen set clears at a turn's end.
    #[test]
    fn a_second_turn_reuses_step_ids() {
        let mut p = StreamParser::new(Harness::Antigravity);
        let first: Vec<Parsed> = AGY_REAL_TOOL_CYCLE.iter().flat_map(|l| p.push(l)).collect();
        let second: Vec<Parsed> = AGY_REAL_TOOL_CYCLE.iter().flat_map(|l| p.push(l)).collect();
        assert_eq!(calls(&first), calls(&second));
    }

    /// A refused tool: `SUCCESS` with `denied_actions` says so without failing.
    #[test]
    fn a_denied_antigravity_tool_is_said() {
        let evs = drive(
            Harness::Antigravity,
            &[
                r#"{"event":"step_update","step_update":{"step_index":6,"state":"ERROR","step_type":"tool","tool_name":"call_mcp_tool","tool_info":{"name":"call_mcp_tool","parameters":{"Arguments":{},"ServerName":"ondin","ToolName":"get_tree"},"error":{"type":"TOOL_ERROR","message":"permission check failed for mcp \"ondin/get_tree\""}}}}"#,
                r#"{"event":"result","result":{"status":"SUCCESS","response":"","duration_seconds":4.5,"usage":{"input_tokens":3,"output_tokens":5},"denied_actions":[{"action":"mcp"}]}}"#,
            ],
        );
        assert!(results(&evs)[0].2);
        assert!(text_of(&evs).contains("refused a tool"));
        assert!(matches!(
            done(&evs),
            Some(Parsed::Done {
                is_error: false,
                ..
            })
        ));
    }

    const OC_REAL_TOOL_CYCLE: &[&str] = &[
        r#"{"type":"step_start","timestamp":1788647017491,"sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","part":{"id":"prt_073ab8c0d001tZ8IFpFLsXlWR1","messageID":"msg_073ab87250011eYtXhMHciOVKE","sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","type":"step-start"}}"#,
        r#"{"type":"tool_use","timestamp":1788647017703,"sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","part":{"type":"tool","tool":"ondin_get_tree","callID":"call_7d7b5533c49a40c6a5d4feca","state":{"status":"completed","input":{"depth":1},"output":"Page 1","metadata":{"count":1,"truncated":false},"time":{"start":1788647017603,"end":1788647017683}},"id":"prt_073ab8c62001vDU6f5IalTNarS","sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","messageID":"msg_073ab87250011eYtXhMHciOVKE"}}"#,
        r#"{"type":"step_finish","timestamp":1788647017703,"sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","part":{"id":"prt_073ab8cd7001Z3Tclx7O3xs4hk","reason":"tool-calls","messageID":"msg_073ab87250011eYtXhMHciOVKE","sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","type":"step-finish","tokens":{"total":2536,"input":2497,"output":10,"reasoning":29,"cache":{"write":0,"read":0}},"cost":0}}"#,
        r#"{"type":"step_start","timestamp":1788647018629,"sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","part":{"id":"prt_073ab907f001lV8Cg2emVjlGnQ","messageID":"msg_073ab8cdd001Gm7WtJOOb7DqA4","sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","type":"step-start"}}"#,
        r#"{"type":"text","timestamp":1788647018724,"sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","part":{"id":"prt_073ab90c1001ClXrpFkcoD8FeO","messageID":"msg_073ab8cdd001Gm7WtJOOb7DqA4","sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","type":"text","text":"DONE","time":{"start":1788647018689,"end":1788647018716}}}"#,
        r#"{"type":"step_finish","timestamp":1788647018724,"sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","part":{"id":"prt_073ab90dd001jG5FDOUzUeQQiA","reason":"stop","messageID":"msg_073ab8cdd001Gm7WtJOOb7DqA4","sessionID":"ses_f8c547cecffep40rcGQbMKGwOv","type":"step-finish","tokens":{"total":2645,"input":637,"output":2,"reasoning":22,"cache":{"write":0,"read":1984}},"cost":0}}"#,
    ];

    /// A real OpenCode turn ends only on the terminal step, sums every step's
    /// tokens (cache reads included) and unflattens Ondin's tool name.
    #[test]
    fn a_real_opencode_turn_decodes() {
        let evs = drive(Harness::OpenCode, OC_REAL_TOOL_CYCLE);
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, Parsed::Done { .. }))
                .count(),
            1
        );
        assert_eq!(
            calls(&evs),
            vec![(
                Some("call_7d7b5533c49a40c6a5d4feca".into()),
                "mcp__ondin__get_tree".into()
            )]
        );
        assert_eq!(text_of(&evs), "DONE");
        assert_eq!(
            done(&evs),
            Some(&Parsed::Done {
                is_error: false,
                duration_ms: Some(1233),
                input_tokens: Some(2497 + 29 + 637 + 22 + 1984),
                output_tokens: Some(12),
            })
        );
    }

    /// A cut-off answer says so and is not an error.
    #[test]
    fn an_opencode_turn_cut_at_the_cap_says_so() {
        let evs = drive(
            Harness::OpenCode,
            &[r#"{"type":"step_finish","part":{"reason":"length"}}"#],
        );
        assert!(text_of(&evs).contains("output limit"));
        assert!(matches!(
            done(&evs),
            Some(Parsed::Done {
                is_error: false,
                ..
            })
        ));
    }

    const COPILOT_REAL_TURN: &[&str] = &[
        r#"{"type":"session.mcp_server_status_changed","data":{"serverName":"ondin","status":"connected"},"ephemeral":true}"#,
        r#"{"type":"user.message","data":{"content":"Call get_tree, then get_node.","turnId":"0"}}"#,
        r#"{"type":"assistant.turn_start","data":{"turnId":"0"}}"#,
        r#"{"type":"assistant.message","data":{"messageId":"m0","content":"","toolRequests":[{"toolCallId":"call_A","name":"ondin-get_tree","arguments":{},"type":"function","mcpServerName":"ondin","mcpToolName":"get_tree"}],"turnId":"0","phase":"final_answer"}}"#,
        r#"{"type":"tool.execution_start","data":{"toolCallId":"call_A","toolName":"ondin-get_tree","arguments":{},"turnId":"0","mcpServerName":"ondin","mcpToolName":"get_tree"}}"#,
        r#"{"type":"tool.execution_start","data":{"toolCallId":"call_B","toolName":"ondin-get_node","arguments":{"id":"1:2"},"turnId":"0","mcpServerName":"ondin","mcpToolName":"get_node"}}"#,
        r#"{"type":"tool.execution_complete","data":{"toolCallId":"call_A","turnId":"0","success":true,"result":{"content":"Page 1","contents":[{"type":"text","text":"Page 1"}]}}}"#,
        r#"{"type":"tool.execution_complete","data":{"toolCallId":"call_B","turnId":"0","success":false,"error":{"message":"no such node"}}}"#,
        r#"{"type":"assistant.turn_end","data":{"turnId":"0"}}"#,
        r#"{"type":"assistant.turn_start","data":{"turnId":"1"}}"#,
        r#"{"type":"assistant.message_delta","data":{"messageId":"m2","deltaContent":"Both "},"ephemeral":true}"#,
        r#"{"type":"assistant.message_delta","data":{"messageId":"m2","deltaContent":"returned."},"ephemeral":true}"#,
        r#"{"type":"assistant.message","data":{"messageId":"m2","content":"Both returned.","toolRequests":[],"turnId":"1","phase":"final_answer"}}"#,
        r#"{"type":"assistant.turn_end","data":{"turnId":"1"}}"#,
        r#"{"type":"result","timestamp":"2026-09-26T00:07:09.000Z","sessionId":"04765e2f-d530-4495-847b-c339fd860b4a","exitCode":0,"usage":{"premiumRequests":1,"totalApiDurationMs":929,"sessionDurationMs":1992}}"#,
    ];

    /// Copilot: two parallel calls paired by id, text printed once, the session
    /// id ahead of the end, and only `result` ending the turn.
    #[test]
    fn a_real_copilot_turn_decodes() {
        let evs = drive(Harness::Copilot, COPILOT_REAL_TURN);
        assert_eq!(
            results(&evs),
            vec![
                (Some("call_A".into()), "Page 1".into(), false),
                (Some("call_B".into()), "no such node".into(), true),
            ]
        );
        assert_eq!(text_of(&evs), "Both returned.");
        let n = evs.len();
        assert_eq!(
            evs[n - 2],
            Parsed::Session("04765e2f-d530-4495-847b-c339fd860b4a".into())
        );
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, Parsed::Done { .. }))
                .count(),
            1
        );
    }

    const CURSOR_REAL_TURN: &[&str] = &[
        r#"{"type":"system","subtype":"init","apiKeySource":"login","cwd":"C:\\run\\cursor","session_id":"470de677-c5df-45ab-9d87-190a6fb230f3","model":"Auto","permissionMode":"default"}"#,
        r#"{"type":"thinking","subtype":"delta","text":"Looking up","timestamp_ms":1790388081000}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll look"}]},"timestamp_ms":1790388082000}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":" it up."}]},"timestamp_ms":1790388082010}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll look it up."}]},"model_call_id":"92b929ea-0-kjrn","timestamp_ms":1790388082030}"#,
        r#"{"type":"tool_call","subtype":"started","call_id":"call-0\nfc_0","tool_call":{"getMcpToolsToolCall":{"args":{"server":"ondin"}}},"model_call_id":"92b929ea-0-kjrn","timestamp_ms":1790388082305}"#,
        r#"{"type":"tool_call","subtype":"started","call_id":"call-1\nfc_1","tool_call":{"mcpToolCall":{"args":{"name":"ondin-get_node","args":{"id":"1:2"},"providerIdentifier":"ondin","toolName":"get_node","serverIdentifier":"ondin"}}},"model_call_id":"92b929ea-1-8ah8","timestamp_ms":1790388083914}"#,
        r#"{"type":"tool_call","subtype":"completed","call_id":"call-1\nfc_1","tool_call":{"mcpToolCall":{"args":{"name":"ondin-get_node","args":{"id":"1:2"},"providerIdentifier":"ondin","toolName":"get_node","serverIdentifier":"ondin"},"result":{"success":{"content":[{"text":{"text":"Frame 1"}}],"isError":false}}}},"model_call_id":"92b929ea-1-8ah8","timestamp_ms":1790388083966}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"It is a frame."}]},"timestamp_ms":1790388084100}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"It is a frame."}]}}"#,
        r#"{"type":"result","subtype":"success","duration_ms":6413,"is_error":false,"result":"…","usage":{"inputTokens":9747,"outputTokens":215,"cacheReadTokens":26240,"cacheWriteTokens":0}}"#,
    ];

    /// Cursor: each segment once though written twice, the catalogue lookup
    /// hidden, the MCP result nested one level deeper, cached input counted.
    #[test]
    fn a_real_cursor_turn_decodes() {
        let evs = drive(Harness::Cursor, CURSOR_REAL_TURN);
        assert_eq!(text_of(&evs), "I'll look it up.It is a frame.");
        assert_eq!(
            calls(&evs),
            vec![(Some("call-1\nfc_1".into()), "mcp__ondin__get_node".into())]
        );
        assert_eq!(results(&evs)[0].1, "Frame 1");
        assert!(evs.contains(&Parsed::ToolUse {
            id: Some("call-1\nfc_1".into()),
            name: "mcp__ondin__get_node".into(),
            input: serde_json::json!({ "id": "1:2" }),
        }));
        assert!(matches!(
            done(&evs),
            Some(Parsed::Done {
                input_tokens: Some(35987),
                ..
            })
        ));
    }

    /// A refused Cursor call carries its reason; `success` beside another key
    /// is still success.
    #[test]
    fn cursor_verdicts() {
        let r = serde_json::json!({ "rejected": { "reason": "User rejected MCP" } });
        assert_eq!(
            cursor_result(Some(&r)),
            ("User rejected MCP".into(), true, false)
        );
        let r = serde_json::json!({ "isBackground": false, "success": { "content": "ok" } });
        assert_eq!(cursor_result(Some(&r)), ("ok".into(), false, false));
    }

    const DSH_REAL_TURN: &[&str] = &[
        r#"{"type":"session","sessionId":"session-3330cd81-e848-4259-b094-6bd4d4c45a81","cwd":"C:\\run\\dsh"}"#,
        r#"{"type":"status","phase":"turn_start","turn":1}"#,
        r#"{"type":"thinking","text":"The user wants the frame."}"#,
        r#"{"type":"tool_call","callId":"call_00_a","tool":"mcp__ondin__find_nodes","input":{"name":"Hero"}}"#,
        r#"{"type":"tool_call","callId":"call_01_b","tool":"mcp__ondin__get_tree","input":{}}"#,
        r#"{"type":"tool_result","callId":"call_01_b","status":"completed","result":"Page 1\n"}"#,
        r###"{"type":"tool_result","callId":"call_00_a","status":"completed","result":"## Hero\n"}"###,
        r#"{"type":"status","phase":"step_end","turn":1,"step":1,"usage":{"inputTokens":2726,"outputTokens":106,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":2832}}"#,
        r#"{"type":"text","text":"Hero is a frame."}"#,
        r#"{"type":"status","phase":"step_end","turn":1,"step":2,"usage":{"inputTokens":187,"outputTokens":2,"cacheReadTokens":2944,"cacheWriteTokens":0,"totalTokens":3133}}"#,
        r#"{"type":"status","phase":"turn_end","turn":1,"reason":{"kind":"completed"}}"#,
        r#"{"type":"final","sessionId":"session-3330cd81-e848-4259-b094-6bd4d4c45a81","reason":{"kind":"completed"},"text":"Hero is a frame."}"#,
    ];

    /// DeepSeek: results paired by id out of order, the answer once, thinking
    /// never, every step's usage summed.
    #[test]
    fn a_real_deepseek_turn_decodes() {
        let evs = drive(Harness::DeepSeek, DSH_REAL_TURN);
        assert_eq!(
            results(&evs),
            vec![
                (Some("call_01_b".into()), "Page 1\n".into(), false),
                (Some("call_00_a".into()), "## Hero\n".into(), false),
            ]
        );
        assert_eq!(text_of(&evs), "Hero is a frame.");
        assert_eq!(
            done(&evs),
            Some(&Parsed::Done {
                is_error: false,
                duration_ms: None,
                input_tokens: Some(2726 + 187 + 2944),
                output_tokens: Some(108),
            })
        );
    }

    /// A failed DeepSeek turn says why; a cut answer is finished from `final`.
    #[test]
    fn deepseek_failure_and_cut() {
        let evs = drive(
            Harness::DeepSeek,
            &[
                r#"{"type":"status","phase":"turn_end","reason":{"kind":"error","error":{"message":"Insufficient Balance","code":"ACCOUNT_QUOTA"}}}"#,
                r#"{"type":"final","text":""}"#,
            ],
        );
        assert_eq!(text_of(&evs), "Insufficient Balance (ACCOUNT_QUOTA)");
        assert!(matches!(
            done(&evs),
            Some(Parsed::Done { is_error: true, .. })
        ));
        let evs = drive(
            Harness::DeepSeek,
            &[
                r#"{"type":"text","text":"abc","truncated":true}"#,
                r#"{"type":"final","reason":{"kind":"completed"},"text":"abcdef"}"#,
            ],
        );
        assert_eq!(text_of(&evs), "abcdef");
    }
}
