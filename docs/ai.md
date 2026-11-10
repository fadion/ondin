# Ondin — the assistant

> **Status: design, not built.** Nothing here exists in code. It depends on `ondin-mcp` serving
> at least its Phase 1 read tools (`design/mcp.md` §8.4). When a part is built, its rules move
> into the design file the code lives under, and this file keeps only what is still unbuilt.

The assistant lets a person ask a coding agent about the design in front of them: what a layer is
doing, why a layout breaks at a width, how to name a set of variants, what to change to fix a
contrast failure. They ask in a chat window, attach layers from the canvas, and read answers that
can point back at layers and propose edits. Command Mode's `/ai` (`vm.md` §11) is a later door
into the same assistant.

## 1. The thesis

**Ondin embeds no model and holds no provider key.** The person already pays for, and is signed in
to, a headless coding agent — Claude Code, Codex and their kin. Ondin starts that agent as a
child process, points it at Ondin's own MCP endpoint, and shows its turn in a chat window. The
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

- **Behaviour is asked as a capability, never by identity**: persistent or per-turn, resume,
  interrupt, prompt on stdin, system prompt in the first turn, effort levels, how it is sealed.
  Each capability is an exhaustive `match`, so a new harness is a compile error, not a quiet
  default.
- **Every harness streams JSON lines**: stream-json for Claude, Antigravity and Cursor, the
  `--json` / `--format json` forms for the rest. No protocol that has not been measured against a
  real binary is driven.
- **Gemini CLI is not a harness**: it needs an API key, and the assistant holds none.

### Finding and probing the program

- **Lookup order:** an `ONDIN_<HARNESS>_BIN` variable, then each harness's known install folders
  (CLIs are often installed off the inherited `PATH`), then `PATH` with `PATHEXT`. A path set in
  Settings overrides all three.
- **On Windows** npm's `.cmd` shims come last, since Rust refuses a multi-line argument to a batch
  file. Behind a shim, the real program is started directly, so *Stop* kills the agent and not
  only its `cmd.exe`. Every child is started with `CREATE_NO_WINDOW`.
- **Ondin never installs a harness.** A missing one is said in the chat window with the program it
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
holds them — and the chat window and Settings say which, naming the mechanism, including where a
harness cannot keep the person's own MCP servers away.

Never passed, whatever a harness offers: any skip-permissions, auto-approve, allow-all or yolo
flag. A headless harness cannot ask the person mid-turn, so its allow-list is exactly the tools
the access level grants and everything else is denied.

### Access levels

One setting, **Access**, decides what the agent may do. Tools above the level are **left out of
the tool list**, not listed and refused: an agent that sees a tool plans around it.

- **Look** — Phase 1 reads: the snapshot, nodes, children, bounds, the selection, `hit_test`,
  `export_png` and `export_svg`. `export_png` is the agent's eyes (§8.4).
- **Propose** (the default) — Look, plus `propose_edit`. The agent sends a list of registry verb
  calls (`ondin_core::verbs`), which Ondin checks as `verbs::call` checks any call and returns
  the result to the agent. The agent then shows the proposal in its reply, and Ondin draws it as
  a card: what it changes, in the verbs' own words, and **Apply**, which commits it as one
  transaction and one undo step named after the proposal. Nothing changes until the person
  applies it.
- **Edit** — Look, plus the write tools of Phases 2 and 3. Each call commits as its own undo step
  marked as the assistant's (`Origin::Mcp`), so the History section shows which steps the agent
  made. Kits and the library (Phase 4) stay out of the assistant at every level.

### Context sent with a turn

- **The system prompt** says what Ondin is, the access level and its tools truthfully, how to
  propose an edit, and the person's own instructions from Settings.
- **Each turn** carries what changed since the last one (the current page, the selection) and a
  recap of the person's last three questions, since a harness's own memory of earlier turns is
  unreliable. The page outline goes only on a thread's first turn for a harness that resumes.
- **A restored conversation** replays its prose, never its tool calls, into a fresh session's
  system prompt; a thread another harness wrote is not replayed, and the chat says so.
- **Document content is data, never instructions.** Layer names, text content, component
  descriptions and variable names may come from someone else's file or kit, so every piece of the
  document the agent reads is fenced and marked as data, and each tool's description ends saying
  so.

### Attaching layers

The person attaches layers to the next question:

- *Ask the assistant* on a layer's context menu and on the canvas selection's, and dragging rows
  from the layers tree onto the chat window. Each adds the layer as a **chip** above the composer.
- **A chip is a reference, not a copy**: the layer's id, name, kind and size go with the
  question, and the agent reads the rest through the tools (its subtree, its bounds, a render of
  it). A chip's × removes it; a chip whose layer is deleted shows as gone and is not sent.
- **While attached, a layer wears a mark on the canvas**, so the person sees what the agent will
  be asked about. Hovering a chip outlines its layer; clicking one selects it and zooms to it.
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
- **Access** — *Look*, *Propose*, *Edit*.
- **Instructions** — free text added to every system prompt.
- The seal's sentence (§4) under the Agent row.

Switching agent clears Program and Model and moves Effort to the nearest level the new harness
has, ties downward; each harness's last values come back within the session. A saved change
restarts the session only when it changes what the agent was started with. With *Off*, the rows
below Agent are disabled with that reason.

## 6. The chat window

A floating window of the app's own, like the script editor (`vm.md` §10): resizable, not modal, its
size and place kept per machine, so the canvas and the selection stay live behind it. Opened by
`Ctrl+Shift+A`, the palette's *Assistant* row and the context menu's *Ask the assistant*.

- **The thread:** the person's turns right-aligned and shrink-wrapped; the agent's full width,
  with the speaker name the turn was written by, kept with the turn, not read from the current
  setting. *Thinking…* with a running time until the first text arrives.
- **Replies render markdown**: paragraphs, lists, tables, code blocks with a language bar and
  *Copy*. A Luau block gains *Open in script editor* once the editor exists. A proposal renders
  as its card (§4).
- **A tool call is a row**: its name, a status dot (running, done, failed in the error
  colour) and its result cut to a few lines with how many more there were.
- **Under each finished turn:** its time and tokens, *Copy*, and on the last turn *Regenerate*,
  which drops that turn and asks again.
- **The composer** is multi-line: Enter sends, Shift+Enter breaks the line, and the send button
  becomes *Stop* while a turn runs. `Ctrl+↑`/`Ctrl+↓` step through earlier questions.
- **When it cannot send**, the composer is disabled with the reason, and off wins over every other:
  the assistant is off; the program was not found; the harness reports it is not signed in.
- **The app's own words** (a refused spawn, a stopped turn, a replay withheld) are notes beside a
  turn, never message text, so they are never sent back to the model.
- **One conversation per document**, kept per machine beside the library's view state, never in
  the `.ondin` file: a chat is the person's, not the work's. At most 100 messages; tool results
  are saved as *(result not saved)*, and a turn still streaming is not saved. *New chat* starts
  over.

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

1. **Several conversations per document**, or one with *New chat*, as above.
2. **Images in a question**: an attached layer's render sent inline for harnesses whose input takes
   images, rather than leaving the agent to call `export_png`.
3. **Access per document**, for a file whose author should not get *Edit* by default.
4. **`/ai`'s look in the bar** (`vm.md` §13, question 7).
