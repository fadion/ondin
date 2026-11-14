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

- **Short ids per client** — `n36` for `48c991654e6b8e81:36`, mapped per client key, surviving the
  proxy's reconnect.
- **Revert one step** — for the assistant (`design/assistant.md`): undo an agent's own step under
  later edits that touch other layers.
- **A script tool** — Command Mode (`vm.md`) exposed to agents, so a loop runs in one call. Waits on
  Command Mode.

## Now · Library

Documents are found by name alone: search is a lowercase substring of the display name or a
project's name (`OndinApp::search_results`), and the scan reads only a file's first 4 KB, so the
library knows nothing of what a document holds. A card shows its cover, name and age.

- **Ranked search** — every word must match, exact before prefix before word start before
  substring, as `assets::sets::rank_name` ranks icons; one rule for search, the palette and the
  results page.
- **Sortable list columns** — a click on a *Name*, *Project*, *Edited*, *Created* or *Size* header
  (`OndinApp::file_list`) sorts by it, a second reverses; `Sort` gains a direction the sort control
  offers too.
- **Day headings under a date sort** — *Today*, *Yesterday*, *This week*, *Earlier*, worded as the
  version-history card's (`versions.rs`).
- **Search inside documents** — page, top-level frame, component and style names and text
  content, each result saying why it matched (*page: Checkout*). An index built on the cover
  worker (`library::cover`), which already loads the whole file, cached in `machine_dir` keyed as
  covers are (id plus mtime), filled for off-screen documents in the background.
- **A results page** — *Show all N results* (or `Enter` with no row steered) opens the matches as a
  grid or list where sort, the project filter and the view switch apply; the dropdown stays capped
  at `SEARCH_MAX`.
- **Documents in the command palette** — *Go to document…*, recent first, ranked as search is, so
  switching needs no trip to the library.
- **Continue where you left off** — a wide card heading *Recent* for the last document, its cover
  drawn at the page and camera it was left on (`LocalIndex::last_page`, `last_view`).
- **More on a card** — page count and a kit's version on the caption's second line (*4 pages · 2h
  ago*), and *Pages* and *Opened* list columns; from the search index, so after it.
- **Pages on hover** — moving across a cover steps through each page's cover; needs a cover per
  page in `Covers`, so after the card line.
- **Kit usage on *Kits*** — *Used in N documents* on a kit's card and the documents in its menu,
  from `kits::users`.
- **A project page header** — the project's colour, file count, last edit and the *Recent* mosaic
  (`mosaic_cells`), and an optional one-line description edited in *Edit project*, kept in
  `projects.json` so no document is rewritten.
- **Small keys** — typing over the grid opens search with that letter; `Ctrl+K` inside a project
  searches it, with a chip to widen to the library; *Starred* and *Kits* offer *Last opened* as a
  sort.

## Now

- **Command Mode** — `architecture.md` §13, `vm.md`. Its verbs are the registry's (`ondin_core::verbs`, built). Scripts
  stored inert, never run by the document (§15 D1100). Open in `vm.md` §13: the editor's UI, the
  parameter header, `mlua` exposing Luau's analysis, files outside the document (parked), first-run
  disclosure, schema place, kit scripts. `/ai`, its door into the assistant, and a reply's *Open in
  script editor* (`ai.md`) come with it.

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
- **ACP as the assistant's transport** — until Claude Code speaks it without an adapter
  (`design/assistant.md` §2).
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
