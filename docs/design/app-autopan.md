# Auto-pan and keeping the work in view

How a gesture dragged to the canvas edge scrolls the view, how keyboard and field moves reveal the
selection, and how the layers tree edge-scrolls. Part of §9.4 (v1 tools).

## 9.4 Auto-pan (`canvas::autopan`)

Inside a 32pt band at the canvas edge the view travels toward the pointer at up to 900 pt/s,
**quadratically** — the band is the only speed control, so the slow end must be the long end.

Three requirements, each a separate mistake if missed:

- **It runs before the input handlers.** Every gesture recomputes its world point from the live
  pointer each frame, so panning first makes the drag advance because the world under a still hand
  moved; no gesture code knows auto-pan exists. Panning afterwards would compute the preview against
  the previous camera.
- **It requests a repaint.** eframe is reactive; a hand held still at the edge produces no events.
- **The camera moves toward the pointer.** `pan_pixels` takes the delta the *content* moves by, so the
  reach must be negated or the canvas flees the drag.

### What does not auto-pan (`Drag::autopans`)

Stated as a rule, not a list of names:

- `Pan` — its hand already moves the view; adding band speed runs away.
- **Every gesture that turns about a fixed pivot** (e.g. `Rotate`, `PointRotate`) — its reachable set
  is a circle about a centre already in view; it can always be finished where it started.
- `Guide` — its band and its home are the same place. Ruler bars are `rulers::THICKNESS` = 20pt
  against the 32pt band, so a press on a bar starts inside the band; auto-panning made clicking a
  ruler scroll the view. Placing a guide beyond the visible area is done with the Guide panel's
  position field instead.

`OndinApp::ruler_at` (callers `guide_input`, `guide_at`) asks the two-part question
`rulers_on() && ruler_at(..)` in one place so no caller claims a press for an invisible bar.

### Cancel puts the view back

`autopan_total` accumulates, in world units, what auto-pan moved the camera; `cancel_gesture`
subtracts it. A running total rather than a snapshot, so a zoom or wheel-pan made during the drag is
kept. It is one drag's total: `normal_mode_input` zeroes it on `drag_started()` **and**
`drag_stopped()` (above the tool routing, so every tool family passes), and `finish_drag` keeps an
idempotent reset. A completed gesture keeps the scrolled view.

## `reveal_selection`

A held arrow key or a scrubbed X/Y field can walk a layer off screen. The nudge action and the
inspector's X and Y scroll it back by the **minimum** that shows it, and only once it has left.
W, H, rotation, skew and the `Ctrl`+arrow size step do not reveal — they change what a layer is, not
where. `reveal_axis` is the arithmetic; a selection larger than the viewport that overhangs both
edges scrolls **not at all**.

## Layers tree edge scroll

`ui::edge_scroll_speed` (shared curve with the canvas): a row dragged into a 22pt band at either end
of the tree scrolls it at up to 420 pt/s (the target is one 20px row). It uses
`scroll_with_delta_animation` with `ScrollAnimation::none()` — the pointer is the clock, and egui's
easing overshoots. There is exactly **one** implementation, in `app::layers_panel`.

## Snap candidates re-cull as the view moves

`snapping()` reads the live viewport each frame, so targets may appear or vanish mid-gesture. This is
deliberate: *snap to what you can see*, even under a moving view.

## Traps

- A second edge-autoscroller on the same band is a summand, not a second opinion:
  `scroll_with_delta_animation` accumulates, and last-write-wins on the animation restores easing.
- Without the release-time reset, a landed drag's scrolling stays in `autopan_total` for the next
  unrelated cancel to undo.
- Listing exempt gestures by name lets a new pivot-turning gesture arrive outside the list; keep the
  rule form.
