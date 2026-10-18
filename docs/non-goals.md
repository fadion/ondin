# Ondin — Decided non-goals

What is deliberately not being built. An item here is decided and not open work; do not re-report
it as a gap. Post-v1 work with a design of its own (MCP, Command Mode, multiplayer) is in
`roadmap.md`.

## Scope

- **MCP and Command Mode in v1** — designed (`architecture.md` §8, §13, `vm.md`), built after the editor.
- **Prototyping.**
- **Plugins.**
- **Multiplayer** — kept possible by the model (`architecture.md` §12), not built.
- **Eyedropper and comment tools.**
- **A unit system** — a screen tool's units are all px at 1x; density belongs to export (§15 D358).

## Canvas and interaction

- **Modifier badges on the cursor** — modifiers sit on stock cursors, so a badge has no base to draw on (§15 D233).
- **Nudging the transform origin from the keyboard** — the origin pills take typed values (§15 D308).
- **Aligning a multi-selection to its container** — select and align one at a time; mixed parents have no one container (§15 D309).
- **A lock blocking inspector edits** — a lock guards against canvas accidents, not deliberate panel edits (§15 D323).
- **Paste at the pointer for `Ctrl+V`** — `Ctrl+V` keeps its fixed offset; *Paste here* is the pointer-aimed one (§15 D300).
- **A frame's *Background…* context-menu row** — the Fill row already does it (§15 D800).
- **A cached rounded `Path` outline map** — the per-frame rebuild is an accepted cost, with D741's local-box map (§15 D778).
- **A rotation handle or mid-edge squares** — sides still resize (§15 D14).
- **A user-draggable ruler origin** — the rulers would disagree with the world-space inspector; rulers re-origin only on a lone selected frame (§15 D137, D36).
- **Middle-button pan over the origin fields** (§15 D254).
- **One insertion line for the layers tree and the paint panels' reorder** — a gap cannot name a parent (§15 D306).
- **A resizable inspector column** — its width is a function of the design (§15 D351).
- **Unique layer names** — names only need to be displayable (§15 D127).
- **A grid's `baseline` alignment in the cards** (§15 D919).

## Booleans and paths

- **A per-operand boolean cache** — measured, nothing to win (§15 D239).
- **`Flatten` on a lone non-boolean** — *Outline shape* (`build::outline`) is that verb (§15 D230).

## Effects and paint

- **Blend modes**, including per-effect blend modes.
- **Background blur** — the export cannot contain the backdrop and SVG cannot express it; no `backdrop-filter`.
- **Noise** — one generator written three times to agree to the pixel; belongs in fills if anywhere.
- **Reordering an effect stack** — order changes a pixel in only two measured cases (§15 D340).
- **Arrowheads, variable-width profiles, separate start/end caps** — a `Stroke` is ink at one width along an outline (§15 D126).
- **A cap on the number of fills or strokes** (§15 D133).
- **A pool of reusable effect-pass renderers** — the build cost is a one-off hitch, and a pool conflicts with a correctness rule (§15 D779).
- **Reading `Filters` (brightness/contrast) back from SVG** — the combined matrix doesn't identify its inputs (§15 D411).

## Text

- **Rich text runs** (check — character spans exist; this may mean only a separate runs model).
- **Vertical writing mode** — and its OpenType tags stay withheld (§15 D279).
- **Hyphenation** — declined on price, not blocked (§15 D399).
- **Tab stops, columns, widow/orphan control** — own builds nobody has asked for (§15 D825).

## SVG import

- **`<foreignObject>`** — HTML; nothing in the model draws it; reported as skipped (§15 D813).
- **Rejoining a multi-line paragraph** — the signal is indistinguishable from stacked labels (§15 D826).

## Images

- **Formats beyond PNG, JPEG, WebP and GIF** (TIFF, BMP, …) (§15 D191, D600).
- **Linked images** — *Embed*/*Link*, *Relink…* and *Embed all*; no gesture authors a linked source (§15 D819).

## Library and files

- **A multiple selection in the library** — every verb takes one document (§15 D383).
- **Rollback after a partly-failed migration** — the retry is the recovery (§15 D814).
- **Per-file "missing" marks** — the unit that goes missing is the whole library (§15 D384).
- **Disabling or warning-styling *Open* on an unreadable card** — the mark already says it (§15 D818).
- **Detecting OneDrive's machine-name conflict copies** — indistinguishable from a typed name (§15 D375).
- **Queueing `Ctrl+S` and document-leaving transitions on the writer thread** — they read the result on the next line (§15 D393).
- **A "remember last page" dashboard option** — the default page is a Settings preference (§15 D749).

## Styles and variables

- **Binding a `Path`'s corner radii** — unbindable by decision (§15 D1057).
- **Editing a style with no users** — *Edit* works through a style's users (§15 D1063).

## Code export

- **Native targets** (SwiftUI, Compose, Flutter, Floem) — an agent porting the HTML does it better (§15 D1098).
- **Interaction states** — a variant named *hover* does not become `:hover` (§15 D1098).
- **Responsive breakpoints** (§15 D1098).
- **Tailwind and CSS-in-JS in v1** (§15 D1098).
- **Code layers, and importing HTML/CSS or framework code as layers** — too complex fully, messy partly; SVG import is untouched (§15 D1099).

## Tests and goldens

- **`f32_pipeline` for `OptimizeQuality`, and keeping strokes out of the PNG fixture** (§15 D304).
