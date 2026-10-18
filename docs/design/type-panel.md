# Type panel and typography popup

The text layer's Type card, its typography popup (Character / Paragraph / Box tabs), where character
and paragraph writes land, and the custom controls the panel is built from: the unit chip, the
slider and the switch row. Code: `panels/typography.rs`.

## 9.4 v1 tools — The Type panel and the typography popup (`panels/typography.rs`)

### Layout

**The card holds the daily controls; the popup is overflow.** The card has four rows:

- family and variant;
- size;
- line height and letter spacing;
- both alignments.

Everything else sits behind one sliders button, in three tabs.

**No control appears in both places.** A duplicate would have to show "mixed" twice, and the user
could not tell which is the real setting. Horizontal and vertical alignment are both inline.

**The alignment row sits on the column grid of the paired field rows above it.**

- The four-cell horizontal track is exactly `half` wide, so it ends on the first field's right edge.
- The three-cell vertical track takes what is left after the popover button, so it starts on the second field's left edge.
- The two tracks cannot both have equal cells and sit on the grid. The grid wins: column edges are exact, and each cell is within a point of the 28pt button.

**The tabs are scopes, labelled by scope:** Character, Paragraph, Box. The popup opens on Character.

- The axes, `opsz` mode, OpenType list and language are character-scoped, so they live in the Character tab.
- These generated sections sit at the bottom, so changing family does not move the controls above them.
- Character tab order: Case, Decoration, Baseline & word spacing, Optical size, Language, Axes, OpenType.

**Sections are 15 apart.** The design uses 10. Against the in-section gap of 5, a 3:1 ratio groups;
2:1 does not.

**The body scrolls when it has to.** A variable font, with decoration and Manual `opsz` showing, can
push the Character tab past 700pt.

- The body is a `ScrollArea` capped at window height less margins.
- The tab strip stays outside it.
- The `ScrollArea` gets an explicit `max_rect`. Using the `Area`'s room instead makes the height ratchet down and never recover (§15 D106).

### Dismissal

The popup's click-away test forgives four things:

1. the button that opened it;
2. an overlay above it for its own combo lists;
3. the colour picker below it in layer order, opened from a decoration swatch. The picker is recognised by layer identity (`picker::layer_id`), not by rect, because it draws after the inspector;
4. a click that ends or cancels a gesture: `dragged_id().is_some() || gesture_cancelled`. `gesture_cancelled` is cleared at the end of the release frame. The test asks about the gesture, not the button, because a cancel is a right-click while the left button is held, and the release that follows is a primary event.

Every inspector popover reads the primary button.

### Character tab

**Language.** The closed field shows the `LANGUAGES` table name, like the open list. A locale missing
from the table shows its tag, not *None*: a document can carry an imported `xml:lang` or a tag from a
longer list.

**OpenType list.** The list is curated from the registry's own per-tag UI suggestions, carried in
`REGISTRY`.

- Features the shaper requires, applies per script, or applies as a corollary of another feature are left out: `aalt`, `ccmp`, `locl`, `mark`, `mkmk`, `numr`, `dnom`.
- *Show all features (n more)* reveals the rest. Its count applies both filters, the curation and the search field, through one shared search predicate.
- *Show fewer features* is always offered while the list is expanded.
- The model is unaffected either way: `CharAttr::Features` carries any tag.
- Each row shows a friendly name (the registry's, or the font's own for the two font-defined ranges). On hover it says what the feature does and shows the tag.
- What the list can offer is bounded by the font file. A web font is a subset and offers fewer features.
- **Multi-value `cvXX`.** A `cvXX` with more than one named value gets a full-width dropdown under its name, with *Off* first, instead of a switch. `FeatureSetting::value` answers both on/off and which value.

**Decoration.** One three-cell track: none, underline, strikethrough. The model holds both lines
independently, but picking one here clears the other.

- **Carried tuning.** Thickness, offset, style and colour carry across a switch.
- **Skip-ink** is drawn only for an underline. On a strikethrough it is absent, and the dropdown takes the freed width. The state still rides in `Decoration`, so switching back resumes it. It shows lit in its default state.
- **Colour slot.** The colour is `PaintSlot::TextDecoration(DecorationSide)`, a node-less slot (§9.2), so it gets the shared swatch, hex and picker. The row has a chip, editable hex, opacity and reset.
- **Unset colour.** Absence means "the text's own". The chip draws the resolved paint: `TypeSubject::text_paint`, the node's first visible fill whole, so a gradient fill shows its ramp (`text_ramp`). `text_color` is the one-colour collapse, the first stop, used by the hex field and the picker.
- **Adoption.** Opening the picker first adopts that colour into `Decoration::color`, so the click changes what is editable, not what is drawn.

**Character-scoped slots.** The decoration colour and the text's own colour are the two slots whose
write is not a `Transaction` (§15 D154).

- All three slot verbs divert them, through `PaintSlot::char_scoped` → `write_char_slot` → `apply_char_attrs` / `char_valve`.
- `slot_transaction` returns an empty transaction for them, never `unreachable!`.
- `preview_slot` diverts too, but declines for a live selection, where the editor's spans are the preview.

**Reset buttons are always drawn.** Axis rows and the decoration colour row end in a 26×26 reset. It
is `FieldButton`-dimmed when there is nothing to restore, so the row never ends short.

**A reset is live when `TypeSubject::ragged` says so**, not when `shown()` differs from the default.

- `mixed` says what a control may display. Over a whole node it is false by design, because a whole-node write flattens.
- `shown()` reads one byte, so it can miss a span that does not reach that byte.

### Where a write goes, and what a control shows

**Character writes** go to the target given by `TypeSubject::range`:

- **A live session with a selection.** The range is the selection, and controls can read *mixed*. The write goes into the session's spans (`TextEdit::style_selection`), which preview themselves and commit once when the session ends (§9.3).
  - List-valued attributes (`CharAttr::Features`, `CharAttr::Variations`) use `TextEdit::style_selection_per_run`, a closure over each run's own value. `style_selection` would flatten the list.
  - No drag valve is used, because the session is the preview.
  - `canvas::preview_session` builds the `SetText` that feeds the canvas for every session-scoped panel write.
  - On cancel there is no override to drop. `app::InFlight::session_scrub` records the prior value, and `canvas::restyle_session_with` rewinds it.
  - The session preview is exempt from every clear (`EditorSession::clear_gesture_preview`).
- **Anything else.** The range is the whole node. The write sets the node's default **and** drops that attribute's span overrides, always both.

**What a control shows** comes from `TypeSubject::resolve_at`, not `range.start`.

- A bare caret resolves at the byte to its left, which is what an insertion inherits: `Spans::edited` extends a span ending at the caret.
- At byte 0 it resolves to the node defaults.
- This affects the readout only. A bare-caret write still sets the default and drops the overrides.

**Paragraph writes** use `TypeSubject::para_range`: the selection snapped outward to hard breaks
(`text::paragraph_bounds`).

- A bare caret is partial here.
- Any open session is partial, select-all included. The node's paragraph defaults are therefore reachable only with the layer selected and no session (§15 D164).
- `Alignment`, `Justify` and base direction are one value per `Layout` (§15 D77). Their sections say so on hover (`WHOLE_LAYER`).
- *Last line* is drawn only under `Justify`.

### Paragraph tab

**Spacing & indent** has five controls in three rows:

- spacing and first-line indent;
- the 26×26 hanging pair;
- block indents `indent_start`/`indent_end`, prefixed with `icon::ARROW_LINE_LEFT`/`ARROW_LINE_RIGHT`. The side the bar sits on is the margin the number is measured from.

The hanging pair shows mixed as neither square lit. A press makes the selection uniform.

### Mixed spelling

The rule is app-wide (§15 D130): the word *Mixed* wherever it fits. A dash appears only in
`ui::segment_mixed`, which raises no cell, and on `ui::Swatch::Mixed`.

This panel's numeric fields are the exception. They show a dash through `typography::mixed_text`:

- Size;
- the list *Level*;
- `length_char_field`'s three character lengths;
- `length_field`'s four paragraph lengths;
- the variable-axis field;
- and line height, through its own copy of the formatter.

Whether those fields are too small for the word has not been ruled. `ui::switch_row_mixed` does
spell the word.

### Units and `Length` fields

**Three states on one field, twice.** The third state in each cycle is a value, not a collapsed
control.

- Line height's suffix cycles `auto → % → px`.
- A decoration's thickness and offset cycle `Font → px → %`. The two cycles run in opposite orders.

`ui::Suffix` is the unit as its own dimmed, clickable widget. `ui::Prefix` is its counterpart at the
start of the field.

**Leaving the "font decides" state seeds the font's value.**

- Line height's `auto` seeds the shaped first line's height.
- Thickness seeds `TextLayout::decoration_sizes`, the first run's `(underline, strikethrough)` from `run.metrics()`, before clamping and overrides.
- Offset seeds zero, deliberately. Its face metric carries the opposite sign (§15 D151), and a band on the baseline is visible and recoverable.

**Ranges.**

- A `Length` field's px range is derived from its `%` range at the current font size (`typography::px_range_for`, matching `Length::in_unit`), so a unit switch cannot change the number. There is no separate px cap.
- Each field gets the `Bounds` for its own quantity. `DECORATION_THICKNESS` floors at zero, and the model floors thickness too (§5.4). `DECORATION_OFFSET` goes both ways. `INDENT` serves the three indents. `MAX_TRACKING_PCT` is tracking's alone.
- Open: the caps for paragraph spacing, indents and decoration all hold an inherited 200% that was never chosen (`roadmap.md`, *Later · Parked decisions*).

**Reporting.** Every `Length` field reports on a frame where the value changed and again on the
frame the drag ends. The release frame is the one `edit_valve` commits on.

- There are four copies of the control: `length_field`, `length_char_field`, `optional_length_field` and `type_line_height_field`. Unifying them is declined; `type_line_height_field`'s `auto` shows a resolved px value.
- The character `Length` fields carry hover text.

**The unit chip.** A clickable unit is a control inside the field.

- **Hit box.** 18pt tall, sized to its text where that is wider (`auto`, `Font`), and four points wider than the text on each side.
- **Placement.** It hangs 4pt into the field's padding, so its edge finishes 5pt inside the hairline where the digits finish at 9.
- **Identity.** It has its own id and rect; the field keeps the rest of the row.
- **Hover** brightens the text with no ground behind it. `%` and `px` ink at different vertical centres, so a box would look off-centre. There is no pointing-hand cursor.
- **Colour.** The galley is laid out in `Color32::PLACEHOLDER`, so `Painter::galley`'s fallback colour applies. A baked `.color(…)` ignores the hover colour, and an unset colour becomes full-strength `text_color`.
- **Allocation.** The unit strip is allocated, not painted over, because `field_frame` shrink-wraps its content (§15 D105).
- **Spacing.** Every gap inside a value field is measured by hand, and `item_spacing.x` is zeroed in the row.

### Box tab

**Sizing.** `OndinApp::set_text_sizing(id, cell)` serves both the segmented control and the context
menu's three checkable rows (`context-menus.md` §5.6).

- The switch adopts the shaped size, so the text does not move.
- It ends in `text_session_restyled`, so a live editor re-shapes.
- It measures the preview layout. The menu reads its tick from the display node for the same reason.
- A scale never changes which sizing mode a layer is in (§5.6).

**Wrapping.** The three wrap controls are stacked segmented tracks: eight options, none hidden.

- *Word break* and *Long words* dim under `NoWrap`, which reaches parley as `TextWrapMode(NoWrap)`. They are dimmed, not reset, so stored values survive a round trip.
- The Wrap strip itself, hard breaks, paragraph spacing and indents all stay live.
- *Margins* (one switch, *Optical margins*) is outside the gate: single-line labels are where it helps most. There is no preview text, because the effect is 1–10% of the font size. The tooltip says what to look for on the canvas.

### Controls

**`ui::slider`** is hand-painted, like `segmented` and `switch`.

- **Look.** A 3pt rail (`SLIDER_RAIL_H`), an 11pt knob (`SLIDER_H`), the filled half in `ACCENT_600` and the knob in `ACCENT_100` with a shadow.
- **Behaviour.** No text box. Knob travel is clamped to `half..width − half`, with the rail drawn full width. It is absolute: it jumps to the press.
- **Overflow.** The shadow paints one point below `SLIDER_H`, into the caller's row gap. The other three sides are tight.
- **`SLIDER_H` is not read outside `ui.rs`.** Do not treat it as what every caller reserves.
- **The picker's slider differs.** `panels::picker::slider_track` allocates `TRACK_H + 6.0` (`TRACK_H` is the rail thickness). It bounds its knob vertically and lets it overhang 7pt off each horizontal end, which fits inside the card's `PAD` of 11. The two sliders bound different axes, so their constants are not one quantity.

**`ui::switch_row`** is the toggle list's row.

- **One claimant.** The switch (26×16) is painted, not allocated, so the whole row is the one click target.
- **State.** Knob travel and track colour both change.
- **Label.** Two tiers of contrast, `ui::SWITCH_ROW_LABEL_PT` and `ui::switch_row_ink`, shared with the `cvXX` row's name.
- **Hover ground** is painted wider than the row, into the card padding, so labels line up with the eyebrow above.
- **Mixed** (`ui::switch_row_mixed`, the same `switch_row_inner` over `on: Option<bool>`): the word *Mixed* right-aligned where the switch would be, with the label in the off tier. The row stays live; a click turns the feature **on**, and the tooltip says so.

## Traps

- **Reset liveness.** Answer it from `ragged`, not `shown()`. `shown()` reads one byte and misses spans elsewhere.
- **Default-only writes.** A whole-node character write that sets the default without dropping overrides leaves runs behind.
- **List-valued attributes in a session** must go through `style_selection_per_run`. `style_selection` flattens them.
- **Decoration seeds.** Leaving a "font decides" state must seed the font's metric. A zero thickness erases the line: `decoration_path` and `text::crossings` refuse a zero band.
- **Reversed cycles.** Line height and decoration cycle their units in opposite orders. A fix copied from one to the other gives the right number in the wrong unit.
- **Shared ranges and caps.** A range or cap shared by two fields claims they measure the same quantity. Give each field its own `Bounds`.
- **Galley colour.** A galley built with `.color(…)` ignores the painter's colour, so a hover state computed for it is silently dropped. Lay out in `Color32::PLACEHOLDER`.
- **Shrink-wrap.** `field_frame` shrink-wraps, so anything painted into the field's space without being allocated makes the field narrower than asked.
