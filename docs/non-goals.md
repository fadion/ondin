# Ondin — Decided non-goals

What is deliberately not being built. An item here is decided and not open work; do not re-report
it as a gap. Post-v1 work with a design of its own (MCP, Command Mode) is in `roadmap.md`, and
multiplayer is one of its considerations.

## Scope

- **MCP and Command Mode in v1** — designed (`architecture.md` §8, §13, `vm.md`), built after the editor.
- **Prototyping.**
- **Plugins.**
- **Multiplayer** — kept possible by the model (`architecture.md` §12), not built.
- **A comment tool.**
- **A unit system** — a screen tool's units are all px at 1x; density belongs to export (§15 D358).
- **Custom key bindings** — the keymap is fixed (`shortcuts.md`).
- **A UI scale setting.**

## Canvas and interaction

- **The eyedropper sampling outside the window** — it samples the canvas and the app's own chrome
  (`tools.md`, *The eyedropper*); off-window would need a capture path per OS and a conversion from
  the display's colour space.
- **A compare's Swipe handle following a hovered pointer** — the handle moves only when dragged (`app-compare.md`).
- **A modifier's badge painted beside the pointer as canvas chrome** — tried and declined; the badge is drawn into the cursor itself (`tools.md`, *Canvas cursors*).
- **Nudging the transform origin from the keyboard** — the origin pills take typed values (§15 D308).
- **Aligning a multi-selection to its container** — select and align one at a time; mixed parents have no one container (§15 D309).
- **A lock blocking inspector edits** — a lock guards against canvas accidents, not deliberate panel edits (§15 D323).
- **Paste at the pointer for `Ctrl+V`** — `Ctrl+V` keeps its fixed offset; *Paste here* is the pointer-aimed one (§15 D300).
- **A frame's *Background…* context-menu row** — the Fill row already does it (§15 D800).
- **A cached local box on `Resolved`** — the chrome's six walks a frame are an accepted cost, and a gesture would bypass the cache (§15 D778).
- **Retained per-frame scenes or effect results kept across frames** — a pan, a preview and an edit already redraw only what they changed (`render.md` §6.3); the caches would serve only a full redraw at an unchanged scale, never the zoom gesture.
- **A rotation handle or mid-edge squares** — sides still resize (§15 D14).
- **A pixel grid drawn coarser than one unit** — the drawn grid matches the 1px lattice snapping uses; a coarser one has no workflow that needs it.
- **A user-draggable ruler origin** — the rulers would disagree with the world-space inspector; rulers re-origin only on a lone selected frame (§15 D137, D36).
- **Middle-button pan over the origin fields** (§15 D254).
- **`Tab` held inside a menu or a dropdown's list** — a desktop menu's keys are its arrows, `Enter` and `Escape`, and `Tab` does not cycle it; popovers, dialogs and cards hold `Tab`, menus do not (`app-chrome.md`, *Two kinds of focus*).
- **One insertion line for the layers tree and the paint panels' reorder** — a gap cannot name a parent (§15 D306).
- **A resizable inspector column** — its width is a function of the design (§15 D351).
- **Unique layer names** — names only need to be displayable (§15 D127). Batch rename makes no name unique, and its preview does not flag duplicates.
- **Batch renaming pages, styles or variables** — the box renames layers only.
- **Letter-case switches in batch rename** (*Title Case*, *UPPER*, …) — *Keep case* carries a match's case, and nothing else changes it.
- **A grid's `baseline` alignment in the cards** (§15 D919).
- **A polygonal (click-by-click) lasso, a contain-only lasso rule, or shape-testing for the box marquee** — the lasso touches shapes, the box keeps its bounds rule.
- **On a loose selection's spacing handles: bands over uneven gaps, a negative gap, a value typed on a band, or a Gap field in the Align card** — Distribute or Tidy up evens uneven gaps, after which the bands appear.

## Booleans and paths

- **A per-operand boolean cache** — measured, nothing to win (§15 D239).
- **`Flatten` on a lone non-boolean** other than a mirrored path, which it makes plain — *Outline shape* (`build::outline`) is that verb (§15 D230).
- **Pressure, tilt, variable width, brushes, an eraser or a smoothing setting on the pencil** — it makes geometry, not ink; *Simplify path* already smooths after the fact.
- **Vector networks** — a model change with no half-step, since every reader and export takes a `BezPath`; the shape builder, point snapping and path cutting cover the same needs with plain paths.

## Effects and paint

- **Noise** — one generator written three times to agree to the pixel; belongs in fills if anywhere.
- **Arrowheads, variable-width profiles, separate start/end caps** — a `Stroke` is ink at one width along an outline (§15 D126).
- **A cap on the number of fills or strokes** (§15 D133).
- **A pool of reusable effect-pass renderers** — the build cost is a one-off hitch, and a pool conflicts with a correctness rule (§15 D779).
- **Reading `Filters` (brightness/contrast) back from SVG** — the combined matrix doesn't identify its inputs (§15 D411).
- **OKLCH or wide-gamut colour entry** — the document stores sRGB.
- **Diamond gradients** — Figma-only; peniko has no such kind, so the model would need its own gradient type for it. The Figma import reads one as radial and says so.
- **A CSS display format in the picker** — CSS colours are typed or pasted into any colour field; the value row shows Hex, RGB, HSL or HSB.
- **Setting one channel across layers that disagree** — over a *Mixed* row the value row is the text field, which sets the whole colour.
- **Pattern fills (a layer or component tiled as a fill)** — niche outside illustration, a tiled image fill covers the common case, and CSS can only repeat an image.

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
- **Reading Figma's `.fig` binary** — the format is unpublished, reading it reverse-engineers another company's product, and it breaks when Figma changes it; Figma files come in by link through the documented REST API.
- **EPS import** — PostScript is a program, and reading it needs an interpreter (Ghostscript, AGPL); PDF and AI import cover the vector files that matter.

## Components and layout

- **Pairing a state's layer with rest's by hand** — states pair by lineage key, then name path (§15 D982); an unpaired layer is fixed on the layer itself, where the Variants card's *Select it* leads.
- **A verb that un-makes a component** — delete the main, which detaches its instances, or detach an instance.
- **Clean up removing layout from one-child frames** — the layout still pads, aligns and hugs, and code export writes it as flex.
- **CSS block, inline and float layout, `calc`, `order`, and margins other than `auto`** — layout is flexbox and grid; gap and padding stand in for margins, and the layers panel's order for `order`.

## Styles and variables

- **Binding a `Path`'s corner radii** — unbindable by decision (§15 D1057).
- **Editing a style with no users** — *Edit* works through a style's users (§15 D1063).
- **Per-node overrides keyed by breakpoint** — every reader would take them; a collection picked by width carries breakpoints instead.
- **A kit's collections in the variables browser's collection picker** — a kit's variables list under the document collection they map onto, and its modes are chosen through that mapping.

## Export

- **Cutting a frame into a grid of slices, nine-slice scaling, and a slice bound to a layer's bounds** — a layer's own Export already exports its bounds.

## Code export

- **Native targets** (SwiftUI, Compose, Flutter, Floem) — an agent porting the HTML does it better (§15 D1098).
- **Tailwind and CSS-in-JS in v1** (§15 D1098).
- **Code layers, and importing HTML/CSS or framework code as layers** — too complex fully, messy partly; SVG import is untouched (§15 D1099).
- **The code fidelity check in the app** — it needs an external browser and cannot check React or Vue; `ondin check code` stays the way to run it.
- **Live interaction states in present mode** — hover and press on the canvas are prototyping; the states exist for code export.

## Charts

- **Chart kinds past the seven** (radar, heatmap, treemap, funnel, gauge, candlestick, maps) — a flattened chart or plain shapes draw them (`charts.md`).
- **Sankey loops** (flows back to a node already passed) — refused with the loop named; a loop needs ribbons that turn backwards.
- **Combo charts and a second value axis** — two charts stacked in one frame do it without a dual-scale design.
- **A time scale** — the category axis is categorical; dates read as labels in table order.
- **Editing one part of a live chart** — series and value options cover the tweaks; past them, flatten.
- **A chart inside a component, an instance or a kit** — a resized instance would regenerate parts its main lacks; flatten first.
- **Data from a URL, formulas in the table, and data bound to variables** — the table is values, from typing, pasting or a local file.
- **Interactivity** (hover readouts, tooltips, animation) — a chart is a picture of its data, on the canvas and in code export.
- **A chart's own title or caption** — a text layer beside it does that.
- **Saved user presets** — duplicate a styled chart and replace its data.
- **Refreshing a linked file without a click** — an opened document's link can name any file on the machine (`charts.md`, *Import and the linked file*).

## Tests and goldens

- **`f32_pipeline` for `OptimizeQuality`, and keeping strokes out of the PNG fixture** (§15 D304).
