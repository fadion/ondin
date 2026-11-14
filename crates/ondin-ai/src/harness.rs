//! Which coding agent drives the assistant, what it can do, and how it is sealed.
//!
//! Behaviour is asked as a capability (`is_persistent`, `prompt_on_stdin`, …),
//! never as `== Harness::Claude`: each capability is an exhaustive `match`, so a
//! new harness is a compile error rather than a quiet default on the unsafe side.
//!
//! Every flag, config shape and stream quirk here was measured against the real
//! program; a harness is never driven on an argv read off documentation alone.

use crate::MCP_SERVER;

/// A coding agent's program, named by the wire dialect it speaks: a fork that
/// still speaks its parent's stream counts as the parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Harness {
    /// `claude`: one process per conversation, `stream-json` both ways.
    Claude,
    /// `codex exec --json`: one process per turn, resumed by thread id.
    Codex,
    /// `agy`: one process per conversation, NDJSON both ways; killed to stop
    /// and resumed with `--conversation`.
    Antigravity,
    /// `opencode run --format json`: one process per turn, whole parts rather
    /// than deltas, and no turn-completion event (the end is read off
    /// `step_finish.reason`).
    OpenCode,
    /// `copilot --output-format json`: one process per turn; the session id
    /// arrives only on the closing `result`.
    Copilot,
    /// `cursor-agent -p --output-format stream-json`: one process per turn,
    /// deltas followed by whole-segment restatements.
    Cursor,
    /// `dsh --profile headless --json`: one process per turn, a step at a time.
    DeepSeek,
}

impl Harness {
    /// Every harness, in the order Settings offers them.
    pub const ALL: [Harness; 7] = [
        Harness::Claude,
        Harness::Codex,
        Harness::Antigravity,
        Harness::OpenCode,
        Harness::Copilot,
        Harness::Cursor,
        Harness::DeepSeek,
    ];

    /// The persisted key, matched back by [`Harness::from_key`]. Changing one
    /// retargets every saved setting.
    pub fn key(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Antigravity => "antigravity",
            Harness::OpenCode => "opencode",
            Harness::Copilot => "copilot",
            Harness::Cursor => "cursor",
            Harness::DeepSeek => "deepseek",
        }
    }

    /// Parse a persisted key. An unknown one (`gemini` included) is `None`,
    /// never another harness: the caller decides, where it can say so.
    pub fn from_key(key: &str) -> Option<Harness> {
        Harness::ALL.into_iter().find(|h| h.key() == key)
    }

    /// The product's name, as Settings and every notice write it.
    pub fn label(self) -> &'static str {
        match self {
            Harness::Claude => "Claude Code",
            Harness::Codex => "Codex",
            Harness::Antigravity => "Antigravity",
            Harness::OpenCode => "OpenCode",
            Harness::Copilot => "GitHub Copilot",
            Harness::Cursor => "Cursor",
            Harness::DeepSeek => "DeepSeek Harness",
        }
    }

    /// The program looked for. Cursor's installer also puts `agent` on `PATH`,
    /// a word other tools claim, so the longer name is the one searched.
    pub fn program(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Antigravity => "agy",
            Harness::OpenCode => "opencode",
            Harness::Copilot => "copilot",
            Harness::Cursor => "cursor-agent",
            Harness::DeepSeek => "dsh",
        }
    }

    /// Who a reply is from: the assistant, not the product or the vendor.
    pub fn speaker(self) -> &'static str {
        match self {
            Harness::Claude => "Claude",
            Harness::Codex => "Codex",
            Harness::Antigravity => "Antigravity",
            Harness::OpenCode => "OpenCode",
            Harness::Copilot => "Copilot",
            Harness::Cursor => "Cursor",
            Harness::DeepSeek => "DeepSeek",
        }
    }

    /// Model names offered as chips, never as the permitted set: the field is
    /// free text and passed verbatim.
    pub fn model_suggestions(self) -> &'static [&'static str] {
        match self {
            Harness::Claude => &["haiku", "sonnet", "opus"],
            Harness::Codex => &["gpt-5.4", "gpt-5.4-codex", "o3"],
            Harness::Antigravity => &[],
            // `provider/model`: a bare name is not an id this CLI accepts.
            Harness::OpenCode => &[
                "opencode/claude-sonnet-5",
                "opencode/claude-opus-5",
                "opencode/gpt-5",
                "opencode/gemini-3.1-pro",
            ],
            // The catalogue is the account's; `auto` is the one every plan takes.
            Harness::Copilot | Harness::Cursor => &["auto"],
            Harness::DeepSeek => &["deepseek-flash", "deepseek-v4-pro"],
        }
    }

    /// The effort levels this harness takes, in its own words, lowest first.
    /// Empty hides the Effort row.
    pub fn effort_levels(self) -> &'static [&'static str] {
        match self {
            Harness::Claude => &["low", "medium", "high", "xhigh"],
            Harness::Antigravity => &["low", "medium", "high"],
            // `--variant`'s own examples; the accepted set varies by model.
            Harness::OpenCode => &["minimal", "high", "max"],
            // In the overlay's `reasoningEffort`; `off` was never run.
            Harness::DeepSeek => &["low", "high", "max"],
            // Copilot's flag fails the turn on the default `auto` model, and
            // Cursor's effort is part of the model id.
            Harness::Codex | Harness::Copilot | Harness::Cursor => &[],
        }
    }

    /// The level of this harness's nearest to `wanted`, a level of `from`'s;
    /// a tie goes to the lower level, and `""` when this harness has none.
    pub fn nearest_effort(self, wanted: &str, from: Harness) -> String {
        let mine = self.effort_levels();
        let Some(target) = effort_rank(wanted, from) else {
            return String::new();
        };
        mine.iter()
            .filter_map(|l| effort_rank(l, self).map(|r| (r, *l)))
            .min_by(|(a, _), (b, _)| {
                (a - target)
                    .abs()
                    .total_cmp(&(b - target).abs())
                    .then(a.total_cmp(b))
            })
            .map(|(_, l)| l.to_string())
            .unwrap_or_default()
    }

    /// Whether a turn's images reach the agent. Only Claude Code (content
    /// blocks) and Codex (`--image`, on `exec` and `exec resume` alike) were
    /// measured to take them.
    pub fn takes_images(self) -> bool {
        match self {
            Harness::Claude | Harness::Codex => true,
            Harness::Antigravity
            | Harness::OpenCode
            | Harness::Copilot
            | Harness::Cursor
            | Harness::DeepSeek => false,
        }
    }

    /// The arguments that print the help page the seal is read from. Codex
    /// keeps `--ignore-user-config` and `--sandbox` off its top-level page;
    /// DeepSeek's headless profile has a page of its own.
    pub(crate) fn help_args(self) -> &'static [&'static str] {
        match self {
            Harness::Codex => &["exec", "--help"],
            Harness::DeepSeek => &["--profile", "headless", "--help"],
            Harness::Claude
            | Harness::Antigravity
            | Harness::OpenCode
            | Harness::Copilot
            | Harness::Cursor => &["--help"],
        }
    }

    /// One process holds the whole conversation, turns arriving on stdin.
    pub(crate) fn is_persistent(self) -> bool {
        match self {
            Harness::Claude | Harness::Antigravity => true,
            Harness::Codex
            | Harness::OpenCode
            | Harness::Copilot
            | Harness::Cursor
            | Harness::DeepSeek => false,
        }
    }

    /// A per-turn harness takes its prompt on stdin rather than as an argument,
    /// which keeps it out of the world-readable command line and out of a
    /// batch shim's refusal. Each was measured; OpenCode has no stdin form.
    pub(crate) fn prompt_on_stdin(self) -> bool {
        match self {
            Harness::Codex | Harness::Copilot | Harness::Cursor | Harness::DeepSeek => true,
            Harness::Claude | Harness::Antigravity | Harness::OpenCode => false,
        }
    }

    /// The system prompt rides in the first turn's text, once per conversation,
    /// rather than in a flag. Only Claude Code has `--append-system-prompt`.
    pub(crate) fn system_in_first_turn(self) -> bool {
        match self {
            Harness::Claude => false,
            Harness::Codex
            | Harness::Antigravity
            | Harness::OpenCode
            | Harness::Copilot
            | Harness::Cursor
            | Harness::DeepSeek => true,
        }
    }

    /// The stdin line that ends a turn without ending the process, or `None`
    /// where Stop has to kill. Antigravity silently ignores an unknown event,
    /// so a guessed interrupt would hang rather than fail.
    pub(crate) fn interrupt_line(self) -> Option<String> {
        match self {
            Harness::Claude => Some(json_line(serde_json::json!({
                "type": "control_request",
                "request_id": "stop",
                "request": { "subtype": "interrupt" }
            }))),
            Harness::Codex
            | Harness::Antigravity
            | Harness::OpenCode
            | Harness::Copilot
            | Harness::Cursor
            | Harness::DeepSeek => None,
        }
    }

    /// The prefix of the CLI's own lines on a stderr that also carries the
    /// model's reasoning; `None` where stderr is the CLI's alone.
    pub(crate) fn stderr_diagnostic_prefix(self) -> Option<&'static str> {
        match self {
            Harness::DeepSeek => Some("dsh:"),
            Harness::Claude
            | Harness::Codex
            | Harness::Antigravity
            | Harness::OpenCode
            | Harness::Copilot
            | Harness::Cursor => None,
        }
    }

    /// The level to send, from this harness's own list, or `None` to send none.
    pub(crate) fn effort_arg(self, requested: &str) -> Option<&'static str> {
        self.effort_levels()
            .iter()
            .copied()
            .find(|l| *l == requested.trim())
    }

    /// The persistent harness's stdin line for one turn, images first.
    pub(crate) fn turn_line(self, text: &str, images: &[Vec<u8>]) -> String {
        match self {
            Harness::Claude => {
                use base64::Engine as _;
                let mut content: Vec<serde_json::Value> = images
                    .iter()
                    .map(|png| {
                        serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": base64::engine::general_purpose::STANDARD.encode(png),
                            }
                        })
                    })
                    .collect();
                content.push(serde_json::json!({ "type": "text", "text": text }));
                json_line(serde_json::json!({
                    "type": "user",
                    "message": { "role": "user", "content": content }
                }))
            }
            // Its own envelope: Claude's `{"type":"user"}` is refused for
            // missing the `event` field.
            Harness::Antigravity => json_line(serde_json::json!({
                "event": "user",
                "message": { "content": text }
            })),
            Harness::Codex
            | Harness::OpenCode
            | Harness::Copilot
            | Harness::Cursor
            | Harness::DeepSeek => String::new(),
        }
    }
}

/// `v` as one newline-terminated NDJSON line.
fn json_line(v: serde_json::Value) -> String {
    format!("{v}\n")
}

/// A level's place on one scale shared by every harness, `minimal` to `max`;
/// a word not on it is placed by its position in `from`'s own list.
fn effort_rank(level: &str, from: Harness) -> Option<f64> {
    const SCALE: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
    if let Some(i) = SCALE.iter().position(|s| *s == level) {
        return Some(i as f64);
    }
    let levels = from.effort_levels();
    let i = levels.iter().position(|l| *l == level)?;
    let span = (levels.len().max(2) - 1) as f64;
    Some(i as f64 / span * (SCALE.len() - 1) as f64)
}

/// How well a harness's own tools are shut off. Ordered worst to best.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Seal {
    /// The probe could not confirm the program takes its restricting options.
    /// A session is refused.
    Unknown,
    /// A sandbox or a rules file holds the agent's own tools back; they can
    /// still read this machine.
    Restricted,
    /// The agent's own tools are off; only Ondin's remain.
    Sealed,
}

/// Claude Code's sealing flags, each passed only when its `--help` lists it: an
/// older build dies on an unknown option rather than running degraded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeFlags {
    pub tools: bool,
    pub setting_sources: bool,
    pub strict_mcp_config: bool,
    /// `--append-system-prompt-file`, which keeps the system prompt (and the
    /// conversation it replays) off the length-limited, world-readable argv.
    pub system_prompt_file: bool,
}

impl ClaudeFlags {
    /// Every flag: a current build, and the answer for an unreadable probe,
    /// where a spawn that fails loudly beats one that runs unsealed.
    pub const ALL: ClaudeFlags = ClaudeFlags {
        tools: true,
        setting_sources: true,
        strict_mcp_config: true,
        system_prompt_file: true,
    };
}

/// What one `--help` says about a program, for the harness it is believed to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Probe {
    pub seal: Seal,
    /// Claude Code's flags; [`ClaudeFlags::ALL`] for every other harness.
    pub claude: ClaudeFlags,
    /// Codex's `--ignore-user-config`, when its page lists it.
    pub isolate_config: bool,
}

impl Probe {
    /// What a probe that ran reads off its exit status and both streams (`agy`
    /// prints its help on stderr). A failed `--help` printed a diagnostic, and
    /// no seal is read out of one.
    pub(crate) fn read(h: Harness, exit_ok: bool, stdout: &str, stderr: &str) -> Probe {
        let both = format!("{stdout}{stderr}");
        let readable = exit_ok && looks_like_help(&both);
        Probe {
            seal: match exit_ok {
                true => seal_from_help(h, &both),
                false => Seal::Unknown,
            },
            claude: match readable {
                true => ClaudeFlags {
                    tools: mentions_flag(&both, "--tools"),
                    setting_sources: mentions_flag(&both, "--setting-sources"),
                    strict_mcp_config: mentions_flag(&both, "--strict-mcp-config"),
                    // Listed only inside another flag's text, as `[-file]`.
                    system_prompt_file: both.contains("--append-system-prompt[-file]")
                        || mentions_flag(&both, "--append-system-prompt-file"),
                },
                false => ClaudeFlags::ALL,
            },
            isolate_config: h == Harness::Codex
                && readable
                && mentions_flag(&both, "--ignore-user-config"),
        }
    }
}

/// A harness's seal read off its help page. An unreadable page is `Unknown`:
/// passing `--sandbox` to a program that never heard of it kills the spawn, and
/// assuming the seal holds is how a read-only promise becomes a shell.
pub(crate) fn seal_from_help(h: Harness, help: &str) -> Seal {
    if !looks_like_help(help) {
        return Seal::Unknown;
    }
    let has = |f: &str| mentions_flag(help, f);
    match h {
        // `--tools ""` empties the built-in set; without it a denylist stands in.
        Harness::Claude if has("--tools") => Seal::Sealed,
        Harness::Claude => Seal::Restricted,
        // A sandbox: the agent's own readers stay live. Antigravity's needs its
        // plan mode too, which is what refuses its writes and commands.
        Harness::Codex if has("--sandbox") => Seal::Restricted,
        Harness::Antigravity if has("--sandbox") && has("--mode") => Seal::Restricted,
        // The seal is a config file and `--help` cannot show it; `--pure` is
        // passed every turn and marks the build the mechanism was measured on.
        Harness::OpenCode if has("--pure") => Seal::Sealed,
        // An allowlist of the tools the model is shown. It has no denylist to
        // fall back on, so a build without it is refused.
        Harness::Copilot if has("--available-tools") => Seal::Sealed,
        // A permissions file in its working directory; `--trust` is passed every
        // turn and was measured alongside it.
        Harness::Cursor if has("--trust") => Seal::Restricted,
        // An overlay re-checked before every turn; the page vouches for the
        // headless interface the argv rests on.
        Harness::DeepSeek if has("--json") && has("--session-id") => Seal::Sealed,
        _ => Seal::Unknown,
    }
}

/// Is this a help page at all, or a diagnostic the probe caught (a shim whose
/// `node` is missing, an auth wall)? It must list a flag every build has.
fn looks_like_help(help: &str) -> bool {
    ["--help", "--print", "--model", "--version"]
        .iter()
        .any(|f| mentions_flag(help, f))
}

/// Does `help` list exactly this flag, not one it is a prefix of?
fn mentions_flag(help: &str, flag: &str) -> bool {
    help.match_indices(flag).any(|(i, _)| {
        help[i + flag.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '-')
    })
}

/// The sentence Settings and the panel's head show under the agent: the
/// mechanism, where the person's own MCP servers can reach the session, and
/// at `Restricted` outside Windows that the access level is a rail.
pub(crate) fn seal_sentence(h: Harness, seal: Seal) -> String {
    let label = h.label();
    let mut s = match (h, seal) {
        (_, Seal::Unknown) => {
            return format!(
                "Ondin could not confirm that this {label} program accepts the options that \
                 restrict it, so it will not start it. Check the program path in Settings."
            );
        }
        (Harness::Claude, Seal::Sealed) => format!(
            "{label} runs with its own tools off and only Ondin's server loaded, so it can use \
             Ondin's tools and nothing else."
        ),
        (Harness::Claude, Seal::Restricted) => format!(
            "This {label} build does not accept the option that turns its own tools off, so a \
             denylist holds them back instead, which is weaker. Updating {label} restores the \
             full seal."
        ),
        (Harness::Codex | Harness::Antigravity, Seal::Restricted) => format!(
            "{label} runs in its read-only sandbox: its own tools cannot write files or run \
             commands, though they can still read this machine."
        ),
        (Harness::Cursor, Seal::Restricted) => format!(
            "{label} is held to Ondin's tools by a permissions file Ondin writes for the session, \
             not by a sandbox: its own tools are refused writing files and running commands, but \
             they can still read files on this machine."
        ),
        (Harness::OpenCode, Seal::Sealed) => format!(
            "{label} runs as an agent with every one of its own tools switched off, in a \
             configuration of Ondin's own, so it can use Ondin's tools and nothing else."
        ),
        (Harness::Copilot, Seal::Sealed) => format!(
            "{label} is started with only Ondin's tools available and its own configuration set \
             aside, so it can use Ondin's tools and nothing else."
        ),
        (Harness::DeepSeek, Seal::Sealed) => format!(
            "{label} runs with every one of its tool plugins disabled and its own configuration \
             set aside, checked again before every turn, so it can use Ondin's tools and \
             nothing else."
        ),
        (_, _) => format!(
            "Ondin could not fully restrict {label}'s own tools, so it may be able to do more \
             than use Ondin's."
        ),
    };
    if !isolates_mcp_servers(h) {
        s.push_str(&format!(
            " MCP servers you have registered with {label} also reach this session, and Ondin \
             cannot keep them away."
        ));
    }
    if seal == Seal::Restricted && !cfg!(windows) {
        s.push_str(
            " Outside Windows the access level is a safety rail against an eager model, not a \
             boundary against a hostile one.",
        );
    }
    s
}

/// Does a session see only Ondin's server, or the person's own as well? `agy mcp
/// add` appends to the person's own registry, and Cursor reads
/// `~/.cursor/mcp.json` from a hard-coded home path; no option closes either.
pub(crate) fn isolates_mcp_servers(h: Harness) -> bool {
    match h {
        Harness::Claude
        | Harness::Codex
        | Harness::OpenCode
        | Harness::Copilot
        | Harness::DeepSeek => true,
        Harness::Antigravity | Harness::Cursor => false,
    }
}

/// Everything one turn's (or a persistent session's) command line needs.
#[derive(Clone, Debug, Default)]
pub(crate) struct TurnSpec {
    pub prompt: String,
    pub system: String,
    pub model: String,
    pub effort: String,
    /// The conversation to continue, from the previous turn's stream.
    pub resume: Option<String>,
    /// `system`, written to a file in the run folder for Claude Code.
    pub system_file: Option<String>,
    /// The file the harness reads Ondin's server from: Claude's `--mcp-config`,
    /// Copilot's `--additional-mcp-config`, DeepSeek's `--patch` overlay.
    pub mcp_config: Option<String>,
    /// Codex's `-c` overrides carrying the server.
    pub mcp_overrides: Vec<String>,
    /// Codex's `--ignore-user-config`, when the probe saw it.
    pub isolate_config: bool,
    /// The bare tool names this chat's access level grants.
    pub allowed_tools: Vec<String>,
    /// Image files for Codex's `--image`.
    pub images: Vec<String>,
}

/// What Claude Code's denylist names: a backstop behind `--tools ""` for a
/// build too old for it, holding the tools whose misuse is worst. A name the
/// CLI does not know is harmless; one missing from here is not.
const CLAUDE_DISALLOWED: &[&str] = &[
    "Bash",
    "Edit",
    "Write",
    "Read",
    "Glob",
    "Grep",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Task",
    "Artifact",
    "CronCreate",
    "CronDelete",
    "CronList",
    "DesignSync",
    "EnterWorktree",
    "ExitWorktree",
    "ListAgents",
    "Monitor",
    "PushNotification",
    "RemoteTrigger",
    "ReportFindings",
    "ScheduleWakeup",
    "SendMessage",
    "Skill",
    "TaskOutput",
    "TaskStop",
    "ToolSearch",
    "Workflow",
];

/// The argv of a persistent session; empty for a per-turn harness.
///
/// Claude Code: `--strict-mcp-config` keeps the person's own servers out,
/// `--setting-sources user` keeps a settings file beside the working folder from
/// configuring it, and `--tools ""` empties its built-ins. The three go before
/// the variadic `--allowedTools`, which would read a later flag as a tool name;
/// `--disallowedTools` is last for the same reason.
pub(crate) fn session_args(h: Harness, spec: &TurnSpec, flags: ClaudeFlags) -> Vec<String> {
    let model = spec.model.trim();
    let effort = h.effort_arg(&spec.effort).unwrap_or_default();
    let resume = spec.resume.as_deref().filter(|s| !s.is_empty());
    match h {
        Harness::Claude => {
            let mut a: Vec<String> = [
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--permission-mode",
                "default",
            ]
            .map(String::from)
            .to_vec();
            if flags.strict_mcp_config {
                a.push("--strict-mcp-config".into());
            }
            if flags.setting_sources {
                a.extend(["--setting-sources".into(), "user".into()]);
            }
            if flags.tools {
                a.extend(["--tools".into(), String::new()]);
            }
            if !model.is_empty() {
                a.extend(["--model".into(), model.into()]);
            }
            if !effort.is_empty() {
                a.extend(["--effort".into(), effort.into()]);
            }
            if let Some(id) = resume {
                a.extend(["--resume".into(), id.into()]);
            }
            match spec.system_file.as_deref() {
                Some(file) if flags.system_prompt_file => {
                    a.extend(["--append-system-prompt-file".into(), file.into()]);
                }
                _ if !spec.system.trim().is_empty() => {
                    a.extend(["--append-system-prompt".into(), spec.system.clone()]);
                }
                _ => {}
            }
            if let Some(cfg) = spec.mcp_config.as_deref() {
                a.extend(["--mcp-config".into(), cfg.into()]);
                if !spec.allowed_tools.is_empty() {
                    a.push("--allowedTools".into());
                    a.extend(
                        spec.allowed_tools
                            .iter()
                            .map(|t| format!("mcp__{MCP_SERVER}__{t}")),
                    );
                }
            }
            a.push("--disallowedTools".into());
            a.extend(CLAUDE_DISALLOWED.iter().map(|t| t.to_string()));
            a
        }
        Harness::Antigravity => {
            // No `-p`: it takes the prompt as its value and would read the next
            // flag as the prompt. `--mode plan` is the seal: `--sandbox` alone
            // restricts its terminal, yet in a folder the person trusts (their
            // home, say) its own tools still write files and run commands.
            let mut a: Vec<String> = [
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--sandbox",
                "--disable-slash-commands",
                "--mode",
                "plan",
            ]
            .map(String::from)
            .to_vec();
            if !model.is_empty() {
                a.extend(["--model".into(), model.into()]);
            }
            if !effort.is_empty() {
                a.extend(["--effort".into(), effort.into()]);
            }
            // Only after a Stop had to kill the process holding the conversation.
            if let Some(id) = resume {
                a.extend(["--conversation".into(), id.into()]);
            }
            a
        }
        Harness::Codex
        | Harness::OpenCode
        | Harness::Copilot
        | Harness::Cursor
        | Harness::DeepSeek => Vec::new(),
    }
}

/// The argv of one turn on a per-turn harness; empty for a persistent one.
/// The prompt is in it only where [`Harness::prompt_on_stdin`] is false.
pub(crate) fn turn_args(h: Harness, spec: &TurnSpec) -> Vec<String> {
    let model = spec.model.trim();
    let effort = h.effort_arg(&spec.effort).unwrap_or_default();
    let resume = spec.resume.as_deref().filter(|s| !s.is_empty());
    let mut a: Vec<String> = Vec::new();
    match h {
        Harness::Claude | Harness::Antigravity => return Vec::new(),
        Harness::Codex => {
            a.push("exec".into());
            // A sub-subcommand: it must follow `exec` immediately.
            if let Some(id) = resume {
                a.extend(["resume".into(), id.into()]);
            }
            a.push("--json".into());
            // `exec resume` has no `--sandbox`; the `-c` below holds on both.
            if resume.is_none() {
                a.extend(["--sandbox".into(), "read-only".into()]);
            }
            if spec.isolate_config {
                a.push("--ignore-user-config".into());
            }
            // The working folder is not a git repository.
            a.push("--skip-git-repo-check".into());
            if !model.is_empty() {
                a.extend(["--model".into(), model.into()]);
            }
            for img in &spec.images {
                a.push(format!("--image={img}"));
            }
            for o in &spec.mcp_overrides {
                a.extend(["-c".into(), o.clone()]);
            }
            // Last: a later `-c` wins, so nothing added above can outrank it.
            a.extend(["-c".into(), "sandbox_mode=\"read-only\"".into()]);
        }
        Harness::OpenCode => {
            // `--pure` keeps the person's plugins (arbitrary code) out, and spares
            // a fresh config folder a minutes-long silent plugin install.
            // `--agent` selects the sealed agent; without it the run is `build`,
            // which has every tool. No `--auto`: it pre-approves anything.
            a.extend(
                [
                    "run",
                    "--pure",
                    "--agent",
                    OPENCODE_AGENT,
                    "--format",
                    "json",
                ]
                .map(String::from),
            );
            if !model.is_empty() {
                a.extend(["--model".into(), model.into()]);
            }
            if !effort.is_empty() {
                a.extend(["--variant".into(), effort.into()]);
            }
            if let Some(id) = resume {
                a.extend(["--session".into(), id.into()]);
            }
        }
        Harness::Copilot => {
            // Every value joined with `=`: the tool flags are variadic and
            // `--resume` takes an optional value, so a separate value is one parse
            // away from swallowing the next flag.
            a.extend(["--output-format", "json", "--stream", "on"].map(String::from));
            a.push(copilot_available_tools(&spec.allowed_tools));
            a.extend(
                [
                    "--disable-builtin-mcps",
                    "--no-custom-instructions",
                    "--no-auto-update",
                    "--no-ask-user",
                ]
                .map(String::from),
            );
            if let Some(cfg) = spec.mcp_config.as_deref() {
                a.push(format!("--additional-mcp-config=@{cfg}"));
                // Approval, apart from visibility: headless there is nobody to
                // ask, and it names only Ondin's server.
                a.push(format!("--allow-tool={MCP_SERVER}"));
            }
            if !model.is_empty() {
                a.extend(["--model".into(), model.into()]);
            }
            // `--resume`, not `--session-id`: the second silently starts a fresh
            // session when the id is unknown.
            if let Some(id) = resume {
                a.push(format!("--resume={id}"));
            }
        }
        Harness::Cursor => {
            // `--trust`: without it a headless run in a new folder stops at a
            // trust prompt. `--mode ask` is a prompt, not the lever; the
            // permissions file is. Never `--approve-mcps`, `--force` or `--yolo`.
            a.extend(
                [
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--stream-partial-output",
                    "--trust",
                    "--mode",
                    "ask",
                ]
                .map(String::from),
            );
            if !model.is_empty() {
                a.extend(["--model".into(), model.into()]);
            }
            // Last: its value is optional, so nothing may follow it.
            if let Some(id) = resume {
                a.extend(["--resume".into(), id.into()]);
            }
        }
        Harness::DeepSeek => {
            // Model, effort, seal and server all ride in the overlay: headless
            // takes none of them as a flag.
            a.extend(dsh_profile_args(spec.mcp_config.as_deref()));
            a.push("--json".into());
            if let Some(id) = resume {
                a.extend(["--session-id".into(), id.into()]);
            }
        }
    }
    if !h.prompt_on_stdin() {
        a.push(prefixed_prompt(turn_system(spec), &spec.prompt));
    }
    a
}

/// What a per-turn harness reads on stdin for one turn, or `None` when the
/// prompt is in the argv. One construction, two destinations.
pub(crate) fn turn_stdin(h: Harness, spec: &TurnSpec) -> Option<String> {
    h.prompt_on_stdin()
        .then(|| prefixed_prompt(turn_system(spec), &spec.prompt))
}

/// The system prompt, folded above the person's text with a blank line so its
/// last line does not read as the start of the question.
pub(crate) fn prefixed_prompt(system: &str, prompt: &str) -> String {
    if system.trim().is_empty() {
        return prompt.to_string();
    }
    format!("{system}\n\n{prompt}")
}

/// Once per conversation: a resumed turn replays the thread, which already
/// carries the system prompt.
fn turn_system(spec: &TurnSpec) -> &str {
    match spec.resume.as_deref().filter(|s| !s.is_empty()) {
        Some(_) => "",
        None => &spec.system,
    }
}

/// `--profile headless`, and `--patch <overlay>` when there is one.
pub(crate) fn dsh_profile_args(overlay: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = vec!["--profile".into(), "headless".into()];
    if let Some(p) = overlay.filter(|p| !p.is_empty()) {
        a.extend(["--patch".into(), p.into()]);
    }
    a
}

/// The arguments that print the composed profile a run with `overlay` would
/// mount. Boots nothing and calls no model.
pub(crate) fn dsh_dump_args(overlay: &str) -> Vec<String> {
    let mut a = dsh_profile_args(Some(overlay));
    a.push("--dump-config".into());
    a
}

/// The OpenCode agent Ondin defines and selects. A definition nothing selects
/// leaves the run on `build`, which has every tool.
pub(crate) const OPENCODE_AGENT: &str = "ondin";

/// Every tool `opencode agent create --permissions` lists, all off. The map is
/// a denylist by omission, so a name here the CLI dropped costs nothing and a
/// name it adds is live: hence over-inclusive.
fn opencode_sealed_tools() -> serde_json::Map<String, serde_json::Value> {
    [
        "bash",
        "read",
        "edit",
        "write",
        "glob",
        "grep",
        "webfetch",
        "websearch",
        "task",
        "todowrite",
        "lsp",
        "skill",
    ]
    .iter()
    .map(|t| ((*t).to_string(), serde_json::Value::Bool(false)))
    .collect()
}

/// The `opencode.json` that seals an OpenCode session and gives it Ondin's
/// server. `XDG_CONFIG_HOME` pointing at the folder that holds this file and
/// `OPENCODE_TEST_HOME` at an empty one keep the person's files out; the
/// resolved result is checked with [`opencode_config_leaks`] before a turn.
pub(crate) fn opencode_config_json(exe: &str, args: &[String], allowed: &[String]) -> String {
    let mut command = vec![exe.to_string()];
    command.extend(args.iter().cloned());
    serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": {
            MCP_SERVER: { "type": "local", "enabled": true, "command": command }
        },
        "agent": {
            OPENCODE_AGENT: {
                "mode": "primary",
                "description": format!(
                    "Ondin's design assistant. Tools available: {}.",
                    allowed.join(", ")
                ),
                "tools": opencode_sealed_tools(),
            }
        }
    })
    .to_string()
}

/// What in `opencode debug config`'s output reaches past Ondin's tools: an MCP
/// server other than Ondin's, or a tool or permission the sealed agent allows.
/// `None` when the output is not a configuration at all.
pub(crate) fn opencode_config_leaks(output: &str) -> Option<Vec<String>> {
    let json = &output[output.find('{')?..];
    let v: serde_json::Value = serde_json::Deserializer::from_str(json)
        .into_iter()
        .next()?
        .ok()?;
    let agent = v.pointer(&format!("/agent/{OPENCODE_AGENT}"))?;
    let mut leaks = Vec::new();
    for (name, server) in v
        .get("mcp")
        .and_then(|m| m.as_object())
        .into_iter()
        .flatten()
    {
        let enabled = server.get("enabled").and_then(|e| e.as_bool()) != Some(false);
        if name != MCP_SERVER && enabled {
            leaks.push(format!("the MCP server {name}"));
        }
    }
    for (tool, on) in agent
        .get("tools")
        .and_then(|t| t.as_object())
        .into_iter()
        .flatten()
    {
        if on.as_bool() != Some(false) {
            leaks.push(format!("its {tool} tool"));
        }
    }
    let allows = |v: &serde_json::Value| match v {
        serde_json::Value::String(s) => s != "deny",
        serde_json::Value::Object(m) => m.values().any(|x| x.as_str() != Some("deny")),
        _ => true,
    };
    for (what, rule) in agent
        .get("permission")
        .and_then(|p| p.as_object())
        .into_iter()
        .flatten()
    {
        if allows(rule) && !leaks.iter().any(|l| l == &format!("its {what} tool")) {
            leaks.push(format!("its {what} permission"));
        }
    }
    Some(leaks)
}

/// `-c` overrides giving Codex Ondin's server and only Ondin's: the whole
/// `mcp_servers` table is assigned, displacing the person's own. `codex exec`
/// cannot ask, so each granted tool is approved by name or every call is refused.
/// Codex starts a server with a scrubbed environment, so the child marker is
/// passed in its `env`.
pub(crate) fn codex_mcp_overrides(exe: &str, args: &[String], allowed: &[String]) -> Vec<String> {
    let tools = allowed
        .iter()
        .map(|t| format!("{t}={{approval_mode=\"approve\"}}"))
        .collect::<Vec<_>>()
        .join(",");
    let args = args
        .iter()
        .map(|a| toml_str(a))
        .collect::<Vec<_>>()
        .join(",");
    vec![format!(
        "mcp_servers={{{MCP_SERVER}={{command={},args=[{args}],env={{{}=\"1\"}},tools={{{tools}}}}}}}",
        toml_str(exe),
        crate::AGENT_CHILD_ENV,
    )]
}

/// A TOML basic string. Every separator in a Windows path is an escape
/// introducer, so quoting alone would change the path.
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A Copilot tool name nothing provides. `--available-tools` with an empty
/// value fails open (every tool, the shell included), so an empty grant names
/// this instead; measured to resolve to no tools.
const COPILOT_NO_TOOLS: &str = "ondin-no_tools";

/// The `--available-tools` argument, in Copilot's `<server>-<tool>` form.
pub(crate) fn copilot_available_tools(allowed: &[String]) -> String {
    match allowed.is_empty() {
        true => format!("--available-tools={COPILOT_NO_TOOLS}"),
        false => format!(
            "--available-tools={}",
            allowed
                .iter()
                .map(|t| format!("{MCP_SERVER}-{t}"))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// The file `--additional-mcp-config=@…` names. It augments the home's own
/// config, which is why the session also runs under a `COPILOT_HOME` of its own.
pub(crate) fn copilot_mcp_config_json(exe: &str, args: &[String]) -> String {
    serde_json::json!({
        "mcpServers": {
            MCP_SERVER: { "type": "local", "command": exe, "args": args, "tools": ["*"] }
        }
    })
    .to_string()
}

/// What Cursor's permissions file always denies. `Write`, `Shell` and
/// `WebFetch` were measured to hold, through a subagent too. `Read` and `Grep`
/// close the obvious route only: `Grep` still returned a file's contents.
const CURSOR_DENY: &[&str] = &[
    "Write(**)",
    "Shell(*)",
    "WebFetch(*)",
    "Read(**)",
    "Grep(**)",
];

/// The project `.cursor/cli.json` that restricts a Cursor session. An MCP call
/// with no allow rule is refused headless, so each granted tool is allowed by
/// name; a missing file fails open for writes, so the session is refused
/// without it.
pub(crate) fn cursor_permissions_json(allowed: &[String]) -> String {
    let allow: Vec<String> = allowed
        .iter()
        .map(|t| format!("Mcp({MCP_SERVER}:{t})"))
        .collect();
    serde_json::json!({ "permissions": { "allow": allow, "deny": CURSOR_DENY } }).to_string()
}

/// The project `.cursor/mcp.json` giving a Cursor session Ondin's server. It
/// loads only once `cursor-agent mcp enable ondin` approves it. Cursor starts a
/// server without the agent's environment, so the child marker is in `env`.
pub(crate) fn cursor_mcp_config_json(exe: &str, args: &[String]) -> String {
    serde_json::json!({
        "mcpServers": { MCP_SERVER: {
            "command": exe,
            "args": args,
            "env": { crate::AGENT_CHILD_ENV: "1" },
        } }
    })
    .to_string()
}

/// The `claude --mcp-config` file: Ondin's server, and only it.
pub(crate) fn claude_mcp_config_json(exe: &str, args: &[String]) -> String {
    serde_json::json!({
        "mcpServers": { MCP_SERVER: { "type": "stdio", "command": exe, "args": args } }
    })
    .to_string()
}

/// The dsh headless profile's rows whose plugins hand the model a tool, every
/// one disabled by the overlay. A denylist: [`dsh_unsealed_rows`] is what
/// notices a plugin a later build adds.
const DSH_TOOL_ROWS: &[&str] = &[
    "tool-pwsh",
    "tool-bash",
    "tool-fs",
    "tool-fs-search",
    "tool-web",
    "tool-jobs",
    "tool-skill",
    "tool-subagent",
    "tool-subagent-fork",
    "tool-subagent-control",
    "tool-subagent-list-agents",
    "tool-workflow",
    "tool-working-directory",
    "tool-todo",
    "tool-goal",
];

/// The id of the row the overlay inserts for Ondin's server.
const DSH_MCP_ROW: &str = "mcp-ondin";

/// Plugins named `dsh-tool-…` that register no tool: the executors every call
/// runs through, and a timeout policy over them.
const DSH_TOOL_NAMED_NON_TOOLS: &[&str] = &[
    "@deepseek-ai/dsh-tool-executor-native",
    "@deepseek-ai/dsh-tool-executor-ptc",
    "@deepseek-ai/dsh-tool-call-timeout-policy",
];

/// Which DeepSeek route signs a run in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DshProvider {
    /// The account the desktop app signs in with, read from the person's own
    /// credentials file by path.
    Account,
    /// A `DEEPSEEK_API_KEY` in the person's environment, passed through.
    ApiKey,
}

impl DshProvider {
    fn id(self) -> &'static str {
        match self {
            DshProvider::Account => "deepseek-account",
            DshProvider::ApiKey => "deepseek-official",
        }
    }
}

/// Everything the DeepSeek overlay carries.
pub(crate) struct DshOverlay<'a> {
    pub exe: &'a str,
    pub args: &'a [String],
    /// The person's own `.credentials.yaml`, read in place, never copied.
    pub credentials: &'a str,
    pub provider: DshProvider,
    pub model: &'a str,
    pub effort: &'a str,
}

/// A YAML single-quoted scalar, control characters made spaces. The profile
/// loader evaluates `!!js`, so a typed model name must never become a tag or a
/// line of its own.
fn yaml_quoted(s: &str) -> String {
    let clean: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("'{}'", clean.replace('\'', "''"))
}

/// The `--patch` overlay a DeepSeek run is composed with: every tool row
/// disabled, the sign-in rows turned back on and pointed at the person's own
/// credentials, the model (a patch replaces that row's config wholesale), and
/// Ondin's server as an `insert`, since a patch naming an absent id only warns.
pub(crate) fn dsh_overlay_yaml(o: &DshOverlay<'_>) -> String {
    let model = match o.model.trim() {
        "" => Harness::DeepSeek.model_suggestions()[0],
        m => m,
    };
    let mut y = String::from("# Written by Ondin for one DeepSeek Harness session.\n");
    y.push_str(&format!(
        "- id: credentials\n  disabled: false\n  config:\n    path: {}\n",
        yaml_quoted(o.credentials)
    ));
    y.push_str("- id: authorization\n  disabled: false\n");
    y.push_str("- id: deepseek-account\n  disabled: false\n");
    y.push_str(&format!(
        "- id: agent-default-model\n  config:\n    provider: {}\n    model: {}\n",
        yaml_quoted(o.provider.id()),
        yaml_quoted(model)
    ));
    if let Some(e) = Harness::DeepSeek.effort_arg(o.effort) {
        y.push_str(&format!("    reasoningEffort: {}\n", yaml_quoted(e)));
    }
    for id in DSH_TOOL_ROWS {
        y.push_str(&format!("- id: {id}\n  disabled: true\n"));
    }
    let args = o
        .args
        .iter()
        .map(|a| yaml_quoted(a))
        .collect::<Vec<_>>()
        .join(", ");
    y.push_str(&format!(
        "- insert:\n    - id: {DSH_MCP_ROW}\n      name: '@deepseek-ai/dsh-mcp-client'\n      \
         config:\n        serverName: {MCP_SERVER}\n        transport: stdio\n        \
         command: {}\n        args: [{args}]\n        failOnStartupError: true\n",
        yaml_quoted(o.exe),
    ));
    y
}

/// The plugins in a composed dsh profile (`--dump-config`) that would still
/// hand the model a tool, as `id (name)`; empty when the run is sealed. By
/// plugin name, so a tool row the overlay has never heard of is caught. A
/// `!!js` `disabled` is not literal and does not count as disabled.
pub(crate) fn dsh_unsealed_rows(dump: &str) -> Vec<String> {
    struct Row {
        indent: usize,
        id: String,
        name: String,
        disabled: bool,
    }
    fn unquote(v: &str) -> String {
        let v = v.trim();
        match v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            Some(inner) => inner.replace("''", "'"),
            None => v.trim_matches('"').to_string(),
        }
    }
    fn key(row: &mut Row, t: &str) {
        if let Some(v) = t.strip_prefix("id:") {
            row.id = unquote(v);
        } else if let Some(v) = t.strip_prefix("name:") {
            row.name = unquote(v);
        } else if let Some(v) = t.strip_prefix("disabled:") {
            row.disabled = v.trim() == "true";
        }
    }
    // A `- key: …` item opens a row whichever key comes first, but only at the
    // depth of the first row: a list inside a row's config is not a row.
    fn opens(t: &str) -> Option<&str> {
        let item = t.strip_prefix("- ")?;
        let (k, _) = item.split_once(':')?;
        (!k.is_empty()
            && k.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
        .then_some(item)
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut depth: Option<usize> = None;
    for line in dump.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim_start();
        if let Some(item) = opens(t)
            && *depth.get_or_insert(indent) == indent
        {
            let mut row = Row {
                indent,
                id: String::new(),
                name: String::new(),
                disabled: false,
            };
            key(&mut row, item);
            rows.push(row);
            continue;
        }
        // A key of the latest row at exactly its own depth; deeper ones are
        // its config's.
        let Some(row) = rows.last_mut() else { continue };
        if indent == row.indent + 2 {
            key(row, t);
        }
    }
    rows.into_iter()
        .filter(|r| !r.disabled)
        .filter(|r| {
            let n = r.name.as_str();
            let tool =
                n.starts_with("@deepseek-ai/dsh-tool-") && !DSH_TOOL_NAMED_NON_TOOLS.contains(&n);
            let mcp = n == "@deepseek-ai/dsh-mcp-client" && r.id != DSH_MCP_ROW;
            let foreign = !n.is_empty() && !n.starts_with("@deepseek-ai/");
            tool || n.ends_with("/tools") || mcp || foreign
        })
        .map(|r| match r.id.is_empty() {
            true => r.name,
            false => format!("{} ({})", r.id, r.name),
        })
        .collect()
}

/// Is `dump` a composed headless profile at all? An empty dump or a usage
/// error has no unsealed rows and proves nothing; the headless runner row is in
/// every real one.
pub(crate) fn dsh_dump_is_headless(dump: &str) -> bool {
    dump.lines().any(|l| {
        let t = l.trim_start();
        t.strip_prefix("- ")
            .unwrap_or(t)
            .strip_prefix("name:")
            .is_some_and(|v| v.trim().trim_matches('\'') == "@deepseek-ai/dsh-headless")
    })
}

/// The name one Antigravity session registers Ondin's server under: `ondin`
/// and eight hex digits, never the person's own `ondin`, so a session never
/// replaces or removes a registration it did not make.
pub(crate) fn antigravity_server_name(tag: &str) -> String {
    format!("{MCP_SERVER}{}", &tag[..tag.len().min(8)])
}

/// Is `name` a session's own registration, as [`antigravity_server_name`] makes it?
pub(crate) fn is_antigravity_session_server(name: &str) -> bool {
    name.strip_prefix(MCP_SERVER).is_some_and(|t| {
        t.len() == 8
            && t.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The `permissions.allow` rules Antigravity needs to run Ondin's tools under
/// `server`, one per granted tool. Headless it cannot ask, so a tool with no
/// rule is denied and the turn still reports success with an empty answer.
pub(crate) fn antigravity_allow_rules(server: &str, allowed: &[String]) -> Vec<String> {
    allowed
        .iter()
        .map(|t| format!("mcp({server}/{t})"))
        .collect()
}

/// Is `rule` one this session's registration grants? Only rules naming its own
/// server are: the person's `mcp(ondin/…)` rules are theirs.
fn is_ondin_rule(rule: &str, server: &str) -> bool {
    rule.starts_with(&format!("mcp({server}/"))
}

/// What a settings surgery asks its caller to do. `Unchanged` writes nothing:
/// a re-serialised file sorts keys and drops the trailing newline, which is a
/// price only an edit that changes something should pay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsEdit {
    Unchanged,
    Write(String),
}

/// Make Antigravity's `settings.json` grant exactly `rules` for `server`, every
/// other rule and key kept. `None` when the file is not a JSON object: losing
/// the person's file is worse than a session without tools.
pub(crate) fn antigravity_settings_with_only(
    current: &str,
    server: &str,
    rules: &[String],
) -> Option<SettingsEdit> {
    let mut doc = parse_settings(current)?;
    let mut changed = false;
    {
        let perms = doc
            .entry("permissions")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()?;
        let arr = perms
            .entry("allow")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()))
            .as_array_mut()?;
        let before = arr.len();
        arr.retain(|v| {
            !v.as_str()
                .is_some_and(|s| is_ondin_rule(s, server) && !rules.iter().any(|r| r == s))
        });
        changed |= arr.len() != before;
        for r in rules {
            let v = serde_json::Value::String(r.clone());
            if !arr.contains(&v) {
                arr.push(v);
                changed = true;
            }
        }
    }
    if !changed {
        return Some(SettingsEdit::Unchanged);
    }
    Some(SettingsEdit::Write(render_settings(doc)?))
}

/// Withdraw every rule for `server` from Antigravity's `settings.json`. Only the
/// containers this call emptied are pruned; an `allow: []` the person had stays.
pub(crate) fn antigravity_settings_without(current: &str, server: &str) -> Option<SettingsEdit> {
    let mut doc = parse_settings(current)?;
    let mut removed = false;
    if let Some(perms) = doc.get_mut("permissions").and_then(|p| p.as_object_mut()) {
        let mut emptied = false;
        if let Some(arr) = perms.get_mut("allow").and_then(|a| a.as_array_mut()) {
            let before = arr.len();
            arr.retain(|v| !v.as_str().is_some_and(|s| is_ondin_rule(s, server)));
            removed = arr.len() != before;
            emptied = removed && arr.is_empty();
        }
        if emptied {
            perms.remove("allow");
            if perms.is_empty() {
                doc.remove("permissions");
            }
        }
    }
    if !removed {
        return Some(SettingsEdit::Unchanged);
    }
    Some(SettingsEdit::Write(render_settings(doc)?))
}

/// A settings document written back, with the trailing newline editors leave.
fn render_settings(doc: serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let mut s = serde_json::to_string_pretty(&serde_json::Value::Object(doc)).ok()?;
    s.push('\n');
    Some(s)
}

/// A settings document, or `None` if it is not a JSON object. An empty file is
/// a fresh install's, read as an empty object.
fn parse_settings(current: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    if current.trim().is_empty() {
        return Some(serde_json::Map::new());
    }
    match serde_json::from_str::<serde_json::Value>(current) {
        Ok(serde_json::Value::Object(m)) => Some(m),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TurnSpec {
        TurnSpec {
            prompt: "what is this layer?".into(),
            system: "You are Ondin's assistant.".into(),
            allowed_tools: vec!["get_node".into(), "export_png".into()],
            ..Default::default()
        }
    }

    /// Keys round-trip, and an unknown key (Gemini's included) is no harness.
    #[test]
    fn keys_round_trip_and_gemini_is_unknown() {
        for h in Harness::ALL {
            assert_eq!(Harness::from_key(h.key()), Some(h));
        }
        assert_eq!(Harness::from_key("gemini"), None);
        assert_eq!(Harness::from_key(""), None);
    }

    /// Effort moves to the nearest level the new harness has, ties downward.
    #[test]
    fn effort_moves_to_the_nearest_level_ties_down() {
        use Harness::*;
        assert_eq!(Antigravity.nearest_effort("xhigh", Claude), "high");
        assert_eq!(Claude.nearest_effort("max", DeepSeek), "xhigh");
        // `medium` sits between `low` and `high`: the tie goes down.
        assert_eq!(DeepSeek.nearest_effort("medium", Claude), "low");
        assert_eq!(OpenCode.nearest_effort("low", Claude), "minimal");
        assert_eq!(Codex.nearest_effort("high", Claude), "");
        assert_eq!(Claude.nearest_effort("", Claude), "");
        assert_eq!(Claude.nearest_effort("high", Claude), "high");
    }

    /// A level from another harness's vocabulary is never sent.
    #[test]
    fn effort_is_clamped_to_the_harness_own_levels() {
        let mut s = spec();
        s.effort = "xhigh".into();
        assert!(!turn_args(Harness::OpenCode, &s).contains(&"--variant".to_string()));
        let a = session_args(Harness::Antigravity, &s, ClaudeFlags::ALL);
        assert!(!a.contains(&"--effort".to_string()));
    }

    /// Antigravity runs in plan mode, and a build without it is not sealed.
    /// flip: `session_args` without `--mode plan` → fails at `plan mode`.
    #[test]
    fn antigravity_runs_in_plan_mode() {
        let a = session_args(Harness::Antigravity, &spec(), ClaudeFlags::ALL);
        assert!(
            a.windows(2).any(|w| w[0] == "--mode" && w[1] == "plan"),
            "plan mode"
        );
        let old = "  --sandbox    Run in a sandbox\n  -h, --help";
        assert_eq!(seal_from_help(Harness::Antigravity, old), Seal::Unknown);
        let new = "  --mode    Set the mode\n  --sandbox    Run in a sandbox\n  -h, --help";
        assert_eq!(seal_from_help(Harness::Antigravity, new), Seal::Restricted);
    }

    const CLAUDE_HELP: &str = "\
  --allowedTools, --allowed-tools <tools...>
  --setting-sources <sources>           Comma-separated list of setting sources
  --strict-mcp-config                   Only use MCP servers from --mcp-config,
  --tools <tools...>                    Specify the list of available tools from
                                        --append-system-prompt[-file], --add-dir
  -h, --help                            Display help for command";

    /// Claude's grade turns on `--tools`, and an unreadable probe is `Unknown`
    /// for the seal while passing every Claude flag.
    #[test]
    fn the_seal_is_read_off_help_and_fails_closed() {
        let p = Probe::read(Harness::Claude, true, CLAUDE_HELP, "");
        assert_eq!(p.seal, Seal::Sealed);
        assert_eq!(p.claude, ClaudeFlags::ALL);
        let old = "  --mcp-config <configs...>\n  -h, --help  Display help";
        let p = Probe::read(Harness::Claude, true, old, "");
        assert_eq!(p.seal, Seal::Restricted);
        assert!(!p.claude.tools && !p.claude.strict_mcp_config);
        // A failed `--help` printed a diagnostic, not help.
        let p = Probe::read(Harness::Claude, false, CLAUDE_HELP, "");
        assert_eq!(p.seal, Seal::Unknown);
        assert_eq!(p.claude, ClaudeFlags::ALL);
        // Text that is not a help page (a shim's error) proves nothing.
        assert_eq!(
            seal_from_help(Harness::Codex, "node: not found"),
            Seal::Unknown
        );
        assert_eq!(
            seal_from_help(Harness::Copilot, "  --model <m>\n  -h, --help"),
            Seal::Unknown
        );
    }

    /// A flag is not found inside a longer flag's name.
    #[test]
    fn a_flag_is_not_found_inside_a_longer_one() {
        assert!(!mentions_flag("  --toolsmith <x>\n", "--tools"));
        assert!(!mentions_flag("  --tools-list\n", "--tools"));
        assert!(mentions_flag("  --tools <x>\n", "--tools"));
        assert!(mentions_flag("--tools", "--tools"));
    }

    /// Codex's isolation flag is read off the `exec` page, which the probe asks.
    #[test]
    fn codex_isolation_is_read_off_the_exec_page() {
        assert_eq!(Harness::Codex.help_args(), &["exec", "--help"]);
        let page = "  --sandbox <MODE>\n  --ignore-user-config\n  -h, --help";
        let p = Probe::read(Harness::Codex, true, page, "");
        assert_eq!(p.seal, Seal::Restricted);
        assert!(p.isolate_config);
        let p = Probe::read(Harness::Claude, true, page, "");
        assert!(!p.isolate_config, "only Codex reads it");
    }

    /// Claude's seal flags precede the variadic allow-list, the allow-list is
    /// qualified, and the denylist is last.
    #[test]
    fn claude_session_argv_order() {
        let mut s = spec();
        s.mcp_config = Some("cfg.json".into());
        let a = session_args(Harness::Claude, &s, ClaudeFlags::ALL);
        let at = |f: &str| a.iter().position(|x| x == f).unwrap();
        assert!(at("--strict-mcp-config") < at("--allowedTools"));
        assert!(at("--tools") < at("--allowedTools"));
        assert_eq!(a[at("--tools") + 1], "");
        assert_eq!(a[at("--allowedTools") + 1], "mcp__ondin__get_node");
        assert_eq!(a[at("--allowedTools") + 2], "mcp__ondin__export_png");
        assert_eq!(
            a[at("--disallowedTools") + 1..].len(),
            CLAUDE_DISALLOWED.len()
        );
        assert_eq!(a[at("--append-system-prompt") + 1], s.system);
        for never in ["--dangerously-skip-permissions", "bypassPermissions"] {
            assert!(!a.iter().any(|x| x == never));
        }
    }

    /// OpenCode's resolved configuration is read for what it adds: another
    /// server, a tool switched back on, a permission allowed. Fixture shapes are
    /// `opencode debug config` output from 1.19.5.
    /// flip: `opencode_config_leaks` skipping the `mcp` loop → fails at the first `assert_eq`.
    #[test]
    fn opencode_leaks_are_read_off_its_resolved_config() {
        let leaky = r#"{"mcp":{"ondin":{"type":"local","enabled":true,"command":["ondin"]},
            "fs":{"type":"local","enabled":true,"command":["npx"]}},
            "agent":{"ondin":{"mode":"primary","tools":{"bash":true,"read":false},
            "permission":{"bash":"allow","read":"deny"}}}}"#;
        assert_eq!(
            opencode_config_leaks(leaky),
            Some(vec!["the MCP server fs".into(), "its bash tool".into()])
        );
        let sealed = r#"{"mcp":{"ondin":{"enabled":true},"off":{"enabled":false}},
            "agent":{"ondin":{"tools":{"bash":false},
            "permission":{"bash":"deny","edit":{"*":"deny"}}}}}"#;
        assert_eq!(opencode_config_leaks(sealed), Some(Vec::new()));
        let opened = r#"{"agent":{"ondin":{"permission":{"webfetch":"ask"}}}}"#;
        assert_eq!(
            opencode_config_leaks(opened),
            Some(vec!["its webfetch permission".into()])
        );
        assert_eq!(opencode_config_leaks("Error: unknown command"), None);
        assert_eq!(opencode_config_leaks(r#"{"agent":{}}"#), None);
    }

    /// A build that reads the system prompt from a file gets the file, and the
    /// prompt's text is nowhere on its command line; an older build gets the text.
    /// flip: `session_args` ignoring `system_file` → fails at the first `!any`.
    #[test]
    fn claude_takes_its_system_prompt_from_a_file_where_it_can() {
        let mut s = spec();
        s.system_file = Some("sys.md".into());
        let flags = Probe::read(Harness::Claude, true, CLAUDE_HELP, "").claude;
        assert!(flags.system_prompt_file);
        let a = session_args(Harness::Claude, &s, flags);
        assert!(!a.iter().any(|x| x.contains(&s.system)));
        let at = a
            .iter()
            .position(|x| x == "--append-system-prompt-file")
            .unwrap();
        assert_eq!(a[at + 1], "sys.md");
        let help = "  --append-system-prompt <prompt>\n  -h, --help";
        let old = Probe::read(Harness::Claude, true, help, "").claude;
        assert!(!old.system_prompt_file);
        let a = session_args(Harness::Claude, &s, old);
        assert!(a.contains(&s.system));
    }

    /// A resumed Codex turn has no `--sandbox` (absent from `exec resume`), the
    /// sandbox key is the last `-c`, and the prompt is on stdin only.
    #[test]
    fn codex_turn_argv() {
        let mut s = spec();
        s.mcp_overrides = vec!["mcp_servers={}".into()];
        let first = turn_args(Harness::Codex, &s);
        assert_eq!(&first[..2], &["exec", "--json"]);
        assert!(first.contains(&"--sandbox".to_string()));
        assert_eq!(first.last().unwrap(), "sandbox_mode=\"read-only\"");
        assert!(!first.iter().any(|a| a.contains(&s.prompt)));
        assert!(
            turn_stdin(Harness::Codex, &s)
                .unwrap()
                .starts_with(&s.system)
        );
        s.resume = Some("t-1".into());
        let next = turn_args(Harness::Codex, &s);
        assert_eq!(&next[..3], &["exec", "resume", "t-1"]);
        assert!(!next.contains(&"--sandbox".to_string()));
        // The system prompt is not sent again on a resumed turn.
        assert_eq!(turn_stdin(Harness::Codex, &s).unwrap(), s.prompt);
    }

    /// OpenCode takes the prompt last, as an argument, under the sealed agent.
    #[test]
    fn opencode_turn_argv() {
        let a = turn_args(Harness::OpenCode, &spec());
        assert_eq!(&a[..4], &["run", "--pure", "--agent", OPENCODE_AGENT]);
        assert!(a.last().unwrap().ends_with("what is this layer?"));
        assert!(turn_stdin(Harness::OpenCode, &spec()).is_none());
    }

    /// The prompt travels exactly once, on stdin or in the argv.
    #[test]
    fn the_prompt_travels_exactly_once() {
        for h in Harness::ALL.into_iter().filter(|h| !h.is_persistent()) {
            let s = spec();
            let in_argv = turn_args(h, &s).iter().any(|a| a.contains(&s.prompt));
            assert_ne!(in_argv, turn_stdin(h, &s).is_some(), "{h:?}");
        }
    }

    /// An empty Copilot grant names a tool nothing provides: the empty flag
    /// value is every tool.
    #[test]
    fn copilot_never_sends_an_empty_allowlist() {
        assert_eq!(
            copilot_available_tools(&[]),
            "--available-tools=ondin-no_tools"
        );
        let mut s = spec();
        s.mcp_config = Some("m.json".into());
        s.resume = Some("abc".into());
        let a = turn_args(Harness::Copilot, &s);
        assert!(a.contains(&"--available-tools=ondin-get_node,ondin-export_png".to_string()));
        assert!(a.contains(&"--allow-tool=ondin".to_string()));
        assert!(a.contains(&"--resume=abc".to_string()));
        assert!(!a.iter().any(|x| x.contains("allow-all")));
    }

    /// Cursor's `--resume` is last, since its value is optional.
    #[test]
    fn cursor_resume_is_last() {
        let mut s = spec();
        s.resume = Some("c-1".into());
        let a = turn_args(Harness::Cursor, &s);
        assert_eq!(&a[a.len() - 2..], &["--resume", "c-1"]);
        for never in ["--approve-mcps", "--force", "--yolo"] {
            assert!(!a.iter().any(|x| x == never));
        }
    }

    /// Cursor's server is started carrying the agent marker.
    #[test]
    fn cursor_server_carries_the_agent_marker() {
        let v: serde_json::Value =
            serde_json::from_str(&cursor_mcp_config_json("ondin", &["mcp-proxy".into()])).unwrap();
        assert_eq!(
            v["mcpServers"][MCP_SERVER]["env"][crate::AGENT_CHILD_ENV],
            "1"
        );
    }

    /// Codex's server override names Ondin's server with each tool approved,
    /// the agent marker in its environment, and Windows paths survive TOML.
    #[test]
    fn codex_override_approves_each_granted_tool() {
        let o = codex_mcp_overrides(
            r"C:\Ondin\ondin.exe",
            &["mcp-proxy".into(), "--client".into(), "k".into()],
            &["get_node".into()],
        );
        assert_eq!(
            o[0],
            r#"mcp_servers={ondin={command="C:\\Ondin\\ondin.exe",args=["mcp-proxy","--client","k"],env={ONDIN_AGENT_CHILD="1"},tools={get_node={approval_mode="approve"}}}}"#
        );
    }

    /// The OpenCode agent switches every listed built-in off and is the one the
    /// argv selects.
    #[test]
    fn opencode_config_seals_the_agent_it_names() {
        let v: serde_json::Value =
            serde_json::from_str(&opencode_config_json("ondin", &[], &["get_node".into()]))
                .unwrap();
        let tools = v
            .pointer("/agent/ondin/tools")
            .unwrap()
            .as_object()
            .unwrap();
        assert!(tools.contains_key("bash"));
        assert!(tools.values().all(|t| t == false));
        assert_eq!(v.pointer("/mcp/ondin/command/0").unwrap(), "ondin");
    }

    /// The overlay disables every tool row, quotes what the person typed, and
    /// inserts Ondin's server.
    #[test]
    fn the_dsh_overlay_seals_and_quotes() {
        let y = dsh_overlay_yaml(&DshOverlay {
            exe: "ondin",
            args: &["mcp-proxy".into()],
            credentials: "/home/p/.dsh/.credentials.yaml",
            provider: DshProvider::Account,
            model: "x'\n- id: tool-bash\n  disabled: false",
            effort: "high",
        });
        for id in DSH_TOOL_ROWS {
            assert!(y.contains(&format!("- id: {id}\n  disabled: true\n")));
        }
        assert!(y.contains("model: 'x'' - id: tool-bash   disabled: false'"));
        assert!(y.contains("reasoningEffort: 'high'"));
        assert!(y.contains("serverName: ondin"));
        assert!(y.contains("args: ['mcp-proxy']"));
    }

    /// A tool plugin left live is found by name, Ondin's own server row is not,
    /// and an empty dump is not evidence of a seal.
    #[test]
    fn a_live_dsh_tool_row_is_found_by_name() {
        let dump = "\
- id: headless
  name: '@deepseek-ai/dsh-headless'
- id: tool-bash
  name: '@deepseek-ai/dsh-tool-bash'
  disabled: true
- id: tool-new
  name: '@deepseek-ai/dsh-tool-new'
- id: mcp-ondin
  name: '@deepseek-ai/dsh-mcp-client'
- id: exec
  name: '@deepseek-ai/dsh-tool-executor-native'
- name: some-plugin/tools
  config:
    - name: '@deepseek-ai/dsh-tool-inner'
";
        assert!(dsh_dump_is_headless(dump));
        assert_eq!(
            dsh_unsealed_rows(dump),
            vec![
                "tool-new (@deepseek-ai/dsh-tool-new)".to_string(),
                "some-plugin/tools".to_string()
            ]
        );
        assert!(!dsh_dump_is_headless(""));
    }

    /// Granting replaces this session's previous rules with exactly this
    /// level's and keeps everyone else's, the person's `mcp(ondin/…)` included;
    /// granting the same again writes nothing.
    /// flip: `is_ondin_rule` matching any `mcp(ondin` prefix → fails at the allow list.
    #[test]
    fn antigravity_rules_describe_the_level() {
        let me = antigravity_server_name("0123abcd");
        assert_eq!(me, "ondin0123abcd");
        let cur = r#"{"permissions":{"allow":["mcp(ondin0123abcd/set_fills)","mcp(ondin/set_fills)","mcp(mine/x)"]},"trustedWorkspaces":["a"]}"#;
        let rules = antigravity_allow_rules(&me, &["get_node".into()]);
        let SettingsEdit::Write(next) = antigravity_settings_with_only(cur, &me, &rules).unwrap()
        else {
            panic!("should write");
        };
        let v: serde_json::Value = serde_json::from_str(&next).unwrap();
        assert_eq!(
            v.pointer("/permissions/allow").unwrap(),
            &serde_json::json!([
                "mcp(ondin/set_fills)",
                "mcp(mine/x)",
                "mcp(ondin0123abcd/get_node)"
            ])
        );
        assert_eq!(v["trustedWorkspaces"], serde_json::json!(["a"]));
        assert_eq!(
            antigravity_settings_with_only(&next, &me, &rules),
            Some(SettingsEdit::Unchanged)
        );
        assert_eq!(antigravity_settings_with_only("[1]", &me, &rules), None);
    }

    /// Withdrawing prunes only what it emptied, and writes nothing when there
    /// was nothing of this session's.
    #[test]
    fn antigravity_withdraw_prunes_only_what_it_emptied() {
        let me = "ondin0123abcd";
        let SettingsEdit::Write(next) = antigravity_settings_without(
            r#"{"permissions":{"allow":["mcp(ondin0123abcd/get_node)"]}}"#,
            me,
        )
        .unwrap() else {
            panic!("should write");
        };
        assert_eq!(next.trim(), "{}");
        for theirs in [
            r#"{"permissions":{"allow":[]}}"#,
            r#"{"permissions":{"allow":["mcp(ondin/get_node)"]}}"#,
            "",
        ] {
            assert_eq!(
                antigravity_settings_without(theirs, me),
                Some(SettingsEdit::Unchanged)
            );
        }
    }

    /// A session's server name is recognised; the person's `ondin` and other
    /// names are not.
    #[test]
    fn a_session_server_name_is_ours_alone() {
        assert!(is_antigravity_session_server(&antigravity_server_name(
            "deadbeef00"
        )));
        for theirs in [
            "ondin",
            "ondinDEADBEEF",
            "ondin-deadbeef",
            "ondindeadbee",
            "fs",
        ] {
            assert!(!is_antigravity_session_server(theirs), "{theirs}");
        }
    }

    /// Every sentence names a harness, says where foreign servers reach the
    /// session, and only `Restricted` outside Windows calls the level a rail.
    #[test]
    fn seal_sentences_say_what_they_must() {
        let s = seal_sentence(Harness::Antigravity, Seal::Restricted);
        assert!(s.contains("cannot keep them away"));
        assert_eq!(s.contains("safety rail"), !cfg!(windows));
        let s = seal_sentence(Harness::Claude, Seal::Sealed);
        assert!(!s.contains("safety rail") && !s.contains("keep them away"));
        assert!(seal_sentence(Harness::Codex, Seal::Unknown).contains("will not start"));
    }

    /// Claude's turn line carries images before the text, as base64 PNG blocks.
    #[test]
    fn claude_turn_line_carries_images() {
        let line = Harness::Claude.turn_line("hi", &[vec![1, 2, 3]]);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v.pointer("/message/content/0/type").unwrap(), "image");
        assert_eq!(v.pointer("/message/content/0/source/data").unwrap(), "AQID");
        assert_eq!(v.pointer("/message/content/1/text").unwrap(), "hi");
        let agy = Harness::Antigravity.turn_line("hi", &[]);
        assert_eq!(
            agy,
            "{\"event\":\"user\",\"message\":{\"content\":\"hi\"}}\n"
        );
    }
}
