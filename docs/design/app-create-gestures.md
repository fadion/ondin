# Creation gestures

How the shape, line, image and text tools turn a drag or a click into a layer, and what Shift, Alt,
Space and the arrows do while drawing (`canvas::create_*`, `Tool::click_makes`). Part of §9.4.

## 9.4 Creating layers

### Shift constrains the extents (`canvas::create_corner`)

- The far corner gets equal extents on both axes: square rect, circle, regular polygon. The **larger**
  extent wins; each axis keeps its sign.
- Order: the corner is **rounded to the grid, then squared** (`begin_create` already puts `start` on
  the grid), giving two equal whole numbers. `squared_corner` is a free function.
- Dispatches on the gesture's own `Drag::Create { shape }`, not `self.tool`.

### Lines get a direction (`canvas::axial_corner`)

- Shift pulls the far end onto the nearest **eighth of a turn**. Direction held as an integer step,
  so for multiples of 45° the unit vector is one of the eight `(0,±1)`, `(±1,0)`, `(±1,±1)`; rounding
  the **length** keeps the endpoint on the grid without leaving the angle. Length is the pointer's
  projection onto the direction.
- Also used by a selected line's endpoint handles, measured from the fixed end.
- The Pen gets no Shift constraint (anchors are placed one click at a time).

### Alt centres on the press (`canvas::create_box`, `centred_ends`)

- Mirrors the near end through the press so the shape grows both ways. Lines too.
- Order: **constrain first, then mirror** (a 30×20 Shift+Alt drag is a square centred on the press).
  The grid survives (`2·start − corner`).

### Clicks (`Tool::click_makes() -> Option<ClickCreate>`)

An exhaustive `match` over `Tool`, no wildcard; `canvas::normal_mode_input` has one creation-click arm.

| Tool | `ClickCreate` | Size from |
| --- | --- | --- |
| Rect, Ellipse, Polygon, Star | `DefaultBox` | `tools::CLICK_CREATE_SIZE` square at the pointer, tool defaults |
| Image | `FittedImage` | the picture's own size, aspect kept, shrunk to fit frame/viewport (`tools::fitted_image_size`), **centred** on the pointer |
| Text | `TextNode` | what gets typed |
| Frame, Line | `None` | deliberately (a default frame guesses a layout; a line has no box) |

- A click synthesizes the gesture: `click_create` runs `begin_create` (parent frame, id, side count),
  takes two points from `click_box`, and calls `commit_create`, the tail shared with the drag release
  (text opens for typing, an image spends itself, a shape returns to Select).
- Alt on a click centres the default box on the click without resizing it. Shift does nothing.
- Any tool that answers a click must also drag a box.
- An image **drag** uses `create_image_in_box` and is obeyed exactly, aspect included.

### Keys during a create drag (`canvas::reshape_create`)

- `Space` held repositions the uncommitted shape: moves `start` by the pointer **delta**.
- `↑`/`↓` add/remove polygon or star sides; the count lives on `Drag::Create`, clamped by
  `tools::step_sides` to `geometry::MIN_SIDES..=MAX_SIDES`.
- Read in the gesture's own loop, not as `Action`s. `dispatch` drops `Action::Nudge` and
  `Action::SizeStep` while a create drag is up. The armed-pan cursor excludes a live create drag.

### Parent of a new layer

`canvas::draw_target`: the topmost frame that **contains** the press, else the document root. A new
frame lands the same way (frames nest).

### An armed pan outranks every tool

`panning_armed` (Space held, Hand tool, or middle-button drag) guards `guide_input`, the pen, the node
tool, image editing and the drag arm. The three click arms read one binding,
`let clicked = resp.clicked() && !panning_armed`.

## Traps

- Squaring before rounding leaves a "square" a pixel off; mirroring before squaring centres the shape
  off the press.
- Going through `cos`/`sin` and rounding the endpoint per axis puts a 45° line on the grid but off 45°.
- egui reports a still press as `clicked()`, never a drag, so without the click arm `MIN_CREATE_PX`'s
  discard is never reached and a click does nothing.
- A guard written once per arm is usually missing from one arm; bind it once.
