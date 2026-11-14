# The assistant (ai.md §1–§7)

The assistant lets a person ask a coding agent about the design in front of them: what a layer is
doing, why a layout breaks at a width, how to name a set of variants, what to change to fix a
contrast failure. They ask in a panel at the tool rail's foot, attach layers from the canvas, and
read short answers that can point back at layers and propose edits. The harness layer is
`crates/ondin-ai`; the window's side is `ondin-app`'s `assistant` module and `mcp::{access,
propose, agent_log}`. What is still unbuilt (Command Mode's `/ai`) is in `docs/ai.md`.

## 1. The thesis

**Ondin embeds no model and holds no provider key.** The person already pays for, and is signed in
to, a headless coding agent. Ondin starts that agent as a child process, points it at Ondin's own
MCP endpoint, and shows its turn in the assistant panel. The harness owns the model, the account,
the billing and the rate limits; Ondin owns what the agent can see and what it can change.

- **No keys in Ondin.** Sign-in is always the harness's own. Settings holds no secret, and nothing
  goes into the keychain for the assistant.
- **One way in.** The agent reads and writes through the same MCP tools an outside client gets, so
  its edits are ordinary transactions on `mcp.md` §8.3's queue, undoable like any other
  (invariant 10).
- **Ondin seals the agent.** A coding agent arrives with a shell, a file editor and a web fetcher.
  Ondin starts every harness with its built-in tools off and only Ondin's MCP tools allowed (§4).

## 2. Harnesses

A **harness** (`ondin_ai::Harness`) names a wire dialect, not a vendor: a fork that speaks its
parent's stream counts as the parent. Each has a persisted key, a label (*Claude Code*), a program
name (`claude`) and a speaker name (*Claude*) shown on its replies. An unknown saved key
(`Harness::from_key`) is no harness and reads as *Off*.

| Harness | Program | Shape | Resume | Stop |
| --- | --- | --- | --- | --- |
| Claude Code | `claude` | one process per conversation, turns as NDJSON on stdin | the process holds the thread; `--resume <id>` after a kill | interrupt control request |
| Codex | `codex` | one process per turn, prompt on stdin | `exec resume <id>` | kill |
| Antigravity | `agy` | one process per conversation, NDJSON on stdin | `--conversation <id>` after a kill | kill |
| OpenCode | `opencode` | one process per turn, prompt as the last argument | `--session <id>` | kill |
| GitHub Copilot | `copilot` | one process per turn, prompt on stdin | `--resume=<id>` | kill |
| Cursor | `cursor-agent` | one process per turn, prompt on stdin | `--resume <id>` | kill |
| DeepSeek Harness | `dsh` | one process per turn, prompt on stdin | `--session-id <id>` | kill |

- **Behaviour is asked as a capability, never by identity**: persistent or per-turn, prompt on
  stdin, system prompt in the first turn, interrupt, effort levels, images, how it is sealed. Each
  capability is an exhaustive `match`, so a new harness is a compile error, not a quiet default.
- **Every harness streams JSON lines**, and no flag or stream shape is driven that was not measured
  against the real program. DeepSeek Harness's `--json` sends a step at a time rather than text
  deltas.
- **Images reach Claude Code and Codex only**: Claude Code as content blocks in the turn's line,
  Codex as `--image` files. `Harness::takes_images` is false for the rest, which get no render.
- **DeepSeek Harness takes almost nothing as a flag.** A `--patch` overlay written into the
  session's own `DSH_HOME` carries the model, the effort, the seal (every built-in tool row
  disabled by id), the sign-in and Ondin's server. The overlay is checked with `--dump-config`
  before every turn, and a row left live refuses the turn. The sign-in is the person's: their dsh
  account, read from their own credentials file by path and never copied, or a `DEEPSEEK_API_KEY`
  already in their environment, passed through and never stored.
- **Copilot, Cursor and DeepSeek Harness run with a home in the session's run folder**, so their
  own configuration stays out; it goes with the session, so a resume holds within a session and
  never across one. Copilot's login file is copied in, since a home without a credential store
  would be signed out.
- **Cursor needs its server approved** (`cursor-agent mcp enable`) before the first turn; a failed
  approval refuses the turn.
- **OpenCode reads its configuration from the session**: `XDG_CONFIG_HOME` at the session's
  `opencode.json`, project config off, and `OPENCODE_TEST_HOME` at an empty folder, since
  `~/.opencode/opencode.json` is merged over everything and no option turns it off. Because it
  merges from places Ondin cannot list, `opencode debug config` is read back before the first turn,
  and another enabled server, or a tool or permission the sealed agent allows, refuses the session.
- **Antigravity is configured globally**: its server is registered in the person's own registry and
  its allow rules written into its `settings.json`, both installed with the session and withdrawn
  when it ends. Each session registers under a name of its own, `ondin` and eight hex digits
  (`antigravity_server_name`), and grants and withdraws rules for that name only, so it never adds,
  replaces or removes a registration or rule named `ondin` the person made. Its claim marker, named
  after it and created exclusively, records the owning Ondin and the `agy` path it registered with,
  and is removed only once the withdrawal held. While a session holds them, an `agy` the person
  runs finds Ondin's server registered and its tools pre-approved; no option scopes either to one
  process.
- **Each harness is driven through its own headless mode, not the Agent Client Protocol.** ACP
  would turn the dialects into one, but Claude Code and Codex speak it only through adapter
  packages, which the person would have to install and which may not run the program they signed in
  to. Parked until Claude Code speaks ACP itself.

### Finding and probing the program

- **Lookup order:** a path set in Settings wins outright; else an `ONDIN_<HARNESS>_BIN` variable,
  then each harness's known install folders (CLIs are often installed off the inherited `PATH`),
  then `PATH` with `PATHEXT`.
- **On Windows** npm's `.cmd` shims come last, since Rust refuses a multi-line argument to a batch
  file. Behind a shim, the real program is started directly. Every child is started with
  `CREATE_NO_WINDOW` and `NoDefaultCurrentDirectoryInExePath=1`, in a Job Object of its own nested
  in the session's, so *Stop* and a grace kill end the harness's whole process tree.
- **A helper** (the `--help` probe, `agy mcp …`, `cursor-agent mcp enable`) runs in a private empty
  folder and is ended, with its tree, past a deadline: 15 seconds for the probe, 30 for `agy`
  (`process::run_bounded`).
- **Ondin never installs a harness.** A missing one is said in the panel with the program it
  looked for.
- **A probe reads `--help` once per program path** and caches what it finds: which sealing flags
  exist, and so how sealed a session can be (§4). A probe that could not run is not cached.
  `ondin_ai::warm` runs it in the background only while the assistant is on.
- **`ondin_ai::sweep` runs at every launch** on a background thread (`assistant::at_launch`):
  leftovers carry their owner's pid and start time, and only those whose owner is gone are removed,
  Antigravity's registration and rules included.

### Model and effort

- **The model is free text**, passed verbatim; empty omits the flag. Each harness offers a few
  suggestions (`Harness::model_suggestions`) in the field's list and ghost, never as the permitted
  set.
- **Effort uses each harness's own words** (`Harness::effort_levels`), and a level not on its list
  is never sent. Codex, Copilot and Cursor have none, so the Effort row is hidden: Copilot's flag
  fails the turn on its default `auto` model, and Cursor's effort is part of the model id.

## 3. A turn

- **Events** (`ondin_ai::Event`): text deltas, tool calls, tool results (paired to their call by the
  harness's own id, never by recency, which mis-pairs parallel calls), the end of the turn with its
  duration and token counts, the harness exiting first, a refusal, and *Stopped*. Thinking is not
  shown.
- **The parser is stateful per harness:** it turns restated text into deltas, drops restated tool
  calls, and resets at the end of each turn. A line that is not JSON is kept as a diagnostic, and
  so is Codex's `error` while it reconnects: only `turn.failed` ends its turn as failed.
- **Stop** sends Claude Code its interrupt and kills every other harness's child with its whole
  tree (its own process group off Windows). An interrupt that
  has not ended the turn in ten seconds kills the process, and the next turn respawns it with
  `--resume <session id>`. The turn ends only when the harness reports `Event::Stopped`; until then
  the send button reads *Stopping…*, disabled, and then a note says *Stopped. The next question
  resumes the session.*
- **A harness that exits before answering drops the session**: the next question starts a new one,
  with the thread's prose replayed as a restored conversation is.
- **A stale stream is discarded.** Every event is tagged with its turn and dropped once a newer turn
  has begun, and a session the person has left (switched document, changed settings) is dropped
  whole.
- **The working folder** is a fresh, exclusive, owner-only folder under the machine data folder's
  `run/` for each session, never the document's folder and never the app's working directory, so
  no planted agent settings file beside a document can configure the child. A session that cannot
  make one is refused.
- **The MCP server** in the harness's config is `ondin mcp-proxy --client <key> --document <path>`.
  The key is the session's, so a harness that starts a process per turn finds its document and
  revisions on its next connection, with `changed` reporting what the person did in between
  (`mcp.md`, *Revisions*).
- **The environment** marks the child (`ONDIN_AGENT_CHILD=1`) and clears the variables that would
  point a harness at another configuration or reopen its seal (`OPENCODE_PERMISSION` among them).
  Under the mark the `ondin` command line refuses everything but `mcp-proxy`, `serve` included, and
  the proxy connects only to the window, never starting `serve`: the window holds the chat's grant
  and `serve` none, so an agent there would act unchecked. A speed bump rather than a boundary.
  Codex and Cursor start an MCP server without the agent's environment, so their server config
  sets the mark itself, and the proxy also counts a chat's `--client` key (`assistant-…`) as the
  mark. Every child's `PWD` is its own working folder: OpenCode's runtime reads an inherited one
  as its working folder, which would put it in the shell's folder Ondin was started from.
- **The window must have its endpoint.** A chat refuses to start when the window could not open
  its MCP endpoint, since the chat's level lives in the window's grants.
- **A command line too long** for the platform is refused before spawning, naming the limit and
  what made it long (the system prompt riding OpenCode's first message, say).

## 4. What the agent can see and do

### Sealing

Each harness is started with its built-in tools off, the person's other MCP servers excluded where
the harness allows it, and only Ondin's tools allowed. The mechanism differs per harness (flags, a
sandbox, a rules or permissions file, a config overlay), and each **fails closed**: a harness whose
restricting config cannot be written, or whose probe cannot confirm its restricting options, is
refused, not started open. The seal is graded — *Sealed* where built-in tools are off, *Restricted*
where a sandbox or rules file holds them — and the panel's head shows its sentence
(`seal_sentence`), naming the mechanism, saying where a harness cannot keep the person's own MCP
servers away (Antigravity, Cursor), and, at *Restricted* outside Windows, that the access level is
a rail rather than a boundary.

Never passed, whatever a harness offers: any skip-permissions, auto-approve, allow-all or yolo
flag. A headless harness cannot ask the person mid-turn, so its allow-list is exactly the tools the
access level lists and everything else is denied.

### Access levels

**Access** (`mcp::access::Level`) decides what the agent may do. Settings holds the default for a
new chat, and the panel's head switches it for the chat in front. A switch ends the session, and a
running turn with a note (*Stopped: the access level changed.*); the next question starts a new
session with the prose replayed. A chat on the default follows it: saving another default in
Settings ends its session, since its grant was given at the old level. Tools above the level are **left out of the tool list**, not
listed and refused: an agent that sees a tool plans around it.

- **Look** — Phase 1's reads and exports (`mcp.md` §8.4). `export_png` is the agent's eyes.
- **Propose** (the default) — Look, plus `propose_edit` (*Proposals*).
- **Edit** — Look, plus the writes of Phases 2 and 3. Each call commits as its own undo step marked
  as the assistant's (`Origin::Mcp`). Edit does not list `propose_edit`.

Every level lists the core set within it (`access::listed`) plus `describe_tools` and `call`, and
reaches its other tools through `call`. Phase 4, the kits and the library, is never a chat's.
`propose_edit` sits outside the phases, in `ondin_mcp::tools::ASSISTANT`.

**The host holds the level, not the harness.** The endpoint serves any process running as the
person, and a harness with a shell can start `ondin mcp-proxy` itself or open the pipe or socket
directly. So the window, which starts the harness, grants each session's level to the MCP host
(`access::Grants`) and the host enforces it on every call (`Engine::handle`):

- **By key.** A call carrying a session's key acts at its level; a tool above it is refused, by
  name, through `call` and as a `batch` step (`access::refused`), even though it was never listed.
- **By process.** The endpoint reads the connecting process at accept (`GetNamedPipeClientProcessId`
  on Windows, `SO_PEERCRED` on Linux, `LOCAL_PEERPID` on macOS, none on other Unixes) and every
  call carries it (`Call::pid`). A process the session owns acts under its key and level whatever
  key it sends. On Windows ownership is the session's Job Object, which every process it starts
  joins however it is launched; elsewhere the parent chain is walked, which a process that left its
  tree (`setsid`, a double fork) escapes.
- The window grants the key before spawning and again with the session's process owner once it
  starts. Ending the session revokes the grant at once, on the UI thread, and finishes the rest
  (withdrawing Antigravity's registration, a bounded wait) off it. A revoked grant is kept as
  *ended*, not removed, so a call still arriving from the chat is refused (*This chat has ended:
  nothing was changed.*) rather than served as an ungranted client's; the last 32 are kept.
- A granted call is written to the agent log (§7).

Left alone: `describe_tools` still describes tools above a chat's level, and a client no grant
covers is an ordinary MCP client and can call `propose_edit` too.

At the *Sealed* grade the agent has no shell, so it cannot reach the endpoint any other way. At
*Restricted*, *Look* and *Propose* are only as strong as the sandbox outside Windows: a safety rail
against an eager model, not a boundary against a hostile one. The seal's sentence says so.

### Proposals

- **`propose_edit(title, calls)`** takes `calls` as `{verb, args}`, each a registry verb of Phases 2
  or 3. They are checked by running them on a copy of the document (`mcp::propose::check`), so the
  person's document, its revision and their redo stay as they were. The answer is
  `{proposal, changes}`: the id, and what each call changes in its verb's own words. A refusal
  names the call and proposes nothing.
- **The host keeps the last 64 proposals in memory.** A tool row of `propose_edit` renders as its
  card; one the host no longer holds (after a restart) says *This proposal is no longer held; ask
  again to see it.*
- **The card** heads with the sparkle, the title and the count of changes, lists the changes, and
  ends in one of:
  - **Apply** — runs the calls again on the document as it is, as one undo step named by the title
    and marked as the assistant's (`Origin::Mcp`): the person applies it, the agent wrote it. It
    is refused, with nothing changed, when the calls now reach a layer the card did not list,
    other than ones they create (a `replace_text` with no `nodes` meeting a layer added since):
    *The proposal now also changes `<name>`, which it did not list.* A refused Apply keeps the
    person's redo (`propose::apply`).
  - **`<layer> changed after this was proposed`**, in the warning colour, with *Ask again*, which
    asks *`<name>` changed. Propose again.* with the proposal's written layers that still exist
    attached.
  - **Applied as one step**, with *Undo* while that step is the last (`propose::is_last`).
  - **Made in another document**.

### Context sent with a turn

- **The system prompt** (`assistant::prompt::system`) says what Ondin is, the access level and its
  tools truthfully, how to propose an edit, and how to answer, then the person's own instructions
  from Settings, which come last and so win.
- **Answers are short**, since the panel is narrow: one or two sentences, the answer first, no
  preamble, recap or offer of more. A choice is at most three options in an ` ```options ` block,
  one short line each, and a question. A layer is named by its id, so it renders as a link. A table
  or code only when asked for one or when the answer is code.
- **The prompt goes in the harness's system-prompt flag** where it has one, so it holds on every
  turn and survives the harness compacting its context; otherwise at the top of the conversation's
  first turn. Claude Code takes it as a file (`--append-system-prompt-file`, falling back to
  `--append-system-prompt` where its probe lacks the flag), keeping it and the replayed
  conversation off the length-limited, world-readable command line. OpenCode has no file form and
  takes its prompt as the last argument, so a long one is refused as too long.
- **Each turn opens with a preamble**, fenced as data: the revision; the layers added, changed and
  removed since the last turn, by id and name (forty at most, then how many more); the page; the
  selection; the attached layers; the person's last three questions, since a harness's own memory
  of earlier turns is unreliable; and `style: brief`, since an instruction given once fades over a
  long thread. The page outline, names and ids only, goes only on a session's first turn. A name
  in the preamble is written on one line with `<` and `>` as `‹` and `›`, so no layer or page name can
  close the `<ondin-turn>` block and write words the model would read as the person's.
- **The design is never sent whole.** The agent's earlier reads stay in its session, and the
  preamble says which went stale. Tool results' `changed` cannot do this alone: an agent answering
  from memory calls no tool.
- **A restored conversation** replays its prose, never its tool calls, into a fresh session's
  system prompt; a thread another harness wrote is not replayed, and the chat says so.
- **Document content is data, never instructions.** Layer names, text content, component
  descriptions and variable names may come from someone else's file or kit; the system prompt and
  the MCP server's instructions say so.

### Attaching layers

- **Two doors attach, and only two**: *Ask the assistant*, the layer menu's last group
  (`menu::Group::Assistant`, `context-menus.md` §3), and the ask button on the selection. Each
  attaches the selection as **chips** above the composer, opens the panel and focuses the composer.
  The rail button, the palette's *Assistant* and `Ctrl+Shift+A` open the panel and attach nothing.
  The menu row is omitted while the assistant is *Off*, as the ask button is hidden, so someone who
  never turns it on sees neither; in private mode it is dimmed.
- **The ask button** is a small `icon::SPARKLE` button above the selection's top-right corner, its
  right edge on the selection's and its bottom 8 clear of it, one for the whole selection and the
  same size at every zoom. Where there is no room above it sits inside the corner, and it stays left
  of the inspector's cards. *View ▸ Show ask assistant* (`ViewSwitch::AskAssistant`, last of the
  View menu's canvas group; per machine, on by default) hides it; the context menu's row stays. The
  canvas context menu's view rows leave it out.
- **An attached layer has its own ask button, lit**, in the same place over that layer, carrying the
  sparkle and its chip's number, whatever *Show ask assistant* says. It shows while the panel is
  open or the layer is selected, on its own page; the selection's button gives way to it once every
  selected layer is attached. A click on it detaches the layer: the button is a toggle.
- **The ask buttons hide** while the selection is dragged, resized or rotated, while text or a path
  is edited, in Present, focus and private mode, and while the assistant is *Off*. They pass the
  wheel and a pinch to the canvas beneath, as its other overlays do.
- **A chip is a reference, not a copy** (`ui::layer_chip`): the layer's id, name, kind and size go
  with the question, and the agent reads the rest through the tools. For a harness that takes
  images, an attached layer also goes with a 1× render. A chip's × removes it; a chip whose layer
  is deleted shows struck through and is not sent.
- **While the panel is open, an attached layer wears a dashed accent outline** 5 outside it; its
  number is on its lit ask button. A closed panel leaves no outline. Hovering a chip outlines its
  layer; clicking one selects it and zooms to it.
- **The agent's reply can name layers.** A layer id in a reply the document holds renders as a
  link naming the layer, in the accent; hovering it outlines the layer and clicking it selects it
  and zooms to it, as a chip does.

## 5. Settings

An **Assistant** tab in Settings, after *Assets* (`app-canvas-chrome.md`, *Settings*), staged
and written on *Save changes* as the rest is. The rows, then an **Instructions** eyebrow over the text area. Stored in prefs (`AssistantPrefs`), per machine; none
of it is a secret.

- **Agent** — a dropdown, *Off* first. A new install starts off. An unknown saved key loads as off
  and is kept until the person picks.
- **Path** — the program's path, empty for found automatically. Settings looks it up for the agent
  and path on screen, saved or not, with its own lookup beside the panel's: a spinner after the
  label while it looks, then *Found …* in the accent ink or *Not found* in the error's. A typed
  path is probed only once it has stood unchanged for 700 ms (`TYPING_QUIET`), since the probe
  runs whatever the half-typed name names.
- **Model** — free text in a `ui::suggest_field` (*Its default* as the hint, *Suggested models*
  on the chevron): the harness's suggestions in its list and as a ghost `Tab` fills.
- **Effort** — the harness's own levels; hidden where it has none.
- **Access** — *Look*, *Propose*, *Edit*: the default for a new chat. Each cell's tooltip says
  what the level lets the agent do, in the panel's level menu's words.
- **Instructions** — free text added to every system prompt (`ui::text_area`), held at its six
  rows and scrolling past them, so the modal keeps its size.

With *Off*, a caption says *Off: nothing is started and nothing is sent…* and the rows below Agent
are disabled. Switching agent clears Path and Model and moves Effort to the nearest level the
new harness has, ties downward; each harness's last values come back within the session. A saved
change ends the session only when it changes what the agent was started with.

## 6. The panel

A panel at the rail's foot rather than a floating window, which has to be placed, ends up over the
layers being asked about, and is one more kind of surface.

- **The door** is the rail's last button, under *Insert components*, wearing `icon::SPARKLE`, History's mark
  for a step made over MCP: the same concept, an agent acting. (`CHAT` stays unused: a speech
  bubble reads as comments, a non-goal.) Its tooltip is *Assistant (Ctrl+Shift+A)*. The palette's
  *Assistant* row and the chord open it too.
- **It is a `ui::rail_popup`, taller than its siblings.** Its bottom-left is theirs
  (`OndinApp::rail_popup_anchor`); its top stops 50 below the canvas's top, leaving the top of the page in view, down to a floor that
  still holds the head, one line of thread and the composer. It is 400 wide, and its right edge
  drags between 360 and 640, the only rail panel with a drag edge, since tables and code need the
  room; the width is kept per machine. The edge lights as the layers panel's does: a hairline over
  the card's border, after the same dwell (`EdgeDwell`), the full accent while dragged.
- **It shares the place**: opening a sibling closes it, and opening it closes them. Closing keeps
  the thread, the draft and a running turn. While a turn runs out of sight the button wears an
  accent ring; a reply that arrived unseen wears an accent dot until the panel opens; the tooltip
  says which (*Assistant · reply waiting*).
- **The canvas stays live** behind it. `Escape` closes it. A panel switch, a page switch or
  `Escape` never stops a turn; a document switch, a settings change, turning the assistant off and
  private mode end the session, each with a note.
- **Private mode turns the assistant off** while it is on. The panel closes and the session ends;
  the rail button and the context menu's row are disabled with *Not available in private mode*;
  `Ctrl+Shift+A` answers on the status line; the ask buttons and the outlines are not drawn. The thread
  and its chips are kept.
- **The head**: the harness's label, the seal's grade beside a lock, then the Access dropdown and
  *New chat* (a plus). *New chat* clears the thread at once, without asking, and ends a running
  turn.
- **The thread:** the person's turns right-aligned; the agent's full width, under the speaker name
  the turn was written by, in bold and kept with the turn. An empty thread shows the app's empty
  state saying how to attach layers. It scrolls with no bar.
- **A turn is never blank while it waits.** The agent's turn takes its place at the send, with
  `ui::paint_spinner`, *Starting Claude Code…* until the harness's first event, then *Thinking…*
  until the first text or tool call, and again once a tool has finished and no text has followed.
  The seconds count from the send. A harness that exits before answering leaves a note, *Claude
  Code stopped before answering (exit code 1).*, with *Ask again* and *Show details* (the
  diagnostics); a turn the harness ends as failed leaves *Claude Code ended the turn with an
  error.* the same way, whether or not it wrote a reason.
- **Replies render markdown**: paragraphs, lists, tables, code blocks with a language bar and
  *Copy*. A loose list, its items parted by blank lines, is one list, and an ordered one is
  numbered from its first number. A reply's text is parsed once, not every frame (an egui
  `FrameCache` keyed by the text). A proposal renders as its card (§4).
- **An ` ```options ` block renders as a stack of `ui::field_button`s**; a click sends its line as
  the next question, with the same chips. Only the last turn's options are live.
- **A tool call is a row**, 22 tall: its name in mono, its argument, a status dot and its result
  cut short. After a turn's third call the rest fold into *+N calls*.
- **Under each finished turn:** its time and tokens, *Copy*, and on the last turn *Regenerate*
  (`icon::ARROW_CLOCKWISE`), which drops that turn and asks again.
- **The composer** is a bare `ui::text_area` in a field ground with the chips above it and the
  hint and send button on its foot, all on one left edge; it grows to 15 lines, then scrolls.
  Enter sends, Shift+Enter breaks the line, and the send button becomes *Stop* (`icon::STOP`)
  while a turn runs. `Ctrl+↑`/`Ctrl+↓` step through earlier
  questions.
- **When it cannot send**, the composer is replaced by the reason, and off wins over every other:
  the assistant is off (*Open settings*); private mode; the document is unsaved (*Save the document
  to ask about it.*); not signed in, saying to run the program once in a terminal; the program not
  found, naming what was looked for and where (*Set the path*); the program not sealable.
- **The app's own words** (a refused spawn, a stopped turn, a replay withheld) are notes beside a
  turn, never message text, so they are never sent back to the model.
- **One conversation per document**, kept per machine in `<machine data>/chats/<document id>.json`
  (by a hash of the path when the id is not a plain name), never in the `.ondin` file: a chat is the
  person's, not the work's. It is written through `atomic::write`; when the document's id or path
  changes, the old file is removed only once the new one is written. At most 100 messages, cut
  from the front to start at a question, with each *Ask again* re-pointed at its question's new
  place (one whose question was cut keeps its words and loses the button); tool results are saved
  as *(result not saved)*, and a turn still streaming is not saved.

## 7. Logging and privacy

- **Off sends nothing and starts no agent**, the probe included. A launch still sweeps what a
  closed Ondin's sessions left, which can run `agy mcp` to withdraw a dead session's own
  registration (its `ondin` and eight hex digits) from the person's Antigravity config, with the
  `agy` its marker recorded, else the one found now: a standing grant there is worse than one
  command. A marker whose withdrawal did not hold stays for the next launch.
- **What leaves the machine** is what the harness sends: the question, the attached layers'
  references, and whatever the agent reads through the tools. No document file is handed over, and
  no telemetry is sent by Ondin.
- **The agent log** (`mcp::agent_log`) is `agent_log.jsonl` in the machine data folder: one line
  `{at, tool, document, key, ok}` per granted call, refusals included, a call through `call` under
  the tool it named. It never records results. It rolls to `agent_log.jsonl.1` at 8 MiB; always on;
  never read back by the app.

## Traps

- **The level is granted twice, and both are needed.** The key grant before spawning covers the
  harness's proxy connecting before `Session::start` returns; the grant with the session's process
  owner covers a process that sends another key or none
  (`an_owned_process_gets_the_chats_level_whatever_its_key`).
- **An agent is kept off `serve` twice.** The proxy under `ONDIN_AGENT_CHILD` or a chat's key
  connects only to the window (`proxy::Machine::connect`, `pick_host`), and `main` refuses
  `ondin serve` under the mark, for an agent with a shell. Each looks redundant beside the other;
  `serve` holds no grant, so either gone lets an agent act unchecked. The key check is not
  redundant with the mark either: Codex and Cursor scrub their servers' environment, and only
  their config's `env` puts the mark back.
- **Copilot's `--available-tools` with an empty value fails open** (every tool, the shell
  included), so an empty grant names a tool nothing provides
  (`copilot_never_sends_an_empty_allowlist`).
- **Antigravity has no interrupt line**: it ignores an unknown stdin event silently, so a guessed
  interrupt would hang the turn rather than fail. It is killed and resumed with `--conversation`.
- **Antigravity's seal is `--mode plan`, not `--sandbox`.** The sandbox restricts its terminal
  only; in a folder the person trusts (their home is a common entry) its own `write_file` and
  `run_command` still act, which was measured writing a file outside the run folder. Plan mode
  refuses both and leaves Ondin's allowed tools working. Headless, a refused tool ends its turn,
  so the system prompt tells every agent to call Ondin's tools directly rather than look around.
- **Cursor runs in its default mode.** `--mode ask` held back nothing the permissions file did not,
  and had the model refuse the edits a chat at *Edit* asked for.
- **DeepSeek Harness grows tool plugins.** The per-turn check refuses a profile with a tool row
  the overlay does not disable (`tool-agent-team` arrived this way); the row then joins
  `DSH_TOOL_ROWS`.
