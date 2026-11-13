# Ondin — the assistant

> **Status: design, not built.** Nothing here exists in code. It depends on `ondin-mcp` serving
> at least its Phase 1 read tools (`design/mcp.md` §8.4). When a part is built, its rules move
> into the design file the code lives under, and this file keeps only what is still unbuilt.

The assistant lets a person ask a coding agent about the design in front of them: what a layer is
doing, why a layout breaks at a width, how to name a set of variants, what to change to fix a
contrast failure. They ask in a panel at the tool rail's foot, attach layers from the canvas, and
read short answers that can point back at layers and propose edits. Command Mode's `/ai` (`vm.md` §11) is a later door
into the same assistant.

## 1. The thesis

**Ondin embeds no model and holds no provider key.** The person already pays for, and is signed in
to, a headless coding agent — Claude Code, Codex and their kin. Ondin starts that agent as a
child process, points it at Ondin's own MCP endpoint, and shows its turn in the assistant panel. The
harness owns the model, the account, the billing and the rate limits; Ondin owns what the agent
can see and what it can change.

That split decides the rest:

- **No keys in Ondin.** Sign-in is always the harness's own. Settings holds no secret, and nothing
  goes into the keychain for the assistant.
- **One way in.** The agent reads and writes through the same MCP tools an outside client gets, so
  its edits are ordinary transactions on §8.3's op queue, undoable like any other (invariant 10).
- **Ondin seals the agent.** A coding agent arrives with a shell, a file editor and a web fetcher.
  None of those belong in a design tool's chat. Ondin starts every harness with its built-in tools
  off and only Ondin's MCP tools allowed (§4).

## 2. Harnesses

A **harness** names a wire dialect, not a vendor: a fork that speaks its parent's output format
counts as the parent. Each has a key (persisted), a label (*Claude Code*), a program name
(`claude`) and a speaker name (*Claude*) shown on its replies.

| Harness | Program | Shape | Resume | Stop |
| --- | --- | --- | --- | --- |
| Claude Code | `claude` | one process per conversation, turns as NDJSON on stdin | the process holds the thread | interrupt control request |
| Codex | `codex` | one process per turn, prompt on stdin | `exec resume <id>` | kill |
| Antigravity | `agy` | one process per conversation, NDJSON on stdin | `--conversation <id>` after a kill | kill |
| OpenCode | `opencode` | one process per turn, prompt as the last argument | `--session <id>` | kill |
| GitHub Copilot | `copilot` | one process per turn, prompt on stdin | `--resume=<id>` | kill |
| Cursor | `cursor-agent` | one process per turn, prompt on stdin | `--resume <id>` | kill |
| Gemini CLI | `gemini` | one process per turn, prompt on stdin | `--resume <id>` | kill |
| DeepSeek Harness | `dsh` | one process per turn, prompt on stdin | `--session-id <id>` | kill |

- **Behaviour is asked as a capability, never by identity**: persistent or per-turn, resume,
  interrupt, prompt on stdin, system prompt in the first turn, effort levels, how it is sealed.
  Each capability is an exhaustive `match`, so a new harness is a compile error, not a quiet
  default.
- **Every harness streams JSON lines**: stream-json for Claude, Antigravity, Cursor and Gemini,
  the `--json` / `--format json` forms for the rest. No protocol that has not been measured against
  a real binary is driven; Gemini CLI's row is still to be measured. DeepSeek Harness's `--json`
  sends a step at a time rather than text deltas.
- **DeepSeek Harness takes almost nothing as a flag.** Its headless profile has no model or
  effort flag, so a `--patch` overlay written into the session's own `DSH_HOME` carries the
  model, the effort, the seal (every built-in tool row disabled by id), the sign-in and Ondin's
  server. The overlay is checked with `--dump-config` before every turn, and a row left live
  refuses the turn. The sign-in is the person's: their dsh account, read from their own
  credentials file by path and never copied, or a `DEEPSEEK_API_KEY` already in their
  environment, which Ondin passes through and never stores.
- **Gemini CLI signs in with Google once.** Running `gemini` in a terminal and choosing *Sign in
  with Google* caches the credentials, and its headless runs reuse them, on the person's own
  account and quota. No API key is involved, so it fits the thesis. A headless run with nothing
  cached fails as not signed in.
- **Each harness is driven through its own headless mode, not the Agent Client Protocol.** ACP
  would turn six dialects into one and lets an agent ask permission mid-turn, but Claude Code and
  Codex speak it only through adapter packages (npm, so Node too). The person would have to
  install one the harness does not need, or Ondin would install it, and an adapter may not run
  the program the person signed in to. Parked until Claude Code speaks ACP itself.

### Finding and probing the program

- **Lookup order:** an `ONDIN_<HARNESS>_BIN` variable, then each harness's known install folders
  (CLIs are often installed off the inherited `PATH`), then `PATH` with `PATHEXT`. A path set in
  Settings overrides all three.
- **On Windows** npm's `.cmd` shims come last, since Rust refuses a multi-line argument to a batch
  file. Behind a shim, the real program is started directly, so *Stop* kills the agent and not
  only its `cmd.exe`. Every child is started with `CREATE_NO_WINDOW`.
- **Ondin never installs a harness.** A missing one is said in the panel with the program it
  looked for.
- **A probe reads `--help` once per program path** and caches what it finds: which sealing flags
  exist, and so how sealed a session can be (§4). A probe that could not run is not cached. The
  probe warms in the background, and never while the assistant is off.

### Model and effort

- **The model is free text**, passed verbatim; empty omits the flag. Each harness offers a few
  suggestions as chips, never as the permitted set: model names change faster than releases.
- **Effort uses each harness's own words** and is clamped to what it advertised, so a level is
  never sent to a harness that lacks it. The Effort row is hidden for a harness with none.

## 3. A turn

- **Events:** text deltas, tool calls, tool results (paired to their call by the harness's own id,
  never by recency, which mis-pairs parallel calls), the session id, and the end of the turn with
  its duration and token counts. Thinking is not shown.
- **The parser is stateful per harness:** it turns restated text into deltas, drops restated tool
  calls, and resets at the end of each turn. A line that is not JSON is kept as a diagnostic.
- **Stop** sends Claude Code its interrupt and kills every other harness; the next turn resumes
  by id. A child gets ten seconds to exit before it is killed.
- **A stale stream is discarded.** Each session has a monotonic id, and output from a session the
  person has left (switched document, changed settings) never lands in the current thread.
- **The working folder** is a fresh, exclusive, owner-only folder under the machine data folder's
  `run/` for each session, never the document's folder and never the app's working directory, so
  no planted agent settings file beside a document can configure the child. A session that
  cannot make one is refused.
- **The MCP server** in the harness's config is `ondin mcp-proxy --client <key> --document <path>`.
  The key is the conversation's, so a harness that starts a process per turn finds its document
  and revisions on its next connection, with `changed` reporting what the person did in between
  (`design/mcp.md`, *Revisions*). The document spares the agent a `use_document` call.
- **The environment** marks the child (`ONDIN_AGENT_CHILD=1`; the `ondin` command line refuses to
  run under it, a speed bump rather than a boundary) and clears the variables that would point a
  harness at another configuration.
- **A command line too long** for the platform is refused before spawning, naming the limit.
- **Leftovers** (run folders, config files) carry their owner's pid and start time, and a sweep at
  launch removes only those whose owner is gone.

## 4. What the agent can see and do

### Sealing

Each harness is started with its built-in tools off, its own and the person's other MCP servers
excluded where the harness allows it, and only Ondin's tools allowed. The mechanism differs per
harness (flags, a sandbox mode, a rules file in the run folder), and each is chosen to **fail
closed**: a harness whose restricting config cannot be written is refused, not started open. The
seal is graded — *Sealed* where built-in tools are off, *Restricted* where a sandbox or rules file
holds them — and the panel's head and Settings say which, naming the mechanism, including where a
harness cannot keep the person's own MCP servers away, and, at *Restricted* outside Windows, that
the access level is a rail rather than a boundary (*Access levels*).

Never passed, whatever a harness offers: any skip-permissions, auto-approve, allow-all or yolo
flag. A headless harness cannot ask the person mid-turn, so its allow-list is exactly the tools
the access level grants and everything else is denied.

### Access levels

**Access** decides what the agent may do. Settings holds the default for a new chat, and the
panel's head switches it for the chat in front: it changes from task to task, and *Edit* left on
by accident is worth a visible control. A switch restarts the session, replaying the thread's
prose as a restored conversation does. Tools above the level are **left out of the tool list**,
not listed and refused: an agent that sees a tool plans around it.

- **Look** — Phase 1's reads and exports (`design/mcp.md` §8.4). `export_png` is the agent's
  eyes.
- **Propose** (the default) — Look, plus `propose_edit`. The agent sends a list of registry verb
  calls (`ondin_core::verbs`), which Ondin checks as `verbs::call` checks any call and returns
  the result to the agent. The agent then shows the proposal in its reply, and Ondin draws it as
  a card: what it changes, in the verbs' own words, and **Apply**, which commits it as one
  transaction and one undo step named after the proposal, marked as the assistant's
  (`Origin::Mcp`) so History wears the sparkle on it: the person applies it, but the agent wrote
  it. Nothing changes until the person applies it. A proposal records the revision it was made at: when a layer it writes has changed
  since, the card says so in place of *Apply* (*Price changed after this was proposed*) with *Ask
  again*, which asks for a fresh proposal with that layer attached. Applied, the card offers
  *Undo* while its step is the last.
- **Edit** — Look, plus the write tools of Phases 2 and 3. Each call commits as its own undo step
  marked as the assistant's (`Origin::Mcp`), so the History section shows which steps the agent
  made. Kits and the library (Phase 4) stay out of the assistant at every level.

**The host holds the level, not the harness.** The endpoint serves any process running as the
person, and a harness with a shell can start `ondin mcp-proxy` itself or open the pipe or socket
directly. `ONDIN_AGENT_CHILD` does not stop it: the harness's own MCP server is that proxy. So the
level is enforced by the window, which starts the harness and knows each chat's client key and
level without any file:

- **By key.** A call carrying a chat's key gets that chat's tool list, and a tool above its level
  is refused even though it was never listed. This holds a harness that ignores its own
  allow-list.
- **By process.** On Windows the harness starts inside a Job Object, which every process it starts
  joins however it is launched. The endpoint checks a connecting process with `IsProcessInJob`
  and gives it that chat's level whatever key it sends, or none. Elsewhere a process can leave its
  harness's tree (`setsid`, a double fork), so the check walks the parent chain and is best
  effort.

At the *Sealed* grade the agent has no shell, so it cannot reach the endpoint any other way. At
*Restricted*, *Look* and *Propose* are only as strong as the sandbox outside Windows: a safety rail
against an eager model, not a boundary against a hostile one. The seal's sentence says so.

### Context sent with a turn

- **The system prompt** says what Ondin is, the access level and its tools truthfully, how to
  propose an edit, and how to answer, then the person's own instructions from Settings, which
  come last and so win.
- **Answers are short**, since the panel is narrow: one or two sentences, the answer first, no
  preamble, recap or offer of more. A choice is at most three options in an ` ```options ` block,
  one short line each, and a question. A layer is named by its id, so it renders as a chip. A
  table or code only when asked for one or when the answer is code; longer only when the person
  asks for detail.
- **The prompt goes in the harness's system-prompt flag** where it has one (Claude Code's
  `--append-system-prompt`), so it holds on every turn and survives the harness compacting its
  context; otherwise at the top of the first turn.
- **Each turn opens with a preamble**, fenced as data: the revision; the layers added, changed and
  removed since the last turn, by id and name (forty at most, then how many more and that the page
  should be read again); the page; the selection; the attached layers; the person's last three
  questions, since a harness's own memory of earlier turns is unreliable; and `style: brief`, since
  an instruction given once fades over a long thread. The page outline, names and ids only, goes
  only on a thread's first turn.
- **The design is never sent whole.** The agent's earlier reads stay in its session, a process
  that persists or a resumed id, and the preamble says which went stale, so it reads again only
  those layers. Tool results' `changed` cannot do this alone: an agent answering from memory calls
  no tool.
- **A restored conversation** replays its prose, never its tool calls, into a fresh session's
  system prompt; a thread another harness wrote is not replayed, and the chat says so.
- **Document content is data, never instructions.** Layer names, text content, component
  descriptions and variable names may come from someone else's file or kit, so every piece of the
  document the agent reads is fenced and marked as data, and each tool's description ends saying
  so.

### Attaching layers

The person attaches layers to the next question:

- **Two doors attach, and only two**: *Ask the assistant* on a layer's context menu and on the
  canvas selection's, in a group of its own at the menu's end, after *State*
  (`context-menus.md` §3), as the one row that leaves the document; and the ask button on the
  selection. Each attaches the selection as a **chip** above the composer, opens the panel and
  focuses the composer. The rail button, the palette row and `Ctrl+Shift+A` open the panel and
  attach nothing.
- **The ask button** is a small `icon::SPARKLE` button above the selection's top-right corner,
  its right edge on the selection's and its bottom 8 clear of it, one for the whole selection and
  the same size at every zoom. Where there is no room above it sits inside the corner, and it
  stays left of the inspector's cards. Once the selection is attached it is lit and a click only
  opens the panel. It hides while the selection is dragged, resized or rotated, during a marquee,
  while text or a path is edited, in Present mode, in private mode (§6), and while the assistant
  is *Off*, so a person who never turns the assistant on never sees it. *View ▸ Show ask
  assistant* (the canvas group, per machine, on by default; the palette has the same words) hides
  it; the context menu's row stays.
- **A chip is a reference, not a copy**: the layer's id, name, kind and size go with the
  question, and the agent reads the rest through the tools (its subtree, its bounds). For a
  harness whose input takes images, an attached layer also goes with a 1× render, sparing the
  `export_png` call nearly every first turn makes. A chip's × removes it; a chip whose layer is
  deleted shows struck through and is not sent.
- **While the panel is open, an attached layer wears a mark on the canvas**: a dashed accent
  outline 5 outside it, with a tab carrying the sparkle and its chip's number, which tells two
  chips both named *Card* apart. A closed panel leaves no ink. Hovering a chip outlines its layer;
  clicking one selects it and zooms to it.
- **The agent's reply can name layers.** A layer id in a reply renders as the same chip, which
  selects and zooms on click, so an answer about *the third card's button* is one click from it.

## 5. Settings

An **Assistant** section in Settings (`app-canvas-chrome.md`, *Settings*), staged and written on
*Save changes* as the rest is. Stored in prefs, per machine; none of it is a secret.

- **Agent** — a dropdown, *Off* first. A new install starts off. An unknown saved key loads as
  off and is kept until the person picks, never turned into another harness.
- **Program** — the path, empty for found automatically, showing what was found.
- **Model** — free text with the harness's suggestions as chips.
- **Effort** — the harness's own levels; hidden where it has none.
- **Access** — *Look*, *Propose*, *Edit*: the default for a new chat.
- **Instructions** — free text added to every system prompt.
- The seal's sentence (§4) under the Agent row.

Switching agent clears Program and Model and moves Effort to the nearest level the new harness
has, ties downward; each harness's last values come back within the session. A saved change
restarts the session only when it changes what the agent was started with. With *Off*, the rows
below Agent are disabled with that reason.

## 6. The panel

The assistant is a panel at the tool rail's foot, whose buttons read, top to bottom, *Assistant*,
*Assets*, *Styles and variables*, *Insert components*. A floating window was weighed and dropped:
it has to be placed, ends up over the layers being asked about, and is one more kind of surface.

- **The door** is the rail button, wearing `icon::SPARKLE`, History's mark for a step made over
  MCP: the same concept, an agent acting. (`CHAT` stays unused; a speech bubble reads as comments,
  a non-goal.) Its tooltip is *Assistant (Ctrl+Shift+A)*. The palette's *Assistant* row and the
  chord open it too and attach nothing; the context menu's *Ask the assistant* and the ask button
  (§4) open it with the selection attached.
- **It is a `ui::rail_popup`, taller than its siblings.** Its bottom-left is theirs
  (`OndinApp::rail_popup_anchor`), 8 right of the rail and level with its button's bottom; its
  top stops 8 below the canvas's top, so it is as tall as the room above its button less 8. The
  siblings stop at 460 and it does not; on a short canvas it shrinks as they do, to a floor that
  still holds the head, one line of thread and the composer. With the toolbar hidden it opens
  from the anchor's fallback, 8 in from the canvas's bottom-left, and so stands the canvas height
  less 16. It is 400 wide, and its right edge drags between 360 and 640, the only rail panel with
  a drag edge, since tables and code need the room; the width is kept per machine. The height has
  no handle.
- **It shares the place**: opening a sibling closes it, and opening it closes them. Closing keeps
  the thread, the draft and a running turn. While a turn runs out of sight the button wears an
  accent ring; a reply that arrived unseen wears an accent dot until the panel opens; the tooltip
  says which (*Assistant · reply waiting*).
- **The canvas stays live** behind it. `Escape` closes it, as it closes its siblings. Only *Stop*
  stops a turn: a panel switch, a page switch or `Escape` never does; a document switch, a
  settings restart and private mode do, each with a note.
- **Private mode turns the assistant off** while it is on. The panel closes and a running turn
  stops; the rail button, the palette row and the context menu's row are disabled with *Not
  available in private mode*; `Ctrl+Shift+A` answers on the status line as private mode's other
  refusals do; the ask button and the attached layers' marks are not drawn. Nothing is sent to
  the agent. The thread and its chips are kept and come back when private mode is turned off. The
  thread shows the names private mode masks, in chips, tool rows and the agent's own prose, and no
  mask reaches the prose.
- **The head**: the harness's label, the seal's grade beside a lock (*Sealed*, *Restricted*), then
  the Access dropdown and *New chat* (a plus).
- **The thread:** the person's turns right-aligned and shrink-wrapped; the agent's full width,
  with the speaker name the turn was written by, kept with the turn, not read from the current
  setting.
- **A turn is never blank while it waits.** The agent's turn takes its place at the send, with
  the speaker's name and `ui::paint_spinner`, and the words name the stage: *Starting Claude
  Code…* until the harness's first event (on every turn that starts a process), then *Thinking…*
  until the first text or tool call, and again once a tool has finished and no text has followed.
  The seconds count from the send. No skeleton bars: they would promise a shape the answer may
  not have. A harness that exits before answering leaves a note, *Claude Code stopped before
  answering (exit code 1).*, with *Ask again* (the same question and chips) and *Show details*
  (the diagnostics, §3).
- **Replies render markdown**: paragraphs, lists, tables, code blocks with a language bar and
  *Copy*. A Luau block gains *Open in script editor* once the editor exists. A proposal renders
  as its card (§4).
- **An ` ```options ` block renders as a stack of `ui::field_button`s**; a click sends its line as
  the person's next question, with the same chips. Only the last turn's options are live; older
  ones are drawn disabled. A harness that never writes the block reads fine, since the fence
  falls back to a code block.
- **A tool call is a row**, 22 tall in a well: its name in mono, the argument it was given, a
  status dot (running as a ring, done, failed in the error colour) and its result cut short (a
  line count, *image*, the error). After a turn's third call the rest fold into *+N calls*, which
  opens on click.
- **Under each finished turn:** its time and tokens, *Copy*, and on the last turn *Regenerate*,
  which drops that turn and asks again.
- **The composer** is multi-line: Enter sends, Shift+Enter breaks the line, and the send button
  becomes *Stop* while a turn runs. `Ctrl+↑`/`Ctrl+↓` step through earlier questions.
- **When it cannot send**, the composer is disabled with the reason, and off wins over every other:
  the assistant is off (*Open settings*); the program was not found, naming what was looked for
  and where (*Set the program*); the harness reports it is not signed in, saying to run the
  program once in a terminal.
- **The app's own words** (a refused spawn, a stopped turn, a replay withheld) are notes beside a
  turn, never message text, so they are never sent back to the model.
- **One conversation per document**, kept per machine beside the library's view state, never in
  the `.ondin` file: a chat is the person's, not the work's. At most 100 messages; tool results
  are saved as *(result not saved)*, and a turn still streaming is not saved. *New chat* starts
  over; a list of threads has no room in the panel.
- **Glyphs:** *Stop* on the send button and *Regenerate* need glyphs the font lacks, drawn to the
  *Drawing rules* (`app-chrome.md`). The failure note wears `WARNING`; the composer's disabled
  reasons wear none.

## 7. Logging and privacy

- **Off sends nothing and starts nothing**, the probe included.
- **What leaves the machine** is what the harness sends: the question, the attached layers'
  references, and whatever the agent reads through the tools. No document file is handed over,
  and no telemetry is sent by Ondin.
- **The agent log** (`agent_log.jsonl` in the machine data folder) records each tool call the
  assistant makes, refusals included: the tool, the document, the caller, ok or failed. It never
  records results. Rolls over at 8 MiB; always on; never read back by the app.

## 8. Non-goals

- **An embedded model, or a provider API key held by Ondin.**
- **An approval prompt inside a turn.** A headless harness cannot pause for one; the access level
  is the approval.
- **The agent's own tools** — shell, files, web — inside the assistant.
- **Kits and the library through the assistant.**
- **A chat saved in the document.**

## 9. Open questions

1. **`/ai`'s look in the bar** (`vm.md` §13, question 7).
2. **What *New chat* costs.** With one thread per document, a click on the head's plus loses the
   old thread for good: whether it asks first or keeps the last thread to bring back, and whether
   it, or an Access switch, stops a running turn or waits for it.
