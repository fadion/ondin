# Charts

A chart is a frame that carries a chart description and whose children are ordinary shapes and
text generated from it: columns, bars, lines, areas, pies, scatters and Sankey flows drawn from
a table typed in, pasted or imported from a CSV or JSON file. The parts are laid out with the
container layout, so a chart resizes as a layout does, live or flattened. The chart stays editable through the options it
was made with until it is flattened, which leaves the shapes and drops the description. Built:
everything below.

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
  takes the palette's next colour.
- **A chart's colours bind** (`Chart::brushes`: series, nodes, the axes' and gridlines' colours
  and the text ink). The parts are generated with the bound brush itself, so they read the
  variable as any layer does and a mode switch recolours them with no regeneration; an outlined
  mark's tint is the binding at 14% of its alpha. To the variable flows the **chart frame is
  the user**, its parts never: `variable::reads`, `users` and `users_by_field` count the frame
  once; `bound_in` lists the description's colours as fills, so the scope is judged on the
  description, and `variable::check` skips the scope (not the kind) of a chart's listed parts,
  which may draw a fill colour as a line's stroke. A delete or a scope that drops the binding
  (`detach_ops`) detaches the description and its parts to the colour they drew through
  `chart::rebind_ops`: one `SetChart`, the same rewrite on the parts' fills and strokes, and a
  `SetChartParts` of the parts as they are, so `chart::guard` reads it as the chart's
  regeneration. A copy carries the description's variables (`variable::carried`); a paste
  rebinds them by the fill's rule and clears the stamp so settle remakes the parts
  (`variable::land`), and a copy leaving its document detaches them (`detached`). A kit's rebind
  never meets a chart, which cannot sit in a main. *Bind other uses* and Colour review leave a
  chart's parts out, as no edit but the regeneration may write them.
- **`nodes: Vec<NodeStyle>`** — a Sankey's node colours, keyed by node name, so a
  re-import or a reorder keeps them. A node with no style draws the palette's colour at its
  place in node order; editing or binding its swatch stores one (`chart::with_node_fill`). A
  style whose name no row uses stays, drawing nothing, and applies again if the name returns;
  only picking a palette clears them (`chart::with_palette`).
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
  make the same chart. Label widths come from the same parley layout a text node uses. A plot
  the labels leave no length still draws finite geometry, its lengths floored above zero. A chart
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
  and redo. New parts take ids under an actor derived from the chart frame's id
  (`settle::part_actor`), past every id of that actor the document holds, so settle needs no
  session's id source. A deleted chart's part ids live on in its undo step; only that chart
  mints under their actor, and it is absent while they are, so no other chart re-mints them.
- **Ids are kept by role.** Settle diffs the new parts against `parts` by `PartKey` (`role`,
  then the series' column id or 0, then the row, tick or node index): a kept key keeps its
  `NodeId` and gets `SetGeometry`, `SetFills`, `SetTransform`, `SetInsets` and the rest only where
  it changed, and a move or a reorder where its place did. A part whose kind no geometry patch
  reaches (a text whose content changes, a `Rect` that becomes a `Path`) is deleted and made
  again under a new id; a new key is created with a minted id; a missing one is deleted.
  Compare and the snapshot see the change that was made, not the whole chart rebuilt.
- **A part keeps its id only while its old parent is the chart or is itself kept**, repeated
  until nothing changes: the delete walk takes out every parent no key keeps, and its subtree
  with it, so a kept part under one would be moved out of a deleted node. Such a part is made
  again under a new id.
- **Previews.** A resize drag needs nothing of its own: the layout reflows the parts as the frame
  is dragged (*Laid out, not placed*), and the release regenerates for the new size. Scrubbing an
  option field regenerates on every frame (`chart::preview`) into
  `RenderOverrides::replace_chart`, the new parts drawn as ghosts in place of the frame's own,
  since a patch cannot add a tick or a bar. Nothing enters the document until release
  (invariant 5).

### Laid out, not placed

The parts are built on the container layout (`layout.md`), so the chart reflows the moment its
size changes — during a drag, when a flex card grows, after a flatten, and in code export —
and regeneration only has to redo what layout cannot: the tick values, which labels are thinned
or turned, which value labels fit.

- **The structure.** The chart frame is a flex column with the chart's padding and gap: *Legend*
  (a frame 100% wide and `fit-content` tall, a flex row that wraps, so a narrow chart puts its
  legend on two lines; each *Legend · ‹series›* a `fit-content` flex row of swatch and label) and
  *Body* (a frame 100% across, `flex-grow: 1` from a basis of 0, a grid). The Body's grid gives
  each axis a px track as thick as its labels and tick marks, which only regeneration changes,
  an axis with a title a second px track past it as thick as the title, and the plot
  `minmax(0, 1fr)` both ways: a bare `1fr` keeps the plot's stored size as its least, so a chart
  made smaller would not shrink its plot. Its cells, each 100% of its area, are *Value axis*
  (tick labels, tick marks and title, spanning the title's track), *Plot*, and *Category axis*
  under the plot; a bar chart swaps the two axes' places, and an axis's *side* moves its tracks
  to the other edge. A title is centred along its axis at the frame's far edge, a side axis's
  turned a quarter to read upwards. A legend placed left or right makes the chart a flex row.
- **Inside the plot, marks are pinned by percentage insets** of the plot (`container::Insets`,
  `LengthPct::Percent`). A column's left and right insets are its slot's edges in the band, its
  top inset its value, its bottom inset the baseline, so a wider or taller plot stretches the
  column and keeps it on its value. A gridline, and an axis line, is a 1 px `Rect` centred on its
  value and pinned between left 0 and right 0; a dashed gridline is a line path with the dash on
  its stroke, pinned the same way. The category axis line is the baseline at zero, or the plot's
  edge on its axis's side when zero is out of range; the value axis line runs along the plot's
  edge on its axis's side. A point, a tick label, a tick mark and a category label are centred on
  their percentage by both insets and auto margins (layout's absolute-centring), so they keep
  their size while their place follows the plot; a tick mark (4 px) is pinned against the plot,
  a tick label the axis gap past the tick marks on the other axis, a turned category label as
  deep as its turned box reaches. Value tick marks sit at the ticks, category ones at every
  category's centre (a scatter's at its X ticks). A pie or donut sits in a square frame
  (`aspect-ratio: 1`) centred in the plot.
- **Value labels** sit outside, inside or at the end of their bar. In a stack only the outermost
  segment each way takes an outside or end label, and a zero, covering nothing, is never the
  outermost; the segments it covers take theirs inside, and a label that does not fit inside is
  left out. A value label writes as many places as the value has, up
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
  the data does not have; points with no width between them have no slope to take and draw
  straight. **Step** turns halfway between points. A line's points sit at the
  centres of the category bands, as columns do.
- **Lines and areas** are one series frame each: the area (a closed path down to the baseline or
  to the stack beneath it, at the area opacity), the line over it (round joins and caps, the
  series' dash), and the points (centred on their values in the series' own point shape, a
  circle when it has none; *Hollow* is white with the series' stroke). A line's value labels sit above their point. A line's axis starts at zero only
  while *Start at zero* is on; an area's always holds zero, and a 100% area's runs to 100%, from
  −100% when any value is negative, as a 100% bar's does.
- **A scatter** reads its category column as X, numbers on a tick axis like the value axis (a row
  whose X is not a number is left out and said), and each series column as a Y series; with a
  group column, the first series split into one series per group value, coloured through the
  palette in first-seen order. With a size column it is a bubble chart: a bubble's side is the
  largest size times the square root of its share of the largest value, so its area follows the
  value, and a row with no size is left out and said (*Row 4: no size*), once however many series
  it holds. Points are circles, squares, or diamond and triangle
  paths, centred on their place at their own size: a series' own shape, else the chart's (a
  group takes its series'). Vertical gridlines follow the category axis's
  *Gridlines*. *Start at zero* is off by default on every axis; columns, bars and areas hold zero
  whatever it says.
- **A pie** reads the first series by category, each slice its palette colour by category, and
  its legend lists the slices. A zero is not drawn; *Group small slices* folds two or more slices
  under its share into *Other* (the palette's last colour). The pie sits in a square frame
  centred in the body at its own size, so it stays round as the chart resizes; each slice is a
  wedge path pinned by its own box; its corner radius rounds the wedge's corners as a path's
  per-anchor radii, and the box is the rounded outline's. Labels sit two thirds out on a pie, mid-ring on a donut, or
  outside on a leader line, the pie shrunk to leave them room; a slice too thin to draw past its
  pad angle gets no label and no leader line, unless it is the only slice. **One slice of at most 100 is a
  progress ring**: its share of 100 drawn over a *Track* in the value gridline colour.
- **Values the kind cannot draw are left out, and said.** Negative values in a pie, text in a
  series column, a row with an empty category, a Sankey row whose value is not above zero or whose
  From is its To: the card's data line says *2 values left out* with the reasons in its tooltip. Column, bar and area charts draw negatives below the baseline;
  stacked variants stack positives and negatives apart.
- **Empty cells**: a bar is not drawn; a line breaks, unless *Connect gaps* is on.
- **Limits.** A table holds at most 10,000 rows and 64 columns, a cell at most 256 characters.
  A chart draws at most 2,000 marks (bars, points, slices); past that it draws nothing new and the
  card says *Too many marks: 3,400 bars, at most 2,000*. A line or area is one path per series and
  takes all 10,000 rows, but its points and its value labels are one mark per value each, so
  either shown counts (*points*, else *value labels*). A pie draws at most 100 slices, and *Group small slices* folds those under
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
  *Neutral* colour (the value axis's colour), at the flow opacity. A *Gradient* runs across the
  ribbon from the source node's edge to the target's; a node colour bound to a variable has no
  stop to give, so such a flow takes its source's paint.
- **Labels** are the node's name, or name and value, beside the node: outside for the first and
  last columns, on the side away from the chart's middle for the rest. There is no legend and
  there are no axes.
- **Double-click** on a node selects the first row naming it; on a flow, its row.

## Options

All live in the chart's own inspector card (*The Chart card*). Each is shown only for the kinds
it applies to; switching kind keeps the others' values.

- **Style presets** (`chart::Preset`). A preset is a set of option values written over the
  description's style fields in one step, not a link: *Clean* (the defaults), *Soft* (rounded
  marks and slices, filled points, a dashed value grid), *Bold* (heavy marks, no grid, large
  values outside; a Sankey's nodes 18 wide and its flows at 60%), *Minimal* (no axes, no grid,
  no legend, no Sankey labels: a sparkline when the chart is small), *Outline* (outlined marks
  at 1.5 px and lines as thin, `chart::OUTLINE_WIDTH`). Every preset changes something each
  kind draws, so no two cells of a kind look alike. A preset owns exactly the options some
  preset sets, writing *Clean*'s value into each it does not set itself (`Preset::apply`).
  Everything else is left alone: data, palette, text family, number formats, and an option no
  preset sets, so a user's bar gap survives a Style while the group gap (*Bold*) and the radius
  (*Soft*) follow it.
- **Palette** — *Ondin* (eight colours, distinct under the vision simulation's three dichromacies),
  *Cool*, *Warm*, *Mono* (one hue in steps), or *From variables*: a group of colour variables
  that may bind a fill (the name before the last `/`, else the collection's name; its variables
  in the table's order), bound to the series in order (a Sankey's nodes), round again past the
  last (`chart::with_variables`), so a mode switch recolours the chart. The palette dropdown
  lists the four palettes, then the groups under *From variables*; a group lights while the
  series run through it in order (`chart::bound_variables`), and the dropdown reads *From
  variables · ‹group›*. Picking a palette recolours and unbinds every series; a series' own
  swatch, once edited, is its own.
- **Marks**:
  - *Bars* (Column, Bar): group gap (% of a band), bar gap (% of a bar), corner radius on the end
    away from the baseline or all corners.
  - *Outline marks* (every kind, `Options::outline`): bars, slices, points and Sankey nodes drawn
    as an inside stroke of the given width in their colour over a 14% tint of it
    (`chart::OUTLINE_TINT`; a bound colour keeps its binding at that alpha). The stroke is inside
    so a mark keeps the size its value gives it. Lines are strokes already and areas keep their
    opacity; a legend swatch stays solid.
  - *Lines* (Line, Area): width, curve (Straight, Smooth, Step), points (None, Hollow, Filled) and
    their size, per-series dash, area opacity, *Connect gaps*.
  - *Slices* (Pie): hole (% of the radius, 0 for a pie), pad angle, corner radius, start angle,
    *Sort by value*, *Group small slices* below a percentage.
  - *Points* (Scatter): size, shape (Circle, Square, Diamond, Triangle), opacity, the bubble's
    largest size.
  - *Flows* (Sankey): node width, node gap, node corner radius, alignment (Justify, Left, Right,
    Centre), order (Automatic, Table order), flow colour (Source, Target, Gradient, Neutral),
    flow opacity, labels (Name, Name and value, Off).
- **Axes** (not Pie or Sankey): per axis, *Show*, its line (off by default on the value axis, whose
  gridlines carry it), its tick marks, the colour both draw in, its labels and their angle (Auto,
  0°, 45°, 90°, on either axis and either side of the plot; Auto turns category labels under
  columns 45° and then thins them to every *n*th when they would touch, and keeps every other
  axis's flat; a set angle beside the plot thins a bar chart's category labels by their turned
  height),
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
- **Colours** for axes, ticks, grid and text are `BrushSource`s like the series', so they bind to
  variables the same way, and the card edits each. A Sankey's *Neutral* flows draw in the value
  axis's colour and a progress ring's *Track* in the value gridlines', so their rows on the card
  write those two.

A chart has no title or caption of its own: a text layer next to it does that, and stays free.

## Chrome

### Making one

- **The tool rail's Chart slot**, after Place image: a grouped slot (`rail::Group`) whose members
  are the seven kinds, holding the last kind picked (`Rail::chart`, `prefs.view.rail_chart`). Key
  `J`, the chart tool's letter in Illustrator; it arms the slot's shown member. Each member wears
  its kind's glyph.
- **The gesture** is the frame tool's: drag a box with the create box and size pill, or click for
  a 480 × 280 chart at the click (`tools::CHART_CLICK_SIZE`), on a white ground, named for its
  kind (*Column chart*). The new chart has the kind's sample table (`Chart::sample`: *Category*,
  three series, six rows: *Jan*–*Jun*; a scatter's numeric *Hours* and two Y series; a Sankey's
  *From*, *To*, *Value* with eight flows over three columns), the *Clean* preset and the *Ondin*
  palette, and the data window
  opens on it with the first cell selected, so typing or pasting replaces the sample. Undo step
  *Insert chart*.
- **Other doors**: the palette's *Insert chart* (aliases *chart*, *graph*, *plot*), which arms
  the tool; a `.csv` or `.tsv` dropped on the canvas, which makes a chart at the drop from the file
  (*Import*, below): a Scatter when the file's first column is numeric, a Sankey when it has
  exactly two text columns followed by one number column, else a Column chart.

### On the canvas and in the layers tree

- **A chart selects as one layer.** `query::hit_test` answers a hit on any part with its chart
  frame, once, so a click, a hover, a double-click and `Enter` all meet the frame and never step
  inside it; the marquee and `Ctrl+A` offer a chart on the page whole (`marquee_range`), never
  its parts.
- **Double-click opens the data window** with the cell behind the clicked mark selected
  (`query::chart_part_at` finds the part, `chart_data::cell_of` its cell): a bar's, point's or
  value label's series at its row, a slice's value, a category label's row, a flow's row, a
  Sankey node's first row naming it; a line, an area or a legend entry its column's first row.
  Anything else selects the first cell.
- **Resize** is the frame's: the chart lays itself out again into the new size, its type staying
  the size it is.
- **The layers tree** shows a chart as one row with the chart glyph and no disclosure; its parts
  are not listed, matched by the layer search or the palette, and the row takes no drop. A
  chart wears `CHART` wherever a layer's glyph is shown (`layers::node_glyph`). After a flatten
  the row is a frame and its tree opens.

### The Chart card

The chart's inspector card (`panels/chart.rs`), after Item, so a chart frame shows identity,
Align, Transform, Position, Item, **Chart**, Appearance (the frame's ground and opacity), Fill,
Stroke, Effects, Export. A frame's *Frame templates*, *Layout grid* and Container cards, and the
group colours of what is inside it, are not shown (`inspector_single`'s `is_chart`). Every edit is
one `SetChart` of the whole description; a scrubbed number previews through
`EditorSession::set_preview`, which draws the parts the new description generates in place of the
chart's own (`RenderOverrides::replace_chart`: the children hidden by the child-order patch, the
new parts as ghosts) and remembers the description (`EditorSession::chart_drawn`) for the card
and the picker. Its rows follow the card rules: dropdowns `CONTROL_H` tall (`ui::combo_height`),
switch rows `settings::SWITCH_H` as Settings', and each colour row one `ui::field_row_pressable`
whose whole width opens the picker, lit while the picker serves it. The Type rows come first with
no head; every other section is headed by a `ui::fold_rule`, in the order below, their folds kept
in `collapsed_panels` (Marks, Axes, Values, Legend and Text start folded, `chart::FOLDED`):

- **Type** — always shown: a full-width `ComboBox` with the kind's glyph and name
  (`ui::combo_chevron`, `ui::menu_rows`, each row its glyph, `rail::chart_glyph`), then the
  variants on `ui::segmented`. A switch to or from a Sankey re-reads the mapping off the
  table's columns. Undo step *Chart type*.
- **Style** — the preset cells in a row, each a drawing of its preset for the chart's kind
  (`panels/chart_picture.rs`) on a 44 × 32 grid, not the chart shrunk: few, thick marks that
  change only what the preset changes. A narrow cell draws it smaller, keeping 5 px each side
  as above it so it clears the lit ring; the cells sit 2 px apart so the width goes to the
  pictures. It is rasterised at a whole-pixel size with every straight edge and hairline
  snapped to the pixel grid, and drawn at a pixel-aligned spot, so a smaller picture stays
  crisp. Donut and Bubble have drawings of their own; stacking variants
  keep their kind's. The marks wear the chart's first colours (series, a pie's first slices, a
  Sankey's first nodes; the palette fills the rest), the chrome is the theme's text ink, and
  there is no ground: a 1 px frame sits over the cell's hover and lit fills. Outline's tint is
  22% there, against the chart's 14%, which vanishes at this size. The pictures are rasterised
  through `rasterize_paths` and cached with the generator's left-out values while the
  description, its bound colours resolved, the drawn size, the display scale and the theme are
  unchanged (`ChartCardCache`), so a bound variable's new value, a mode switch or a theme
  switch redraws them (`style_pictures_follow_a_bound_variables_value`). The preset the
  options are exactly at lights. Undo step *Chart style*.
- **Data** — the summary (*3 series · 12 rows*, a Sankey's *8 flows*), *Edit data…* (an
  `ui::action_button`, tooltip *Open the chart's table over the canvas*), the source line while
  linked, the left-out line (*2
  values left out*, the reasons in its tooltip) or the refusal (*Too many marks: …*, *Flows loop
  back: …*) in the warning ink, and the mapping: *Category* (*X* for a scatter) or a Sankey's
  *From*, *To* and *Value* as dropdowns of columns.
- **Series** — the palette dropdown, then one row per column but the category: its colour row
  (swatch and name), which opens the picker on `PaintSlot::Chart` (solid colours, and the *Variables* tab: a
  binding written into the description, `chart_bind_tx`, *Solid* detaching it to the colour
  drawn), and the eye, which
  shows the column as a series or hides it (a hidden series keeps its place and colour). A bound
  colour row (a series', a node's, and the card's other colour rows) shows its variable's name
  right-aligned in a capsule (`binding::capsule`, at most half the row, `binding::chip_tip` its
  tooltip). On a
  line, area or scatter chart a `…` button between them opens the series' style menu: its line's
  dash (*Solid*, *Dashed*, *Dotted*; not a scatter) and its points' shape (a line's unshaped
  points are circles; a scatter's take the chart's shape), undo steps *Series dash* and *Series
  point shape*. A Sankey shows *Nodes* here instead: one row per node name in
  `chart::sankey_nodes`' order with its swatch and, right-aligned in the muted ink, its total
  (`chart::sankey_totals`: the larger of what flows in and out) in the values' number format,
  a bound node's capsule before it.
  Picking a palette puts every series and node back on it (`chart::with_palette`).
- **Marks**, **Axes**, **Values**, **Legend** — the options above for the kind, as
  `ui::value_field`, `ui::switch_row`, `ui::segmented` and dropdown rows, number fields two to a
  line. Each field's undo step is its label (*Bar radius*, *Legend position*). A value axis (and
  a scatter's X) has *Auto range*; turned off it starts at the extent the data already draws on
  nice steps, zero held where the kind holds it, with *Min* and *Max* fields that refuse to
  cross. The number format is one control, *Format*: a dropdown whose closed face is 12,345.6
  written through the format, opening on *Prefix* and *Suffix*, *Decimals* (Auto, 0–3), and
  *Thousands separator*, *Compact* and *Percent*, the same rows whatever is set so the popup
  keeps its size. The value axis, a scatter's X axis and the value labels each have one.
  The chart's own colours are colour rows (`chart_slot_row`, *Text colour*'s row): in Axes, per
  axis, *Value* / *Category axis colour* after its tick-marks switch while it draws a line or
  ticks, and *Value* / *Category gridline colour* after the dashed-gridlines switch while
  gridlines show; in Marks, *Neutral colour* after *Flow colour* while a Sankey's flows are
  Neutral, and *Track colour* last on a pie drawn as a progress ring (`ChartCardCache::track`,
  read off the generated parts). Undo steps *Chart axis colour*, *Chart gridline colour*,
  *Neutral flow colour* and *Track colour*.
- **Text** — the family (the Type card's family dropdown, `family_dropdown`; empty is the
  document's default family and shows its name), *Text colour* (the ink, a series row's swatch
  field on `ChartColour::Ink`, bindable the same way), the four sizes and the weight. Undo steps
  *Chart font* and *Chart text colour*.
- At the foot, **Flatten chart** (`ui::action_button`), with `Ctrl+E` in its tooltip.

A multi-selection of charts shows the Chart card when every chart is the same kind
(`selected_charts`), drawn from the first and writing each in one step: the kind, and every
option the edit changed (`chart::Options::carried`, a diff of the first chart's options carried
onto each), so each keeps its own data and the options left alone. Picking the segment or
dropdown row already lit changes nothing on the first chart, so it writes the others to that
value, one step (`chart_pick`). A chart-wide colour (text, axis, gridline, neutral, track) picked
or bound in the picker writes every selected chart (`chart_slot_targets`); a series' or node's
colour belongs to its table and writes only the shown chart. The Data section says *Edit
data one chart at a time* and the Series section is left out. A selection of charts only drops
the frame's own cards and group colours as one chart does. A scrub previews every chart, the
render overrides minting each chart's ghost ids past the others'.

### The data window (`chart_data.rs`)

A canvas card as Clean up's (`settings::canvas_card` with `CardHold::Open`, hung from
`ui::canvas_popup_area`), so the canvas stays live under it and the chart redraws as cells are
committed. 640 wide (the canvas's width less 32 when narrower, never under 320); the grid is a
fixed 240 tall and scrolls inside, so the card keeps one size (egui popups shrink but never grow
back). Rows inset `CANVAS_CARD_PAD`, 10 apart. One at a time, on one chart: it stays on its chart
when the selection is cleared or moves to anything but a chart, so a click on the board leaves it
open, and selecting another chart alone moves it there, at its first cell
(`the_window_stays_open_when_the_selection_moves_off`). It closes on its ✕, on
`Escape` and when its chart is deleted. It is one card over the canvas's top among Find, Colour
review, batch rename, Clean up and Fill with content: opening it closes them, and opening any of
them closes it. Leaving the document (a tab switch) closes it, the import box and a pending chart
read, since they name this document's node ids.

- **Head**: `settings::card_head_noted` — *Chart data*, the chart's layer name as the note, the
  shared ✕. Under it the toolbar of `ui::action_button`s at `CONTROL_H`, each as wide as
  `ui::action_button_w` makes it: *Add row* (`icon::ROWS_PLUS_BOTTOM`) and *Add column*
  (`icon::COLUMNS_PLUS_RIGHT`) on the left, disabled at the table's limits with *At most N rows*
  / *columns*, and *Import…* (`icon::IMPORT`) on the right. Each add adds at the end, moves the
  cursor onto the new row or column, and carries a cell being typed into in the same step. A
  linked chart hides the adds and shows its source line under the toolbar.
- **The grid** is one focusable region that locks the arrows, `Tab` and `Escape` while it has
  the keyboard, so the canvas keymap stays quiet (undo, redo and save still reach the
  document). A header row of column names, each with its type tag (`Aa` text, `#` number), a
  series' swatch, the name (dim when unmapped) and, against the right edge, its role's mark: the
  category's `icon::KEY` (11 pt, faint; tooltip *The chart's categories*, a scatter's *The chart's
  X values*) or a Sankey's *From*, *To* or *Value* in words. The name is cut with an ellipsis
  short of the mark, its whole name the tooltip when cut. The header and a row-number gutter are
  held at the scrolled view's edges over the cells; cells on the field ground with its hairline
  at `ui::MENU_ROW_H`, numbers right-aligned, the selected range in the accent's tint and the
  cursor a square 1.5 px accent edge. Past the table's last row and column a dim grid fills the
  view (the cells' lines in `color::card_border()` on the card's ground, no gutter numbers); it
  neither scrolls nor takes a click, so only real rows and columns make the grid scroll. The
  scroll bars are hidden, as Settings' are; the wheel and the cursor still scroll it. A column
  is a number column while every non-empty cell in it parses as one; the type is read, never set.
- **Keyboard**, the spreadsheet's: arrows move; typing starts an edit replacing the cell, `F2` or
  `Enter` edits it in place (`ui::label_edit_set`, with no ground or edge of its own, so the
  cell's ground and the cursor's edge stay and the text stays where it was drawn, right-aligned
  in a number column; a header renames the same way at `SMALL_PT`, over a field-ground cell
  with the cursor's edge); `Enter` commits and moves down, `Tab` commits and moves right,
  `Shift` the other way; `Escape` abandons an edit, and with none closes the window (the focused
  grid answers it; with nothing holding the keyboard it reaches the window through
  `OndinApp::escape`); `Delete` and `Backspace` clear the selected cells; `Shift`+arrows, a
  `Shift`-click and a drag select a range, a header click its column. `Ctrl+C` / `Ctrl+X` /
  `Ctrl+V` read and write tab-separated text, so a range pasted from a spreadsheet lands as cells
  from the range's corner, growing the table to fit (`chart_data::pasted`). A double-click edits
  a cell or renames a header.
- **Commits**: each committed cell, paste, clear, insert, sort or delete is one `SetChart`, undo
  step *Edit chart data*. A cell being typed into is the window's own, dropped when the window
  closes or moves to another chart.
- **Header menu** (on `ui::menu_frame`, opened by a right-click on the header at the pointer;
  a click selects the column): *Rename*, *Use as category* / *Use as X* (`icon::KEY`),
  *Show as series* (ticked), *Insert column left* / *right*, *Sort ascending* / *descending*
  (numbers by value in a number column, empty cells last), *Delete column*, which leaves the
  mapping and the series styles with it. **Row menu** on the gutter: *Insert row above* /
  *below*, *Delete row*. Both are placed by `ui::menu_place`; a menu or a rename whose column or
  row an undo removes closes.
- **While the import box is up** the window gives up the keyboard and is not drawn; it takes the
  keyboard back after.
- **Linked**, the grid is read-only and says so in the line under the toolbar (*Edited in
  sales.csv — unlink to edit here*): the menus' rows that write the table, *Rename* among them, are disabled and a
  double-click on a header does nothing. The mapping (the menu's *Use as category* and *Show as
  series* too), series and options stay editable.

### Import and the linked file

- **Doors**: *Import…* on the data window's toolbar; the context menu's and the palette's *Import
  chart data…* (the palette's dim with *Select a chart* without one); a `.csv`, `.tsv` or `.json`
  dropped onto a chart (`chart_import::drop_chart_data`). A `.csv` or `.tsv` dropped on bare
  canvas makes a chart there from it, linked (*Making one*); a `.json` dropped anywhere but on a
  chart stays the tokens import's door.
- **The readers** are core's (`chart::table_io`), each holding the table's limits and noting what
  it cut. **CSV** (RFC 4180): the delimiter is detected among comma, semicolon and tab; the first row is
  the header unless it parses as numbers. **TSV** the same with tab. **JSON** is an array of
  objects; every key seen becomes a column, in first-seen order (each object read in its own key
  order, which `serde_json`'s sorted map would lose); numbers are numbers, written into the cell
  without an exponent, which `parse_number` refuses; strings are
  text, `true`/`false` are text, `null` is empty, and a nested object or array leaves its cell
  empty and is named in the note. Anything else is refused: *Not an array of objects*. Numbers
  written with a thousands separator (`1,234.5`) parse; a decimal comma does not.
- **The import box**, the tokens review's modal canvas card (`settings::canvas_card`; a modal to
  `modal_is_up`, so the keymap is held off under it): *Import
  chart data*, the file name and its two parent folders; the first 20 rows as they will land,
  each column's type tag; *First row is the header* (a switch, CSV and TSV); an info note for
  anything left out; foot *Keep this file linked* (absent for a browser drop, which has no
  path), *Cancel*, *Replace data*; `Enter` replaces. Replacing keeps the mapping and series
  colours for every column whose name the new table also has (`Chart::with_table`). Undo step
  *Import chart data*; the status line says *Imported sales.csv · 12 rows, 3 series*.
- **A linked chart** stores `DataLink { path, size, modified, header }` in its own description:
  the stat the worker took when it read the file, so a change while the import box is open reads
  as *Changed*, and the box's *First row is the header* choice, which *Update* reads the file
  with (`header` absent from the file when `None`: a canvas drop, guessed on each read). The table is always the document's own copy, so a chart whose file is gone still
  draws. **The path is the document's, so untrusted**: the tokens link's rules hold
  (`styles-variables.md`, *A linked file*; `tokens_import::read_linked`) — absolute, on a local
  drive, a regular file of at most 16 MB. The file is read and parsed on a worker
  (`chart_import::ChartRead`, one at a time, a later read replacing the pending one) for every
  door that reads a path: *Import…*, a file dropped on a chart or on bare canvas, and *Update*.
  What it was for lands when it answers (`poll_chart_read`): the import box, the replaced tables,
  or the new chart; a refusal goes to the status line. A browser drop's bytes are parsed in the
  frame, having no file to wait on. The watch's `stat`s stay on the UI thread.
- **Watching**, as the tokens link (`watch_chart_links`): every `WATCH_EVERY` (45 s), when the
  window regains focus, and when a new path is linked, each linked path is `stat`ed. A change
  shows the pill *sales.csv changed · Update chart* (*Update 2 charts* when several link it) after
  the tokens pill, and the Data section's source line says *Changed on disk* with *Update*.
  Updating reads the file and replaces each
  linked chart's table in one undo step, *Update chart data*. **A file's contents enter the
  document only on the user's click**, never on its own: an opened document's link can name any
  file on the machine, and the next save would carry what it read.
- **A missing file** shows *File not found* on the source line with *Relink…* and *Unlink*.
  *Unlink* drops the link and makes the grid editable; undo step *Unlink data file*.
- **Copying a chart drops its link.** The clipboard carries the description and table but not the
  path, written and read through `Chart::copied`, the variables' detached twins included, for the
  reason the clipboard refuses linked images: pasted content must not make the
  receiving document read a file the copy chose.

### Flatten

- **`chart::flatten_ops`**: the frame keeps everything it is, its parts stay where they are, and
  the description goes (`chart::flatten`, one `SetChart { chart: None }`), undo step *Flatten
  chart*. In the same transaction every part binding its variable's scope refuses — a line's
  stroke drawing a colour scoped to fills, which `variable::check` excuses only on a chart's parts
  — detaches to the colour it drew (`variable::out_of_scope_ops`); the drop comes first, so the
  detaches no longer write a part. `chart::flatten` alone is refused at the commit when such a
  binding exists, so every door goes through `flatten_ops`
  (`flatten_chart_detaches_a_line_drawing_a_fill_scoped_colour`). What was on screen is what
  remains; the labels are text layers that can be edited, restyled or converted to paths.
  The frame then behaves as any frame: its tree opens, the Frame cards return, and *Frame* /
  *Group* converts it. Until then a chart stays a frame, since a group could not hold what its
  description regenerates: `build::convert_container_refusal` refuses the conversion with *A
  chart stays a frame: flatten it first*, so the identity card's dropdown is disabled with that
  reason and every other door refuses it (`tests/charts.rs · a_chart_stays_a_frame`).
- **Doors**: the Chart card's foot; `Ctrl+E`, which flattens a selected chart as it flattens a
  boolean (`app-booleans.md`); the context menu; the palette's *Flatten chart*. Over a selection
  of several layers that are all charts, the context menu and the palette offer it too
  (`menu::Context::all_charts`); a selection of charts flattens each, one undo step
  (`flatten_charts`).

### Context menu

On a chart (`context-menus.md`, `menu::head_rows`): *Edit data…*, *Import chart data…*, *Update
chart data* (while its file changed), *Unlink data file* (while linked), and *Flatten chart* in
the structure group, then the frame's usual rows. Over several charts its one chart row is
*Flatten chart*. The palette lists the same rows for a selected
chart, and *Edit data…*, *Import chart data…* and *Flatten chart* dim with *Select a chart*
without one (`menu::KIND_VERBS`).

## Elsewhere

- **Save format**: `chart` on the frame's DTO, the table as a column list and an array of rows,
  in v15 (`io.md`, the ladder). v15 had not shipped, so no bump of its own: a v14 reader already
  refuses the file by its version rather than loading the parts as plain shapes and dropping the
  description on its next save.
- **MCP snapshot** (`export.md`, *Snapshot*): a chart frame carries `chart` (kind, variant
  (`Chart::variant`), the table's header with each column's id and type and its row count, the
  mapping by column id, each series' name, visibility and own colour, and whether it is linked)
  beside its parts, each part with its `role`. Version 17.
- **Verbs** (`ondin_core::verbs`, MCP's Phase 2, `mcp.md`): `insert_chart` makes a 480 × 280
  frame named *Chart* on the kind's sample table, or on a table given, its mapping read off the
  column types unless one is given (`Mapping::guess`: a Sankey's first two text columns and first
  number column, a scatter's first number column as X and the rest as Y, else the first text
  column as the category and the number columns as series); `set_chart` changes the kind and
  merge-patches the mapping and options, a null setting a key back to its value in the whole
  struct's default (`Options::default()`, not a nested struct's own); `set_chart_data` replaces
  the table by *Import*'s rule (`Chart::with_table`; a mapping of which nothing survives is
  guessed) and drops the link; `flatten_chart` is `chart::flatten_ops` for each chart. Any other verb writing a part is refused at the commit by
  `chart::guard`.
- **Code export** (`export.md`, *Code export*) writes a chart as the frames it is: its flex and
  grid containers as CSS, its marks positioned by their percentage insets, its labels and pie
  centred on their points, its paths as stretching inline `<svg>`s (*Laid out, not placed*), so
  the exported chart resizes in a browser as it does on the canvas. The chart frame gets
  `role="img"` and an `aria-label` of its name, and its parts are `aria-hidden`.
- **Compare** (`app-compare.md`) marks the chart frame, not its parts, as changed, with *Chart
  data changed* or *Chart options changed* in the changes card.
- **SVG, PDF, PNG, Penpot and Sketch** need nothing: the parts are ordinary layers.

## Icons

New glyphs, on the *Drawing rules* (`app-chrome.md`): `CHART` (three ascending columns on a
baseline; the slot, the layers row, the palette), and one per kind — `CHART_COLUMN`, `CHART_BAR`,
`CHART_LINE`, `CHART_AREA`, `CHART_DONUT`, `CHART_SCATTER`, `CHART_SANKEY` (one node bar whose
two flows part to two bars) — plus `TABLE` for *Edit data…* and the window's undo step. In
the data window, `ROWS_PLUS_BOTTOM` (U+E59E, `COLUMNS_PLUS_RIGHT` transposed) is *Add row*,
`COLUMNS_PLUS_RIGHT` (the browser's *New mode*: a column added to a table) *Add column*, and
`KEY` (U+E2D6, a ring with a shaft and two teeth) the category column, its header mark and *Use
as category*. `CHART_PIE_SLICE` already means an arc's sweep and is not reused for the Pie kind;
`CHART_DONUT` is a ring with a gap, drawn to read apart from it. Column and `CHART` differ: the kind
is three columns of mixed height, the tool three ascending.

## Words

*Chart*, never *graph*, in every string; *Chart data* for the table; *series*, *category*,
*value*; *Flatten chart*; *Update chart data*, *Unlink data file*. Status line: *Imported
sales.csv · 12 rows, 3 series*; *Updated 2 charts from sales.csv*; refusals in the shared refused
look with their sentence.
