# Ondin — Roadmap

Open work only. **Now** is in the order it is taken up; **Later** waits on a stated trigger;
**Considerations** are ideas not yet decided. Decided non-goals are in `non-goals.md`; built
work is described in `architecture.md`. Refs are file + function name, never line numbers. Read
the code before picking an item up — an item's premise can go stale while its ask still reads
true.

## Now · MCP

Fewer tokens and fewer round trips for an agent driving `ondin-mcp`. Measured on a live run (a
hero section built into an existing page): `tools/list` is 164 tools and ~119 KB, a third of it
the same `base_rev` and results-are-data text repeated per tool; learning the page's type and
colours took about ten reads; the one `insert_subtree` was 13 KB of output, most of it float
colours, stroke fields at their defaults and hand-placed coordinates. Each item lands with a size
budget test, and the set is checked by re-running the same agent task on the old and new build.

A live hero run on the core set took twice as long as before it, nearly all of it six whole trees
`insert_subtree` refused one mistake at a time; a pass over every core tool found the rest below.

- **Model arguments say what is wrong, and where** — `verbs::Args::model`: a refused model names
  its path (`children[1].kind.Text.sizing`), and a field the model does not have is refused, not
  dropped (`Flex { gap }` laid out with no gap).
- **What a read says, a write takes** — `verbs/loose.rs`: lengths as `"56px"` / `"1.2em"`, sizing
  as `get_node` writes it, an effect flat with `visible` left out, corners left out or as one
  number, padding as one number, a path as SVG path data (and `get_tree` answering it so), a
  partial `TextStyle` in `set_text_style` and `set`, a corner by name in `bind_variable`, and every
  colour as hex once it is within a rounding of one.
- **Results stay small** — the `changed` report caps its list and names the top-level layers past
  the cap (`get_view` listed 600 after a frame was deleted); a refused write leaves `rev` where it
  was; `undo` and `redo` name the step; `check_contrast` lists failures unless asked for all.
- **One way to place and patch** — `set` runs inside `batch`; `at` places a group's box as it
  places a layer's; `create_instance` takes the box's left and top as `create_text` does;
  `set_variable_value` takes the only mode when there is one.
- **Finding the tool** — the core set gains `create_node`, `make_component`, `define_property`,
  `add_style` and `add_variable`; `describe_tools` ranks name matches first, can answer names only,
  and says which names it did not find; `insert_subtree` points to `get_tree`, `like` and
  `create_node`, `set` to `style_text`.
- **Readings that are wrong** — `check_contrast` measures a text against its own ink (orange on
  off-white scored 1 : 1); `get_outline`'s boxes count stroke ink; `export_code` writes a solid fill
  as a one-stop gradient and a bound corner as nested `min`/`max`; a kit variable can keep a cell
  for a mode its collection does not list.
- **Short ids per client** — `n36` for `48c991654e6b8e81:36`, mapped per client key, surviving the
  proxy's reconnect.
- **Revert one step** — with the assistant (`ai.md`): undo an agent's own step under later edits
  that touch other layers.
- **A script tool** — Command Mode (`vm.md`) exposed to agents, so a loop runs in one call. Waits on
  Command Mode.

## Now

- **Command Mode** — `architecture.md` §13, `vm.md`. Its verbs are the registry's (`ondin_core::verbs`, built). Scripts
  stored inert, never run by the document (§15 D1100). Open in `vm.md` §13: the editor's UI, the
  parameter header, `mlua` exposing Luau's analysis, files outside the document (parked), first-run
  disclosure, schema place, kit scripts. `/ai` is built after the assistant.
- **The assistant** — `ai.md`. `ondin-mcp` serves every phase's tools. Open: finding and
  probing each harness (Gemini CLI's stream, resume and seal not yet measured), the sealed spawn per harness and its stream parser, `propose_edit` and its
  card, the panel at the rail's foot, attaching layers from the canvas, the ask button and the
  layers tree, Settings' *Assistant* section, the agent log. Open in `ai.md` §9. Parked: ACP as
  the transport, until Claude Code speaks it without an adapter (`ai.md` §2).

## Later · Platform and dependencies

- **egui 0.36** is blocked by vello requiring wgpu 29 (check — last asked 2026-09-03). Re-derive
  `ui::menu_rows`' button-style arithmetic when it moves.

## Considerations

Ideas worth weighing, not yet decided or scheduled.

- **A control channel into the running app.** Loopback only: read UI state, drive tools with
  synthetic pointer and key events, and screenshot the window. Agents never launch the GUI because
  the window takes the user's mouse and keyboard; a window that accepts input only through the
  channel (hidden or offscreen) would let an agent check chrome against the real app instead of
  throwaway probe tests. It could grow from the local socket the window already listens on for
  open requests (`one_window`).
- **Multiplayer without a server.** The host's app holds the session and peers join by link, peer
  to peer over QUIC with NAT traversal and an end-to-end encrypted relay when no direct path exists.
  Edits merge per property, a delete beats an edit, and each person's undo takes back only their
  own edits; optional password, approval of each joiner, view-only access. §12 locks in the ids and
  transactions such a layer needs (components bend its fourth property, §15 D978) and leaves the
  protocol and conflict rules undesigned; this shape needs no hosting and no accounts. The cost:
  the session ends when the host closes the file.
- **A tolerant pixel diff.** A compare with anti-aliasing detection and SSIM that writes an HTML
  report. The PNG golden is absent because `vello_cpu`'s bytes vary with SIMD level and version
  (*Parked decisions*); a compare that forgives edge pixels and still catches a moved shape is what
  would unblock it. The same harness could hold approved screenshots of the chrome as regression
  baselines.
- **A centroid snap during a move.** A layer's visual centre (`ondin_core::centroid`) as a source
  in `snap_rect` while the Align card's visual-centre toggle is on. Worth weighing only once the
  toggle has seen use.

## Parked decisions

- Binary save format (the versioned JSON format ships).
- Rich text model migration (`runs: Vec<(Range, TextStyle)>`, `architecture.md` §5.4).
- History cap / snapshot compaction for huge delete inverses.
- Font embedding for non-Google local fonts (`architecture.md` §5.4a).
- **Justify-all** — blocked upstream in parley (`align_impl`, `pub(crate)` internals); trigger is a
  parley patch (§15 D825).
- **A stored, user-placed transform origin for a multi-selection** — unparks when a user asks for it.
- UI toolkit revisit (gpui / Xilem), `architecture.md` §9.1.
- MCP transport hardening (auth, TCP/remote).
- `vello_hybrid` as a third backend if low-end GPUs demand it.
- **PNG goldens** — wait on an aarch64/NEON measurement (§15 D304); per-`vello_cpu` too (D414).
  `num_threads` is still a default.
- **Layers panel `MIN_W = 210`** rests on one reading, not two (§15 D444): re-tighten or amend its prose.
- **Chrome hairline base** in `rulers::snap_across_axis` (guides) is unruled (§15 D479). The pixel
  grid no longer calls it: it places lines from its lattice, which
  `the_lattice_picks_the_pixels_the_snapped_lines_did` holds to the pixels `snap_across_axis` picks,
  so ruling the base means moving the lattice with it.
- **The Type panel's four `%` caps** are 200, inherited, not chosen (§15 D861).
- **Dashboard virtualisation** — deferred (§15 D862); not a one-line `show_rows` (rename field, ⋮ menu,
  `follow_selection` depend on drawn cards).
- **Components: five unruled questions** — a group instance's canvas label; a main-scoped guide not
  copied; a linked node moved into a nested instance losing its link; instance roots exempt from
  membership (§15 D978); a moved-out layer previewed on copies (§15 D979 (xiv)).
- **`Ctrl+Alt` is `AltGr`** on many European layouts (`shortcuts.md` §12, §15 D987).
- **One font-availability question for panel and snapshot** — only worth it if the snapshot ever
  reads a live `FontService`.
