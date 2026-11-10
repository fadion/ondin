# Charts

A chart is a frame that carries a chart description and whose children are ordinary shapes and
text generated from it: columns, bars, lines, areas, pies, scatters and Sankey flows drawn from
a table typed in, pasted or imported from a CSV or JSON file. The parts are laid out with the
container layout, so a chart resizes as a layout does, live or flattened. The chart stays editable through the options it
was made with until it is flattened, which leaves the shapes and drops the description. Built so
far: the model, settle, and the generator for every kind; the rest is in `roadmap.md`, *Now ·
Charts*.

## Model

### A chart is a frame with a description

- **`Node::chart: Option<Box<chart::Chart>>`**, set only on an `Artboard`. It is a flag on a
  frame rather than a `NodeKind`, for the reason components are (`components.md`): a new kind
  answers `false` to every kind predicate (`clips_children`, `takes_paint`, `steps_into`, …)
  nobody added it to, and nothing fails to compile.
- **A frame, not a group.** A chart has a size of its own that its contents are laid out into;
  a group's box is derived from its contents, and resizing one scales its children, which would
  stretch the type. The frame also gives the chart its ground (the frame's fills), corner radii,
  clip and a place as a layout item, so a chart in a flex card can fill its width.
- **The children are the chart's parts**, real nodes: `Rect`s, `Path`s, `Ellipse`s and `Text`s
  inside named frames (*Laid out, not placed*, below). Every renderer, writer, the snapshot, the clipboard, the contrast
  check and the vision simulation draw them as they draw any layer, with no per-chart code.
- **The parts are derived, and nothing else writes them.** A regeneration is a transaction
  carrying `SetChartParts` for its chart (the parts by role, and the mark that it ran).
  `chart::guard` judges every op of every transaction against the tree it meets and refuses one
  that writes a node under a chart frame, adds or moves a child into one, or sets the frame's
  `display`, unless the transaction regenerates that chart: `OpError::ChartPart`, said as *Part of
  a chart — edit the chart, or flatten it first*. An agent setting a bar's colour gets the refusal
  rather than a colour the next commit overwrites. The chart frame's own fields, and creating,
  moving, copying or deleting it whole, are not part edits.
- **`SetChart`** writes the whole description (`None` flattens), refused off a frame and for a
  description `Chart::validate` refuses: the table's limits and shape, a mapping naming columns the
  table has, every option finite and in range. The loader holds a file to the same rule and refuses
  it rather than loading the parts as plain shapes.
- **Where a chart may sit.** Anywhere a frame may, except inside a main component, an instance,
  a kit main, a boolean or a mask (*A chart can't be part of a component — flatten it first*;
  `chart::check`, over the whole document after every transaction and on load). An
  instance resized away from its main would regenerate parts its main does not have, and a linked
  copy has no meaning for a part that is rebuilt. A chart frame's own `display`, and the layout
  of everything inside it, are written only by its regeneration. It sets no variable modes or
  width queries, is not a component set, and holds no guides or layout grids. As an item in its
  parent's layout it is a frame like any other: it can fill, grow, stretch or hold a ratio.

### The description (`chart::Chart`)

- **`kind`** — `Column`, `Bar`, `Line`, `Area`, `Pie`, `Scatter`, `Sankey` (*Chart types*,
  below). The variant lives in the options of the kind's family, so switching kind keeps it:
  `bars.stacking` (Grouped, Stacked, 100%), `lines.curve` (Straight, Smooth, Step),
  `lines.area_stacking` (Overlapping, Stacked, 100%), `slices.donut`, and a scatter is a bubble
  chart while its mapping has a size column.
- **`table`** — `Table { columns: Vec<Column { id, name }>, rows: Vec<Vec<String>> }`: every cell
  is text, one per column, and a column is a number column while every non-empty cell parses as
  one (`chart::parse_number`).
- **`mapping`** — which column is the category (or, for a scatter, X), which columns are series,
  for a scatter the optional size and group columns, and for a Sankey the *From*, *To* and
  *Value* columns. Columns are named by `ColumnId`, so a rename keeps the mapping.
- **`series: Vec<SeriesStyle>`** — one per series column, keyed by its `ColumnId`: colour (a
  `BrushSource`, so a colour variable binds to it through the existing binding chrome), visible,
  and the per-series line dash and point shape. A column mapped as a series for the first time
  takes the palette's next colour. **Bindings wait for the Chart card**: until the variable flows
  (`variable::reads`, `remove_ops`' detach, `detached`, a kit's rebind) know a chart's colours,
  `Chart::validate` refuses a bound one, so a delete can never leave one dangling.
- **`nodes: Vec<NodeStyle>`** — a Sankey's node colours, keyed by node name, so a
  re-import or a reorder keeps them. A name seen for the first time takes the palette's next
  colour; a style whose name no row uses any more is dropped by the next regeneration.
- **`options`** — every kind's options at once (*Options*, below), so switching kind and back
  keeps what was set.
- **`source: Option<chart::DataLink>`** — the linked file, if any (*Import and the linked file*).
- **`parts: Vec<(PartKey, NodeId)>`** — the generated nodes by role (`PartKey { role, series,
  index }`), ids saved in the file's wire spelling. Never shown. A copy's keys follow its new ids
  (`document::remap_subtree`).

### Regeneration (`chart::generate`, `chart::settle`)

- **`chart::generate(&Chart, size, family) -> Result<Generated, Refusal>`** is pure: it lays the
  chart out into the frame's size and returns the frame's own `display` and each part's key,
  parent key, kind, geometry, transform, paint, name and layout (insets, display, item settings),
  parents first, plus the values it left out with their reasons. Every part carries both its place
  at this size and the layout that keeps it there, so the parts draw right before any layout runs.
  `family` stands in when the chart names none. It lives in core so MCP, the CLI and Command Mode
  make the same chart. Label widths come from the same parley layout a text node uses. A chart
  past its limits is a `Refusal` (*Too many marks*), and settle keeps the parts it has.
- **`chart::settle`** runs after layout resolves, as `Document::settle_widths` picks modes by
  width: **the parts are derived state, written outside history**. After any commit, undo, redo
  or load that sets a chart's description or changes the size the frame is drawn at — its own
  resize, a parent's layout giving it a new used width, a font arriving — settle regenerates it in
  a transaction of its own carrying `SetChartParts`, applied to the document directly. The undo
  step holds only the edit; undoing it changes the description back and settle follows. That is
  why `SetChart` keeps a chart's own parts: an undone description must not put back a parts list a
  later regeneration replaced. Settle reads the **used** size from `Resolved`, because a chart
  filling a flex card is drawn at the card's width, not its stored one, and it owns the frame's
  whole subtree: a descendant no part key names is deleted. A chart is stale (`chart::stale`)
  while `Chart::generated`, the size `SetChartParts` stamped, differs from its drawn size; a
  changed description clears the stamp. `Resolved::settle` runs it after the width picks (so a
  load, an export and compare settle as the app does) and the session after every commit, undo
  and redo; new parts take ids under an actor of their own, past every id the document holds.
- **Ids are kept by role.** Settle diffs the new parts against `parts` by `PartKey` (`role`,
  then the series' column id or 0, then the row, tick or node index): a kept key keeps its
  `NodeId` and gets `SetGeometry`, `SetFills`, `SetTransform`, `SetInsets` and the rest only where
  it changed, and a move or a reorder where its place did; a text whose kind changes is made
  again under the same id; a new key is created with a minted id; a missing one is deleted.
  Compare and the snapshot see the change that was made, not the whole chart rebuilt.
- **Previews.** A resize drag needs nothing of its own: the layout reflows the parts as the frame
  is dragged (*Laid out, not placed*), and the release regenerates for the new size. Scrubbing an
  option field regenerates on every frame into `RenderOverrides::chart`, a replacement for the
  frame's parts drawn in their place, since a preview cannot otherwise add a tick or a bar.
  Nothing enters the document until release (invariant 5).

### Laid out, not placed

The parts are built on the container layout (`layout.md`), so the chart reflows the moment its
size changes — during a drag, when a flex card grows, after a flatten, and in code export —
and regeneration only has to redo what layout cannot: the tick values, which labels are thinned
or turned, which value labels fit.

- **The structure.** The chart frame is a flex column with the chart's padding and gap: *Legend*
  (a frame 100% wide and `fit-content` tall, a flex row that wraps, so a narrow chart puts its
  legend on two lines; each *Legend · ‹series›* a `fit-content` flex row of swatch and label) and
  *Body* (a frame 100% across, `flex-grow: 1` from a basis of 0, a grid). The Body's grid gives
  each axis a px track as thick as its labels, which only regeneration changes, and the plot
  `minmax(0, 1fr)` both ways: a bare `1fr` keeps the plot's stored size as its least, so a chart
  made smaller would not shrink its plot. Its cells, each 100% of its area, are *Value axis*
  (tick labels), *Plot*, and *Category axis* under the plot; a bar chart swaps the two axes'
  places, and an axis's *side* moves its track to the other edge. A legend placed left or right
  makes the chart a flex row.
- **Inside the plot, marks are pinned by percentage insets** of the plot (`container::Insets`,
  `LengthPct::Percent`). A column's left and right insets are its slot's edges in the band, its
  top inset its value, its bottom inset the baseline, so a wider or taller plot stretches the
  column and keeps it on its value. A gridline, and the baseline at zero, is a 1 px `Rect` centred
  on its value and pinned between left 0 and right 0. A point, a tick label and a category label
  are centred on their percentage by both insets and auto margins (layout's absolute-centring),
  so they keep their size while their place follows the plot; a tick label is pinned the axis gap
  from the plot on the other axis, a turned category label as deep as its turned box reaches. A
  pie or donut sits in a square frame (`aspect-ratio: 1`) centred in the plot.
- **Value labels** sit outside, inside or at the end of their bar. In a stack only the outermost
  segment each way takes an outside label; the segments it covers take theirs inside, and a label
  that does not fit inside is left out. A value label writes as many places as the value has, up
  to three; a 100% chart's labels and ticks are whole percentages of each category's total.
- **Every container is a frame**, unpainted and unclipped, never a group, because a group takes
  no insets. The tree, bottom to top: *Legend*, then in the Body *Value axis*, *Category axis*
  and *Plot*; in the Plot, *Ground* (a `Rect`, absent when unpainted), *Grid*, one *Series
  ‹name›* per series (its bars, its line and area, its points, its slices; a Sankey's *Flows* and
  *Nodes*), and *Values*. Part names say what they are: *Revenue · Mar*, *Tick 200*, *Legend ·
  Revenue*. The live chart and the flattened one have the same tree.
- **Lines, areas, slices and flows are paths pinned on both edges** of the plot (or of the pie's
  square), which stretch with it, their stroke width unscaled (`layout.md`, *What stretches*).
  Each is pinned by its own box's place as percentages of the plot, so a resized line lands point
  for point where regeneration at the new size would draw it.
- **What a flattened chart keeps.** Resized after a flatten, it reflows exactly as the live chart
  does during a drag: columns stretch, labels stay their size and follow their places, the
  legend wraps. What it no longer does is regenerate, so its tick values and the labels shown stay
  as they were at the flatten.

## Chart types

Seven kinds, each with its variants. The category axis is always categorical: labels are spaced
evenly in table order, as typed, so a date column reads as text and no time scale is inferred.

| Kind | Variants | Reads |
| --- | --- | --- |
| **Column** | Grouped, Stacked, 100% | category + 1 or more series; vertical bars |
| **Bar** | Grouped, Stacked, 100% | the same, horizontal; categories top to bottom |
| **Line** | Straight, Smooth, Step | category + 1 or more series |
| **Area** | Overlapping, Stacked, 100% | category + 1 or more series; Straight or Smooth |
| **Pie** | Pie, Donut | category + one series; the donut's hole is an option, so a one-row donut is a progress ring |
| **Scatter** | Scatter, Bubble | X + Y number columns; Bubble adds a size column; an optional group column splits the points into series |
| **Sankey** | — | From + To text columns and a Value column: one row per flow between two named nodes |

- **Smooth** is a monotone cubic (Fritsch–Carlson), so a smoothed line never overshoots a peak
  the data does not have. **Step** turns halfway between points. A line's points sit at the
  centres of the category bands, as columns do.
- **Lines and areas** are one series frame each: the area (a closed path down to the baseline or
  to the stack beneath it, at the area opacity), the line over it (round joins and caps, the
  series' dash), and the points (ellipses centred on their values; *Hollow* is white with the
  series' stroke). A line's value labels sit above their point. A line's axis starts at zero only
  while *Start at zero* is on; an area's always holds zero.
- **A scatter** reads its category column as X, numbers on a tick axis like the value axis (a row
  whose X is not a number is left out and said), and each series column as a Y series; with a
  group column, the first series split into one series per group value, coloured through the
  palette in first-seen order. With a size column it is a bubble chart: a bubble's side is the
  largest size times the square root of its share of the largest value, so its area follows the
  value, and a row with no size is left out. Points are circles, squares, or diamond and triangle
  paths, centred on their place at their own size. Vertical gridlines follow the category axis's
  *Gridlines*. *Start at zero* is off by default on every axis; columns, bars and areas hold zero
  whatever it says.
- **A pie** reads the first series by category, each slice its palette colour by category, and
  its legend lists the slices. A zero is not drawn; *Group small slices* folds two or more slices
  under its share into *Other* (the palette's last colour). The pie sits in a square frame
  centred in the body at its own size, so it stays round as the chart resizes; each slice is a
  wedge path pinned by its own box. Labels sit two thirds out on a pie, mid-ring on a donut, or
  outside on a leader line, the pie shrunk to leave them room. **One slice of at most 100 is a
  progress ring**: its share of 100 drawn over a *Track* in the value gridline colour.
- **Values the kind cannot draw are left out, and said.** Negative values in a pie, text in a
  series column, a row with an empty category, a Sankey row whose value is not above zero or whose
  From is its To: the card's data line says *2 values left out* with the reasons in its tooltip. Column, bar and area charts draw negatives below the baseline;
  stacked variants stack positives and negatives apart.
- **Empty cells**: a bar is not drawn; a line breaks, unless *Connect gaps* is on.
- **Limits.** A table holds at most 10,000 rows and 64 columns, a cell at most 256 characters.
  A chart draws at most 2,000 marks (bars, points, slices); past that it draws nothing new and the
  card says *Too many marks: 3,400 bars, at most 2,000*. A line or area is one path per series and
  takes all 10,000 rows. A pie draws at most 100 slices, and *Group small slices* folds those under
  a percentage into *Other*. A Sankey draws at most 200 nodes and 1,000 flows.

### Sankey

- **The data is a list of flows**, not categories by series: *From*, *To*, *Value* per row
  (`Search, Signup, 1200`). Rows with the same From and To add up. A node's value is the larger
  of what flows in and what flows out, so a node that leaks (more in than out) shows the leak as
  the unfilled end of its bar.
- **Columns by depth.** A node with nothing flowing in is in the first column; every other node
  is one column past the furthest node flowing into it. *Alignment* then moves nodes: *Justify*
  (the default) sends every node with nothing flowing out to the last column, *Left* leaves them
  where depth put them, *Right* counts depth from the end, *Centre* moves a node with nothing
  flowing in next to the first node it feeds.
- **Order within a column** starts in table order (the first row naming a node places it) and is
  improved by six relaxation passes (`RELAX_PASSES`), alternately left to right toward what flows
  in and right to left toward what flows out, each moving nodes toward the flow-weighted centre
  of what they connect to and then pushing overlaps apart down and back up from the bottom. A
  fixed count, not a convergence test, so the same table always draws the same chart. *Order:
  Table order* skips the passes. One height scale serves every column, so a value is as tall
  everywhere; a node's place and height are pinned in percent of the body and its width stays.
- **Flows** are ribbons, each a filled path between two cubic curves, as thick as its value,
  stacked on each node in the order of the nodes at their other ends so ribbons cross as little
  as they can.
- **Loops are refused**: a table where flows lead back to a node already passed draws nothing,
  and the card says *Flows loop back: Signup → Search → Signup*. Drawing a loop needs ribbons that
  turn backwards, a different picture (`non-goals.md`).
- **Colours belong to nodes.** The Series section is *Nodes* for a Sankey: one row per node name
  with its swatch, bindable like a series'. A flow is coloured by its *Source* node (the default),
  its *Target*, a *Gradient* between the two (a linear-gradient fill across the ribbon), or one
  *Neutral* colour (the value axis's colour), at the flow opacity. *Gradient* is drawn in the
  source's colour until the generator writes gradient fills (`roadmap.md`).
- **Labels** are the node's name, or name and value, beside the node: outside for the first and
  last columns, on the side away from the chart's middle for the rest. There is no legend and
  there are no axes.
- **Double-click** on a node selects the first row naming it; on a flow, its row.

## Options

All live in the chart's own inspector card (*The Chart card*). Each is shown only for the kinds
it applies to; switching kind keeps the others' values.

- **Style presets.** A preset is a set of option values written over the description in one step,
  not a link: *Clean* (the default), *Soft* (rounded marks, light dashed grid), *Bold* (heavy marks,
  no grid, large values), *Minimal* (no axes, no grid, no legend: a sparkline when the chart is
  small), *Outline* (stroked marks, no fill). The preset cells are drawn by `chart::generate`
  itself on the chart's own data, so what a cell shows is what it does.
- **Palette** — *Ondin* (eight colours, distinct under the vision simulation's three dichromacies),
  *Cool*, *Warm*, *Mono* (one hue in steps), or *From variables*: a colour collection's group, its
  variables bound to the series in order, so a mode switch recolours the chart. Picking a palette
  recolours every series; a series' own swatch, once edited, is its own.
- **Marks**:
  - *Bars* (Column, Bar): group gap (% of a band), bar gap (% of a bar), corner radius on the end
    away from the baseline or all corners, stroke.
  - *Lines* (Line, Area): width, curve (Straight, Smooth, Step), points (None, Hollow, Filled) and
    their size, per-series dash, area opacity, *Connect gaps*.
  - *Slices* (Pie): hole (% of the radius, 0 for a pie), pad angle, corner radius, start angle,
    *Sort by value*, *Group small slices* below a percentage.
  - *Points* (Scatter): size, shape (Circle, Square, Diamond, Triangle), opacity, the bubble's
    largest size.
  - *Flows* (Sankey): node width, node gap, node corner radius, alignment (Justify, Left, Right,
    Centre), order (Automatic, Table order), flow colour (Source, Target, Gradient, Neutral),
    flow opacity, labels (Name, Name and value, Off).
- **Axes** (not Pie or Sankey): per axis, *Show*, its line, its ticks, its labels and their angle (Auto,
  0°, 45°, 90°; Auto turns them 45° and then thins them to every *n*th when they would touch),
  a title, and for a value axis its range (Auto or a typed min and max), *Start at zero* (always
  on for columns and bars, whose length is the value), the tick count (Auto aims for five on
  1-2-5 steps), side (left or right, top or bottom) and number format. Gridlines per axis: on or
  off, solid or dashed, colour.
- **Values** (value labels): Off, Inside, Outside, End; a label that does not fit inside its mark
  is not drawn. Pie labels: the value, the percentage or the category, inside or outside with a
  leader line.
- **Legend** (not Sankey): Off, Top, Bottom, Left, Right, with its alignment; a one-series chart
  starts with it off.
- **Number format** (one per value axis and one for values): decimals, thousands separator,
  prefix and suffix (`$`, `%`, ` kg`), *Compact* (1.2k, 3.4M), *Percent* (×100 with `%`).
- **Text**: one family and weight for the chart, a size each for tick labels, values, legend and
  axis titles, and the ink colour, from the Type controls the inspector already has. A chart starts
  with the document's default text family, as the Text tool's first node does.
- **Colours** for axes, ticks, grid and text are `Paint` fills like the series', so they bind to
  variables the same way.

A chart has no title or caption of its own: a text layer next to it does that, and stays free.

## Chrome

### Making one

- **The tool rail's Chart slot**, after Place image: a grouped slot (`rail::Group`) whose members
  are the seven kinds, holding the last kind picked (`Rail::chart`, `prefs.view.rail_chart`). Key
  `J`, the chart tool's letter in Illustrator; it arms the slot's shown member. Each member wears
  its kind's glyph.
- **The gesture** is the frame tool's: drag a box with the create box and size pill, or click for
  a 480 × 280 chart at the click. The new chart has the kind's sample table (*Category*, three
  series, six rows: *Jan*–*Jun*; a Sankey's *From*, *To*, *Value* with eight flows over three
  columns), the *Clean* preset and the *Ondin* palette, and the data window
  opens on it with the first cell selected, so typing or pasting replaces the sample. Undo step
  *Insert chart*.
- **Other doors**: the palette's *Insert chart* (aliases *chart*, *graph*, *plot*), which arms
  the tool; a `.csv` or `.tsv` dropped on the canvas, which makes a chart at the drop from the file
  (*Import*, below): a Scatter when the file's first column is numeric, a Sankey when it has
  exactly two text columns followed by one number column, else a Column chart.

### On the canvas and in the layers tree

- **A chart selects as one layer.** A click on any part selects the chart frame;
  `query::steps_into` does not step into it, and the marquee and the lasso take the frame whole.
  The parts are not hit-tested on their own.
- **Double-click opens the data window** with the cell behind the clicked mark selected (the
  bar's value, the slice's value, the point's row); on a tick label or the legend it selects that
  column's header.
- **Resize** is the frame's: the chart lays itself out again into the new size, its type staying
  the size it is.
- **The layers tree** shows a chart as one row with the chart glyph and no disclosure; its parts
  are not listed. After a flatten the row is a frame and its tree opens.

### The Chart card

The chart's inspector card, in the Type card's place, so a chart frame shows identity, Align,
Transform, Position, Item, **Chart**, Appearance (the frame's ground and opacity), Stroke,
Effects, Export. A frame's *Frame templates* and *Layout grid* cards and its Container card are not
shown. Built from the inspector's existing primitives; its sections are `ui::eyebrow` heads in the
order below, each closable as the inspector's other sections are:

- **Type** — a full-width `ComboBox` with the kind's glyph and name (`ui::combo_chevron`,
  `ui::menu_rows`, each row its glyph), then the variants on `ui::segmented`. Undo step *Chart
  type*.
- **Style** — the five preset cells in a row, `inspector::template_cell`'s cell, each the chart
  drawn small. Undo step *Chart style*.
- **Data** — the summary (*3 series · 12 rows*), *Edit data…* (`ui::field_button`), the source
  line when linked (*Linked to a file*, below), and the mapping: *Category* (or *X* and *Y*) as a
  dropdown of columns.
- **Series** — one row per series column: swatch (the fill picker, bindable), name (read from the
  column header, renamed here or there alike), the eye, and for lines its dash. The palette
  dropdown sits in the head. A Sankey shows *Nodes* here instead: one row per node name, its
  swatch and its name, the total flowing through it at the row's end.
- **Marks**, **Axes**, **Values**, **Legend** — the options above, as `ui::value_field*`,
  `ui::switch_row`, `ui::segmented` and dropdown rows, two to a line where they pair (min and max,
  size and weight). Each field's undo step is its label (*Bar radius*, *Legend position*).
- **Text** — family, weight and the four sizes, as the Type card's rows.
- At the foot, **Flatten chart** (`ui::field_button`), with `Ctrl+E` in its tooltip.

A multi-selection of charts shows the Chart card when every chart is the same kind, writing each;
the data rows are left out there (*Edit data one chart at a time*).

### The data window (`chart_data.rs`)

A floating card over the bottom of the canvas column, non-modal, so the chart redraws on the
canvas as cells are committed. 640 wide (the canvas column's width less 32 when narrower), 320
tall, fixed: the grid scrolls inside it rather than the card growing (egui popups shrink but never
grow back). One at a time, for the selected chart; selecting another chart moves it there, and
selecting anything else closes it.

- **Head**: the table glyph, *Chart data*, the chart's name, *Import…*, ✕. Under it the toolbar:
  *Add row*, *Add column*, and the source line when linked.
- **The grid**: a header row of column names, each with its type tag (`Aa` text, `#` number) and
  its role (*Category*, a series' swatch, or dim when unmapped); a row-number gutter; cells in
  `ui::text_field`'s ground at `ui::MENU_ROW_H`, numbers right-aligned. A column is a number
  column while every non-empty cell in it parses as one; the type is read, never set.
- **Keyboard**, the spreadsheet's: arrows move; typing starts an edit replacing the cell, `F2` or
  `Enter` edits it in place; `Enter` commits and moves down, `Tab` commits and moves right,
  `Shift` the other way; `Escape` abandons an edit, and a second closes the window; `Delete`
  clears the selected cells; `Shift`+arrows and a drag select a range. `Ctrl+C` / `Ctrl+X` /
  `Ctrl+V` read and write tab-separated text, so a range pasted from a spreadsheet lands as cells,
  growing the table to fit. `Ctrl+Z` is the document's undo.
- **Commits**: each committed cell, paste, clear, insert or delete is one undo step, *Edit chart
  data*. A cell being typed into is a `ui::Draft`, dropped if the chart is deselected mid-edit.
- **Header menu** (right-click or the header's chevron, on `ui::menu_frame`): *Rename*, *Use as
  category* / *Use as X*, *Show as series* (a switch), *Insert column left* / *right*, *Sort
  ascending* / *descending*, *Delete column*. **Row menu** on the gutter: *Insert row above* /
  *below*, *Delete row*.
- **Linked**, the grid is read-only and says so in a line over it (*Edited in sales.csv — unlink
  to edit here*); the mapping, series and options stay editable.

### Import and the linked file

- **Doors**: *Import…* in the data window's head; the palette's *Import chart data…* (dim with
  *Select a chart* without one); a `.csv`, `.tsv` or `.json` dropped onto a chart. A `.json`
  dropped anywhere but on a chart stays the tokens import's door.
- **CSV** (RFC 4180): the delimiter is detected among comma, semicolon and tab; the first row is
  the header unless it parses as numbers. **TSV** the same with tab. **JSON** is an array of
  objects; every key seen becomes a column, in first-seen order; numbers are numbers, strings are
  text, `true`/`false` are text, `null` is empty, and a nested object or array leaves its cell
  empty and is named in the note. Anything else is refused: *Not an array of objects*. Numbers
  written with a thousands separator (`1,234.5`) parse; a decimal comma does not.
- **The import box**, batch rename's modal: the import glyph, *Import chart data*, the file name
  and its two parent folders; the first 20 rows as they will land, each column's type tag; *First
  row is the header* (a switch, CSV only); an info note for anything left out; foot *Keep this file
  linked*, *Cancel*, *Replace data*. Replacing keeps the mapping and series colours for every
  column whose name the new table also has. Undo step *Import chart data*.
- **A linked chart** stores `DataLink { path, stamp }` (path, size and modified time as last read)
  in its own description; the table is always the document's own copy, so a chart whose file is
  gone still draws. **The path is the document's, so untrusted**: the tokens link's rules hold
  (`styles-variables.md`, *A linked file*) — absolute, on a local drive, a regular file of at most
  16 MB, read on a worker.
- **Watching**, as the tokens link: every `WATCH_EVERY` (45 s) and when the window regains focus,
  each linked path is `stat`ed. A change shows the pill *sales.csv changed · Update charts* (with
  the count when several charts link it) beside the tokens and kit pills, and the Data section's
  source line says *Changed on disk* with *Update*. Updating reads the file and replaces each
  linked chart's table in one undo step, *Update chart data*. **A file's contents enter the
  document only on the user's click**, never on its own: an opened document's link can name any
  file on the machine, and the next save would carry what it read.
- **A missing file** shows *File not found* on the source line with *Relink…* and *Unlink*.
  *Unlink* drops the link and makes the grid editable; undo step *Unlink data file*.
- **Copying a chart drops its link.** The clipboard carries the description and table but not the
  path, for the reason the clipboard refuses linked images: pasted content must not make the
  receiving document read a file the copy chose.

### Flatten

- **`chart::flatten`**: the frame keeps everything it is, its parts stay where they are, and the
  description goes: one `SetChart { chart: None }`, undo step *Flatten chart*. What was on screen
  is what remains; the labels are text layers that can be edited, restyled or converted to paths.
  The frame then behaves as any frame: its tree opens, the Frame cards return, and *Frame* /
  *Group* converts it.
- **Doors**: the Chart card's foot; `Ctrl+E`, which flattens a selected chart as it flattens a
  boolean (`app-booleans.md`); the context menu; the palette's *Flatten chart*. A selection of
  charts flattens each.

### Context menu

On a chart (`context-menus.md`): *Edit data…*, *Import chart data…*, *Update chart data* (while
its file changed), *Unlink data file* (while linked), a separator, *Flatten chart*, then the
frame's usual rows.

## Elsewhere

- **Save format**: `chart` on the frame's DTO, the table as a column list and an array of rows,
  in v15 (`io.md`, the ladder). v15 had not shipped, so no bump of its own: a v14 reader already
  refuses the file by its version rather than loading the parts as plain shapes and dropping the
  description on its next save.
- **MCP snapshot**: a chart frame carries `chart` (kind, variant, mapping, series, the table's
  header and row count) beside its parts, each part with its `role`; `snapshot_version` bumps.
  The verbs `insert_chart`, `set_chart`, `set_chart_data` and `flatten_chart` join the registry
  (`ondin_core::verbs`).
- **Code export** writes a chart as the frames it is: its flex and grid containers as CSS, its
  marks positioned by their percentage insets, its paths as stretching inline `<svg>`s (*Laid
  out, not placed*), so the exported chart resizes in a browser as it does on the canvas. The
  chart frame gets `role="img"` and an `aria-label` of its name, and its parts are
  `aria-hidden`.
- **Compare** marks the chart frame, not its parts, as changed, with *Chart data changed* or
  *Chart options changed* in the changes card.
- **SVG, PDF, PNG, Penpot and Sketch** need nothing: the parts are ordinary layers.

## Icons

New glyphs, on the *Drawing rules* (`app-chrome.md`): `CHART` (three ascending columns on a
baseline; the slot, the layers row, the palette), and one per kind — `CHART_COLUMN`, `CHART_BAR`,
`CHART_LINE`, `CHART_AREA`, `CHART_DONUT`, `CHART_SCATTER`, `CHART_SANKEY` (one node bar whose
two flows part to two bars) — plus `TABLE` for the data window and *Edit data…*. `CHART_PIE_SLICE` already means an arc's sweep and is not reused for the Pie kind;
`CHART_DONUT` is a ring with a gap, drawn to read apart from it. Column and `CHART` differ: the kind
is three columns of mixed height, the tool three ascending.

## Words

*Chart*, never *graph*, in every string; *Chart data* for the table; *series*, *category*,
*value*; *Flatten chart*; *Update chart data*, *Unlink data file*. Status line: *Imported
sales.csv · 12 rows, 3 series*; *Updated 2 charts from sales.csv*; refusals in the shared refused
look with their sentence.
