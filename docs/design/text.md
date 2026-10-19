# Text node and fonts (§5.4, §5.4a)

The `Text` node kind (scopes, spans, layout, editing, decoration, type on a path, lists) and how fonts are found, loaded and cached. Text styles and string-variable bindings of text are in `styles-variables.md`.

## 5.4 Text node

### Scopes and data

Three scopes (`typography.rs`). A character attribute may vary over a byte range, so it lives in the node's defaults plus per-range overrides. Seven paragraph attributes are spannable too (spacing, first-line indent, `hanging`, the two block indents, list marker, nesting level); every block attribute and the rest of the paragraph scope is a plain field. One list type serves both: `Spans<A>` generic over `SpanAttr`, implemented by `CharAttr` (over `TextStyle`) and `ParaAttr` (over `ParagraphStyle`). Scope is a type-level fact: nothing can write a per-range alignment because no span type carries one.

```rust
NodeKind::Text {
    content: String,
    style: Box<TextStyle>,     // character defaults
    spans: CharSpans,          // per-attribute character overrides; empty on uniform text
    para_spans: ParaSpans,     // per-paragraph overrides, byte-keyed like `spans`
    paragraph: ParagraphStyle, // align, indents, wrapping, direction, spacing, optical margins
    block: BlockStyle,         // node-wide: vertical align, box trim, overflow, max lines
    sizing: TextSizing,
}

pub enum Length { Px(f64), Em(f64) }   // Em is a FRACTION of the font size

pub enum TextSizing {
    Auto,                      // no wrapping; box grows both ways
    AutoHeight(f64),           // wraps at this width; grows downwards
    Fixed(kurbo::Size),        // wraps at width; box exactly this, per BlockStyle::overflow
}
```

`Length` is one type for line height, letter/word spacing, paragraph spacing, indent, baseline shift, decoration thickness/offset.

### Spans

- **One span carries one attribute** (`Span<A> { start, end, attr }`), the shape parley resolves styles in. `Spans::normalize` establishes: no empty span; no two spans of one kind overlapping; no span equal to the node default; sorted by `(attribute, start)` for byte-stable saves.
- **Declaration order of `CharAttrKind`/`ParaAttrKind` is part of the file format.** An existing variant never moves (it would re-sort spans of every document holding both); appending is the convention. `CharAttrKind::ALL` / `ParaAttrKind::ALL` are written-out lists, asserted strictly ascending.
- **Overlap is resolved by painting**: spans of one kind are laid down in sorted order, each byte takes the value of the last span covering it, and a painted-over span keeps what lies outside the shared bytes on both sides. `Reverse(end)` joins the sort key so two spans sharing a start have a defined winner (widest first). Identical ranges with different values are decided by array order (stated, not guarded). Reachable only from a file, hence the loader's normalize (§5.11).
- **`Length` is canonicalized on write** (rounded to 4 places) so span coalescing can use exact equality. `Spans::clamped` also canonicalizes on load via `canonical_attr`; node defaults are not canonicalized on load. `round_to_places` returns a huge magnitude unchanged rather than overflowing to infinity.
- **Scale rule for text is published from core**: `Length::scaled` (multiply `Px`, leave `Em`), `CharAttr::scaled_by`, `ParagraphStyle::scaled`, `TextStyle::scaled`. `TextStyle::scaled` is a field literal rather than a loop through `TextStyle::set` on purpose (`set` canonicalizes and `Spans::scaled` doesn't, which would change which spans are stored); a test over `CharAttrKind::ALL` holds them together.
- **A `Length`'s unit is data even at zero.** The seven optional lengths (`letter_spacing`, `word_spacing`, `baseline_shift`, `spacing`, `indent`, `indent_start`, `indent_end`) skip serializing on `Length::is_default_zero` (default variant), not `is_zero`, so `Em(0)` survives a save. The panel compares on the resolved value: `shared`/`para_shared` fall through to `agreed_zero`, which hands back the first value that cannot be the default (so the unit chip keeps the user's suffix). It stops at zero on purpose.
- **Boundary rule**: typing at the edge of a span inherits from the character to the left unless the caret was explicitly restyled while empty (`TextEdit::pending`). `Spans::edited` is the single implementation, paragraph list included (a paragraph reads the value in force at its first byte).
- **A paragraph-scoped write acts on the selection snapped outward to hard breaks** (`text::paragraph_bounds`, shared by the panel and `TextEdit::paragraph_range`). A selection ending exactly on a boundary doesn't reach the next paragraph (a bare caret there does); a paragraph with no bytes of its own (after a trailing newline) reaches back over its break.
- **`Option<Length>` where `None` means "the font decides"**: decoration thickness and offset, and line height's Auto (`MetricsRelative(1.0)`, CSS `normal`). The offset is positive up and signed; thickness is floored at 0 in `canonical_decoration`. `Some(0.0)` (no line / zero) and `None` (the font's own) are distinct.

### Layout

- Layout is computed by `parley` in `text.rs` and cached in `Resolved` (derived, never serialized). Shaping is the costliest thing in the pipeline and bounds, hit-testing and the scene walk all need it, so nothing outside `Resolved` calls `text::layout` in a hot path. The brush parley carries is our **run index**, which lets the decoration walk recover line style and decoration colour from a `GlyphRun`. Bounds and hit-testing come from `TextLayout::bounds`, the one place the trim offset is applied.
- **`text::YMap`** folds the three post-parley shifts (paragraph spacing, box trim, vertical alignment) into one statement of how far each line moved, read at both boundaries (the scene walk and the hit test), which keeps the caret under the pointer. A shift belongs to a line, not to a y: `YMap::to_local` is right only for a baseline; caret/highlight edges are ink coordinates outside their line box and use `to_local_on_line`. The way back isn't a subtraction (a paragraph gap exists in local y but not layout y): `Shaped::layout_y` finds the line whose box contains or is nearest the point, then asks that line's shift. Spacing is **space-before**, read off the paragraph below each hard break (a gap at every break, none over the opening line).
- **Every line's baseline is stored** (`TextLayout::baselines`, one text-local y per line, filled in `extract`'s line walk from `YMap::to_local(line.metrics().baseline)`). It can't be recovered from glyphs: `baseline_shift` is a draw offset. Used by baseline snapping and the baseline overlay (§9.4). `text::export_lines` reports the same number but re-shapes, so it's wrong on the render path.
- **Box trim never moves the ink.** `TextLayout::origin` is the offset from the node's origin to the box top-left; trimming shifts that and shrinks the box, so glyph coordinates and `TextSizing::Fixed` mean what the user typed. `BoxTrim::None` is the type default (what an absent field means); the app authors new text as `CapToBaseline`. A text node has two boxes: `bounds()` (trimmed, what everything measures with) and `untrimmed_bounds()` (full line box, only drawn, §9.4); equal when trim changed nothing (including every `Fixed` node, where trim moves only the vertical-alignment datum). Consumers test that equality.
- **Indents go in before the break.** `text::break_lines` sets parley's per-line `line_x` and `line_max_advance` from the paragraph the line falls in (`text::line_geometry`); hit test, highlight and ink need nothing extra. The caret is the exception and `TextEdit` corrects it itself. `Layout::set_text_indent` is deliberately never called (a second mechanism on the same edge double-counts).
- **Optical margins** ride the same two values (`ParagraphStyle::optical_margins`, off by default). A line is placed by ink at both edges (`break_lines` does `x -= lsb`, `measure += lsb + rsb`), so no arm reads `align`. Bearings come from the once-broken layout (`text::optical_offsets`) and `shape` breaks again, gated on `Paragraphs::any_optical_margins()`. `shape` settles a line at a time from the top; a line returning to a guess already tried is pinned to that left bearing and right bearing zero; past `4 + 3 × lines` passes the node is laid out uncorrected (a whole-layout fixed point cycles). The node's box (width from `box_of`'s `natural_width` on the uncorrected pass) doesn't move when it's switched on. On `TextSizing::Auto` there's no measure, so only the start edge.
- **Text case is a display transform**, applied to the string handed to the shaper; `content` is never rewritten. Restricted to characters whose cased form is the same number of UTF-8 bytes.

### Fonts metadata

- Metadata is read off the face via `skrifa`/`read-fonts` (`text::family_axes`, `family_named_instances`, `family_variants`, `family_features`); the Character tab builds from these, so a family without small caps offers no switch. **Read but not trusted**: `FontAxis::from_fvar` is the one place a `FontAxis` is built from a file (orders ends, replaces non-finite end with the other, pulls `default` into range); `FontAxis::clamp` is what readers use.
- **Which face is asked depends on the question.** The variant list asks every face in the family (`with_family_fonts`; italics often live in a second file); axis rows and feature switches ask the default face only. The shaper asks the family it will actually shape in: for a family the machine lacks, `text::shape` gathers the bundled fallback's axes (so `opsz` tracks size); "unknown" means `fontique` cannot find the family, not an empty axis list.
- Feature tags: `family_features` returns a `FaceFeature` with only what the font may legitimately say (`ssXX`/`cvXX` label, tooltip, value names); name, description and "is a user's choice" come from a static table beside the panel; generic fallbacks too.

### Editing

`text::TextEdit` (in core) is the caret/selection model over the parley layout (`Cursor`/`Selection`): cluster-correct deletion, word motion, vertical motion, click and drag select. The app owns only when a session starts and ends. The caret rectangle and vertical motion are ours because parley measures a caret in a frame that omits the line's start edge where its hit test reads it; `move_up`/`move_down` keep their own sticky column.

### Ink: fills, strokes, decoration

- Glyph fills come from the node's `Paint`; each visible fill is one pass over the same shaped runs. Strokes go on the glyph outline (`text::outline`) through a shape's stroke construction, so `StrokeAlign::Outside` means outlined type. That outline is also what *Convert to path* uses (`build::outline_text`, §9.4). A text node with no fill **and no stroke** draws black (outlined type is a stroke with no fill).
- **Decoration ink is reported by `Resolved` and drawn at the render boundary.** parley gives position and thickness; the four line styles (solid, dashed, dotted, wavy) are built in the scene walk (like stroke dashes, §6.3) so they don't go stale when size is scrubbed.
- **Skip-ink** is an interval, not a path (`DecorationInk::gaps`, computed in `text::skip_ink` from glyph outlines during relayout). Underlines only (a strikethrough's gaps are always empty). Because it's a span along the band, dash phase and wavy crests remain the scene walk's. Per-underline setting `Decoration::skip_ink` (default on, CSS's initial value); an opted-out band is not listed among the indices handed to `skip_ink`. The SVG writer says nothing for ordinary underlines and writes `text-decoration-skip-ink="none"` for runs that asked for a continuous bar, off the underline alone.
- **`gaps` is the final answer, cosmetics included.** Order is the design: break the band where glyph ink crosses it with no clearance; close fragments that aren't rules (a hole inside one letter's ink, or under half a thickness); only then grow each gap by up to a thickness a side, taking just what the bar beside it can spare above a floor of one thickness. Air narrows a bar, never removes one. Widening first and choosing after makes drawings pop.

### Type on a path

`NodeKind::Text::on_path` is a `BezPath` rail in the node's own space (owned by the node, not referenced). The Text tool's click on an outline makes a railed node (`query::outline_at`, `build::text_on_new_path`, which consumes the path layer, creates the node in the curve's own space and puts it in the layer's slot) — the only way type gets onto a curve; `build::detach_text_path` is the way back.

- `text::bend` runs **last**, after `decoration_ink` and `skip_ink`, so bands and gaps are still plain flat-space intervals; it maps flat `x` to distance along the rail and flat `y` across it. Mapping the whole plane means second lines, markers, superscripts and descenders need no special case.
- **Glyphs are carried rigidly, continuous geometry is warped**: `Glyph::rot` rotates each glyph about its origin; `TextLayout::warp` is what a decoration ribbon follows (and the only reader).
- **Each glyph is placed by its own midpoint** (SVG's rule). Advance is read off the next glyph on the same line **across run boundaries**; only a glyph with no successor borrows its predecessor's. The same-line test isn't an `x` comparison (right/centre alignment can put the next line's first glyph further right).
- **On a rail the box is inert**: `TextSizing` neither wraps nor sizes and `ParagraphStyle::align` becomes an offset along the rail. Every resize door must therefore edit the rail (`tools::resize_to_handle`; group/selection/skew via `tools::scale_geometry`); a door without that arm writes a patch the layout ignores (§5.6).
- The node's box is the ribbon the whole rail sweeps, not the glyph stretch (ink bounds would move the box as you type). Text that outruns the rail is dropped, reported via `TextLayout::truncated`.
- `on_path_flip` is one bool beside the rail (not inside the `Option`, so detach/attach doesn't discard it; same as `Node::mask_mode`): reversing traversal negates the tangent; the normal is a quarter turn, so type reverses and changes sides.
- `PathWarp` maps text-local coordinates both ways: `place` carries the first baseline, `nearest` is the inverse (every pointer question on a railed node). A live `TextEdit` carries a warp of its own (`warp_for`) and answers `caret_quads`/`selection_quads`.
- The rail is drawn as chrome (outline only) while the node is selected or edited, once when both are true.
- **A boolean's operand is neither offered as a rail nor accepted** (`outline_at` skips anything with a `Boolean` ancestor; the builder refuses). **Nor is a mask, nor ink a mask has clipped away** (`outline_at` asks `masked_away`; builder refuses a node whose `mask` flag is set). The governing property is a node's role, written as kinds; each new role needs its own guard.
- `on_path_offset` is where along the rail the type starts, as a fraction of its length (survives scaling); click writes it, `align` composes, SVG adds it to `startOffset`. A closed rail wraps (`PathWarp::fits`; `shortest_way_round` moves a pointer's arc length into the half-turn nearest the text's start, where `nearest` deliberately doesn't); an open one runs out. A closed-rail test needs a non-zero offset or the wrap point sits at the seam.
- A filled dot on the rail where the type starts drags the offset (`Drag::RailOffset`). A preview writes `RenderOverrides` and leaves `Resolved` alone, so the committed offset is read each frame and the new value is `committed + nearest / length`.

### Lists, markers, ellipsis

- **A list marker is ink without bytes.** `ParagraphStyle::marker` is a spannable `Option<ListMarker>`, never in `content` (byte indices drive spans, caret, hit test, case transform), and not a parley inline box (`OutOfFlow` reserves no space and parley ignores the kind in `Cluster::visual_offset`, `from_point`, `Selection::geometry`). The gutter is the paragraph's `indent_start` (which every line honours, so that's the hanging measure); the marker is right-aligned on it so `1.` and `10.` share a text column. A list is a run of consecutive paragraphs asking for the **same** marker; anything else ends it and the next run counts from one.
- **Nesting** is `ParagraphStyle::level` (`u8`), one gutter per level. `ParagraphStyle::start_edge` = `indent_start × (level + 1)` is the only place that multiplication happens (lines read it via `line_x`, the marker for its right edge). `ListMarkers::ordinals` keeps a counter per open level; numbering generalises to same marker at same level. A level with no marker still indents but a markerless paragraph ends every open run. `Tab`/`Shift+Tab` (`TextEdit::step_list_level`) are relative per paragraph and move only paragraphs with a marker.
- **An ellipsis is ink without bytes too.** `BlockStyle::overflow = Ellipsis` shapes a `…` itself, outside the caret/span byte space, styled at the **last visible byte** (`line.text_range().end` of the last drawn line, never a run's range), taking the last span *starting* before that byte. `extract_flat` returns no colour on purpose (the font picker uses it), so `marker_run` and `ellipsis_run` each owe the run colour. `text::export_lines` appends one too (as flowing `<tspan>`s). Room is made by **moving the last line's runs and decoration bands left by the overlap** (never past its left edge), not by clamping the glyph; the visible string is not shortened (bidi-safe). Known gap: the ellipsis goes at the line's right edge regardless of direction; an RTL line's logical end is its left.

### Built / deferred

Rich text, per-paragraph attributes, node-level block indents, the Paragraph tab, lists and nesting are built. Remaining deferrals are in `roadmap.md`.

### Traps

- Appending is the only safe edit to `CharAttrKind`/`ParaAttrKind` (save order).
- Never compute a caret/selection edge's shift from its own y; name the line (`to_local_on_line`).
- Don't use ink bounds for a railed node's box.
- Don't call `Layout::set_text_indent` (double-counts the indent).
- Optical-margin correction must settle line by line, not iterate a whole-layout fixed point (it cycles).
- A new "role" (operand, frame, mask) reaching the rail builder as an ordinary kind needs its own guard.
- Don't resize a railed node by its box; edit the rail.

## 5.4a Fonts

Designers expect local fonts plus the whole Google Fonts library. Sources, in resolution order:

1. **System fonts** — enumerated at runtime via `fontdb` on a background thread.
2. **Google Fonts** — on demand, and the one source the user can switch off (*Settings › Web fonts › Load web fonts*, `prefs::Prefs::web_fonts`, default on). A family catalog is fetched from the Fontsource API, cached at `%LOCALAPPDATA%\OndinData\catalog.json` (`OndinData` because `…\Ondin` is the installer's) and read from there first on every launch; the network is asked again only once that copy is older than `CATALOG_MAX_AGE` (seven days, judged on the copy that could actually be read). The picker lists the full catalog; selecting an uninstalled family downloads its files into a per-user disk cache (`%LOCALAPPDATA%\OndinData\fonts`, `$XDG_CACHE_HOME/ondin/fonts`) and registers them into the `fontique` collection, after which they behave like system fonts, offline included. Download-on-demand is the only workable model (open-licensed, too large to bundle).
3. **Bundled fallback** — one open-licensed font (Inter) compiled in, so tests and headless export are deterministic.

**Variable files are preferred over static cuts**: one file carries every weight and the axis rows and variant list read a face that has the axes (both from `fvar`, §9.2). Variable files are woff2 only, so `ondin-app` decodes them to sfnt (`woff2-patched`) before handing them to core, which knows sfnt alone. A family whose file the **decoder** cannot read falls back to static cuts; the decode is wrapped so a *panic* on a malformed file counts as a decode failure. `FaceFailure` splits `Fetch` from `Decode` and only `Decode` falls back (a transient fetch failure must not permanently downgrade a family). Which file to ask for derives from the axes the API reports; the three Fontsource names aren't interchangeable (`fonts::variable_cut`).

**A web face is a subset** of the shipped font: the CDN path names the subset and the one asked for is the family's `defSubset` (`latin`). Subsetting drops glyphs and the features that need them, so OpenType features read off a web face are not the font's. Check anything about a face's tables against an installed copy.

### Architectural rules

- **Core does no network I/O, ever.** `ondin-core` resolves fonts only through the `fontique` collection it is handed. Acquisition (catalog, downloads, cache) lives in `FontService` in `ondin-app` (shared by GUI and `serve`). A document referencing an unavailable font still loads, lays out (fallback), renders and exports, with a warning; fetching only enhances.
- Documents record family **names** (plus weight/style), never file paths or embedded font data. A missing Google family is fetched automatically on document open and again whenever the family list changes (`ensure_loaded` can only queue a family it can already resolve, and the system scan and catalog both land on later frames). Unknown families fall back with a warning in the MCP snapshot and the Type panel. The fallback covers **metrics** too: `text::shape` gathers the bundled fallback's axes, so text doesn't reflow when the real family arrives.
- **`fonts::FamilyStatus`** is `Ready` / `Pending` / `Missing`. `Pending` folds "a face in flight", "a source that could still list the family hasn't landed", "nothing has asked". `Missing` means every source has landed and none has it (or every face tried was unusable). Needed because `text::is_family_available` can't tell "not yet" from "never" (`snapshot.rs` derives its warning from it). The catalog thread counts as finished when its channel closes, covering failed fetches. The panel shows `theme::color::WARN` on the family row's glyph. `ondin-export` has no `FontService`, so the snapshot still asks core directly.
- **Only font data for a family the document names invalidates the text cache.** `FontService::poll` reports family-list growth and font bytes registered separately; `FontPoll` carries registered family names and `FontPoll::touches` asks the question. The family set is the node's default **unioned with its spans** (`CharAttr::Family` spans appear in no `TextStyle::font_family`); both callers read one walk so the gate and `ensure_document_fonts` agree. The picker-miss retry is deliberately not narrowed. A family bound to a string variable adds every value of its variable across all modes and kit modes (`app::bound_families`), and the document's fonts load once per revision while the table holds a string variable (`OndinApp::bound_fonts_at`).
- **Font acquisition never happens on the UI thread; registration and rasterization do** (the `fontique` collection is thread-local, §5.9). The `fontdb` scan, local reads, catalog fetch, download and decode run on a worker pool with three lanes (`fonts/queue.rs`): *urgent* FIFO (a face something is waiting on), *preview* LIFO capped at `PREVIEW_BACKLOG` (picker rows that may scroll away), *prefetch* FIFO uncapped and last (the popular set). A job the preview cap evicts must be given back to the bookkeeping or its family becomes unrequestable. One shared `ureq::Agent`; every request is bounded (connect, response, body, global deadlines; ureq defaults are `None`). Every job is unwind-guarded (`load_one_face`, `sfnt_from`) so a panic can't skip the `FontMsg::Failed` that clears `claimed`/`outstanding`; this rests on `panic = "unwind"`, enforced by the build. Workers repaint after each successful send. A picker row's preview strip goes through core's thread-local shaping engine, so it's bounded in time instead (2 ms of a frame, `FRAME_BUDGET` in `fonts/preview.rs`, past which a row draws its name in the UI font and asks for another frame).
- **Preview-registered fonts can be un-registered**: `text::unregister_family` plus an LRU over preview-only families, budgeted in **bytes** (a Latin subset is tens of KB, a CJK face megabytes). A family anything asked for in full (document font, user pick) is exempt for good. `fontique` keeps a family's name after its last face is removed, so `text::is_family_available` asks about faces.
- **Disk cache is bounded in bytes and swept once at startup**: `fonts::trim_font_cache` deletes least-recently-used faces down to `FONT_CACHE_BYTES` (256 MB; a cache hit touches the file so mtime means last wanted). Spawned on its own thread by `FontService::new`; a single session can exceed the budget. No floor, unlike the preview LRU.
- **Cache size is visible**: *Settings › Web fonts* shows the cache size and *Clear* (`fonts::dir_bytes` on a background thread when the modal opens, so a *counting* state precedes the number; `FontService::clear_cache` synchronous). Clearing un-registers nothing; cost is a re-download each.
- **The whole web source is switchable and off means no request** (the updater is outside it, gated only by `ONDIN_NO_UPDATE_CHECK`, §9.1). Off: the picker lists installed families plus bundled Inter, `FontService::new` doesn't spawn the catalog thread, no face downloads. Neither state hides a family the other shows (a three-way `Off | Popular | All` was rejected). Three doors must close: `rebuild_families` leaves web names out of the picker; `faces_of` refuses to dispatch a web face (a document saved with the switch on can name one without the picker); and **the fetch already in flight**: `FontService::catalog_live` is a cancel flag per spawned thread, read before the fetch and again before the write. Off cancels; on spawns with a fresh flag (reusing the flag would un-cancel the thread holding it). A call in flight can't be interrupted, so its result is dropped rather than written. `FontService::set_web_fonts` moves it within a session; turning on is a rebuild when the catalog is in memory.
- **The popular set is prefetched**: `fonts::POPULAR` is fifty editorial families (the catalog carries no rank), warmed at `Want::Preview` once per session from `poll` the frame `catalog_done` turns true, via the **prefetch** lane (the picker's LIFO lane would reverse order and evict the rest on scroll). `Lane` is separate from `Want`; `FamilyState::lane` is stored so a variable-file retry returns down its lane. Families already local are skipped; the `web_fonts` guard refuses when off, but a request already queued or in flight (up to fifty) is not stopped. **Rule**: anything new reaching the network for the web-font source must be startable only while the switch is on, or hold a cancel flag like `spawn_catalog`.
- Font embedding in `.ondin` files (portability of non-Google local fonts) is deferred (§14).

### Traps

- `is_family_available` can't distinguish pending from missing; use `FamilyStatus` for UI.
- A preview-evicted job must be returned to the queue bookkeeping, or the family is unrequestable.
- An unwinding panic in a worker job must still send `Failed`; keep `panic = "unwind"`.
- Cache-size scans must not run per frame.
- Checking a face's features/tables against a web subset gives the wrong answer; use an installed copy.
