//! Visual theme for the app chrome: the "Nocturne" dark design realized in egui.
//!
//! Every panel ground is the near-neutral text colour mixed toward black, with a
//! single steel-blue accent. The mixed values are frozen here as `Color32`
//! constants, since egui has no `color-mix`.
//!
//! This module is where the design tokens become egui `Style`/`Visuals`/fonts.

use eframe::egui;
use std::sync::Arc;

/// Palette. Grounds are `color-mix(in srgb, var(--color-text) N%, #000)` with
/// text = `#e9e9ed` (233, 233, 237); the accent ramp is a steel blue.
///
/// No `#[allow(dead_code)]` here: `dead_code` reports a colour that loses its
/// last caller.
pub mod color {
    use eframe::egui::Color32;

    /// `--color-text` — the one near-neutral foreground everything derives from.
    pub const TEXT: Color32 = Color32::from_rgb(233, 233, 237);

    // Grounds (text mixed toward pure black).
    /// 10% — main container, canvas, inspector pane.
    pub const BG: Color32 = Color32::from_rgb(23, 23, 24);
    /// 13% — top bar.
    pub const TOPBAR: Color32 = Color32::from_rgb(30, 30, 31);
    /// 14% — layers pane.
    pub const PANEL: Color32 = Color32::from_rgb(33, 33, 33);
    /// 16% — tool rail and inspector cards.
    pub const CARD: Color32 = Color32::from_rgb(37, 37, 38);
    /// Field/input ground inside a card (text 5% over the card).
    pub const FIELD: Color32 = Color32::from_rgb(47, 47, 48);
    /// Hairline field border — text 8% over **[`FIELD`], not over [`CARD`]**.
    ///
    /// A translucent CSS border composites over the element's own background
    /// (`background-clip: border-box`), so the 8% resolves against the field's 47,
    /// not the card's 37; resolved over the card it is 53, which is too dim to
    /// register. [`CARD_BORDER`] is the same 8% over the card, for boxes filled
    /// with [`CARD`].
    pub const FIELD_BORDER: Color32 = Color32::from_rgb(62, 62, 63);

    /// Hairline round a box filled with [`CARD`] — the dashboard's project and
    /// file cards, and the *New file* slot's dashed outline.
    ///
    /// Text 8% over [`CARD`]. Kept apart from [`FIELD_BORDER`]: against a 37 ground
    /// 62 would read as a lit edge, which is what a *hovered* card already means
    /// (`ACCENT_700`).
    pub const CARD_BORDER: Color32 = Color32::from_rgb(53, 53, 54);

    /// The ground every hovered control paints (§15 D484): egui's stock widgets
    /// (`install_style_into`), `ui::icon_button_padded`, `ui::button_face` and
    /// `inspector::template_cell` all use it, and they must agree or the hover
    /// splits into two greys.
    pub const HOVER: Color32 = Color32::from_rgb(52, 52, 54);

    // Accent ramp (Editor `:root` override — steel blue).
    pub const ACCENT: Color32 = Color32::from_rgb(109, 140, 217); // 500 / base
    pub const ACCENT_100: Color32 = Color32::from_rgb(226, 233, 248);
    pub const ACCENT_200: Color32 = Color32::from_rgb(196, 211, 241);
    pub const ACCENT_300: Color32 = Color32::from_rgb(166, 189, 234);
    pub const ACCENT_400: Color32 = Color32::from_rgb(139, 165, 224);
    pub const ACCENT_600: Color32 = Color32::from_rgb(87, 115, 189);
    pub const ACCENT_700: Color32 = Color32::from_rgb(68, 92, 154);
    pub const ACCENT_800: Color32 = Color32::from_rgb(47, 63, 102);
    pub const ACCENT_900: Color32 = Color32::from_rgb(33, 44, 70);

    /// Text at a given opacity over the dark ground. Used for the muted label
    /// tiers the design leans on (38% / 45% / 62% / 88%).
    pub const fn text_a(alpha: u8) -> Color32 {
        Color32::from_rgba_premultiplied(
            (233 * alpha as u32 / 255) as u8,
            (233 * alpha as u32 / 255) as u8,
            (237 * alpha as u32 / 255) as u8,
            alpha,
        )
    }

    /// Hairline divider / faint border (text at ~9%).
    pub const DIVIDER: Color32 = text_a(23);

    /// The selection colour on the canvas — a light sky blue, deliberately
    /// *not* the chrome accent: the panels and much artwork are blue, and an
    /// outline in that family disappears into both.
    pub const SELECT: Color32 = Color32::from_rgb(0x63, 0xb3, 0xf5);
    /// The same hue at ~55%, for the parts of the chrome that must not compete
    /// with the outline itself: pen control handles, the entered-group hint.
    pub const SELECT_DIM: Color32 = select_a(0x8c);
    /// The layer-row highlight in the layers tree, at panel strength.
    pub const SELECT_ROW: Color32 = select_a(43);
    /// A selected grid container's **gaps** on the canvas (§15 D921), and the
    /// tint of every spacing band — a flex container's gaps and any container's
    /// padding — while the pointer is inside it. Faint enough that the artwork
    /// under a gap still reads.
    pub const GRID_GAP: Color32 = select_a(30);
    /// A spacing band under the pointer or being dragged: [`GRID_GAP`], stronger.
    pub const SPACING_STRONG: Color32 = select_a(70);
    /// The hatch marking a gap's stored part inside the space it lays out.
    pub const SPACING_HATCH: Color32 = select_a(150);

    /// The **key layer** outline: the one member of a selection that align aligns
    /// to and that a boolean takes as its base.
    ///
    /// A different hue rather than a shade of [`SELECT`], because every other
    /// member of the selection already wears `SELECT` and this mark has to
    /// separate one of them from the rest. Magenta is the far side of the wheel
    /// from the blue and the steel panels; light because the artwork below is
    /// unknown. The chrome's other hues are `MEASURE` and `COMPONENT` (§15 D13).
    pub const KEY: Color32 = Color32::from_rgb(0xd2, 0x64, 0xc8);

    /// **A component's hue on the canvas** — the outline, handles, size badge and
    /// hover outline of a main or an instance, the main's chip and the instance's
    /// tag (§15 D1001): a violet in [`SELECT`]'s register. A set keeps `SELECT`
    /// and its neutral tab, and a layer inside a component keeps `SELECT`.
    pub const COMPONENT: Color32 = Color32::from_rgb(0xa7, 0x8b, 0xfa);
    /// [`COMPONENT`] at [`SELECT_DIM`]'s alpha, for a multi-selection's box and
    /// the secondary words of an instance's tag.
    pub const COMPONENT_DIM: Color32 =
        Color32::from_rgba_unmultiplied_const(COMPONENT.r(), COMPONENT.g(), COMPONENT.b(), 0x8c);
    /// Ink on a [`COMPONENT`] badge or chip, for [`BADGE_INK`]'s reason.
    pub const COMPONENT_INK: Color32 = Color32::from_rgb(0x1e, 0x12, 0x3d);
    /// [`COMPONENT_INK`] faded, for the secondary words on a main's chip — its
    /// instance count.
    pub const COMPONENT_INK_DIM: Color32 =
        Color32::from_rgba_unmultiplied_const(0x1e, 0x12, 0x3d, 0xa6);

    /// [`SELECT`] at an alpha, premultiplied. Derived so retuning the hue carries
    /// every tier with it.
    const fn select_a(alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied_const(SELECT.r(), SELECT.g(), SELECT.b(), alpha)
    }
    /// The ground of the dimension badge under a selection: the selection
    /// colour itself, so the box and the number it quotes are one object.
    pub const BADGE: Color32 = SELECT;
    /// Ink on [`BADGE`]. The badge ground is light, so the near-white text tier
    /// the rest of the chrome uses would be unreadable on it.
    pub const BADGE_INK: Color32 = Color32::from_rgb(0x0c, 0x25, 0x30);

    /// **A labelled distance on the canvas** — the lines and pills of the Alt-hover
    /// measure and of the gaps an equal-gap snap has just made (`measure.rs`).
    ///
    /// Red, as a distance annotation is in every tool that has one. Not a shade of
    /// [`SELECT`], because the measure speaks about the gap between the selection
    /// and something unselected. Light rather than the guides' dark red
    /// ([`ondin_core::DEFAULT_GUIDE_COLOR`]), since measures are often drawn *to* a
    /// guide and must not read as the same object. The Position card's pin diagram
    /// also draws a centred axis's struts in it (§15 D891).
    pub const MEASURE: Color32 = Color32::from_rgb(0xf2, 0x6d, 0x6d);
    /// Ink on [`MEASURE`], for [`BADGE_INK`]'s reason: the pill's ground is light.
    pub const MEASURE_INK: Color32 = Color32::from_rgb(0x2b, 0x0c, 0x0c);

    /// A layer the app cannot draw as the document describes it — today, one
    /// whose picture is missing (§15 D179).
    ///
    /// Amber rather than red: the usual cause is a moved file, a thing to fix
    /// rather than a mistake. It tints the row's existing kind glyph, which costs
    /// no width.
    pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xa0, 0x4a);

    /// The star on a starred document's card.
    ///
    /// Its own constant rather than [`WARN`], though near-identical: the two mean
    /// opposite things (*you marked this* / *this cannot be drawn*) and must be
    /// free to be re-tuned apart.
    pub const STAR: Color32 = Color32::from_rgb(0xe0, 0xa9, 0x3c);

    /// A row that destroys something — today, *Delete* in a context menu.
    ///
    /// The app's one red, so the eye lands on the row work can be lost with. Distinct
    /// from [`WARN`]: amber grades toward "cannot be drawn", red means "this click
    /// removes it".
    pub const DANGER: Color32 = Color32::from_rgb(0xe0, 0x73, 0x6b);

    /// The editor's save dot, red while the document differs from its file
    /// (§15 D962). [`DANGER`]'s value under its own name: a dirty document is
    /// not a click that destroys something, and the two must be free to part.
    pub const UNSAVED: Color32 = DANGER;

    /// The save dot once the file matches the document (§15 D962) — the app's
    /// one green, computed to [`DANGER`]'s OKLCH lightness and chroma (0.68,
    /// 0.137) at a green hue (154°), so neither state is louder than the other.
    pub const SAVED: Color32 = Color32::from_rgb(0x45, 0xb0, 0x70);

    /// The three steps a *filled* destructive button needs, mirroring
    /// [`ACCENT_900`]/[`ACCENT_700`]/[`ACCENT_200`] — ground, border and ink for
    /// [`super::super::ui::FieldButton::Danger`].
    ///
    /// Computed from [`DANGER`] by the accent ramp's ratios (×0.31 ground, ×0.66
    /// border, 61.5% of the way to white for the ink), so the red button is the
    /// blue one's twin in weight. A fourth step is the hovered ground, mirroring
    /// [`ACCENT_800`].
    pub const DANGER_900: Color32 = Color32::from_rgb(69, 36, 33);
    pub const DANGER_800: Color32 = Color32::from_rgb(101, 52, 48);
    pub const DANGER_700: Color32 = Color32::from_rgb(148, 76, 71);
    pub const DANGER_200: Color32 = Color32::from_rgb(243, 201, 198);

    /// The ground of an *unselected* point marker in the node tool.
    ///
    /// Opaque rather than hollow like a resize handle (§15 D14): the anchor *is*
    /// the target and sits on ink of any colour. Near-white because it has to
    /// read on the canvas, not in a panel.
    pub const POINT: Color32 = Color32::from_rgb(0xf2, 0xf4, 0xf8);
}

/// Muted text tiers (named for how the design uses them).
pub mod text {
    use super::color::text_a;
    use eframe::egui::Color32;
    pub const STRONG: Color32 = text_a(224); // ~88%
    pub const MUTED: Color32 = text_a(158); // ~62%
    pub const DIM: Color32 = text_a(115); // ~45%
    pub const FAINT: Color32 = text_a(97); // ~38%
    /// **A control that cannot be used**, as distinct from one that is merely
    /// quiet. Below [`FAINT`], which stays the tier for placeholders, hints and
    /// secondary labels.
    ///
    /// 79 is matched, not chosen: `Ui::disable` fades by `Visuals::disabled_alpha`
    /// (0.5), so a [`MUTED`] glyph disabled that way paints at 79, and colour-based
    /// disabling (`icon_button`'s `enabled: false`, `FieldButton::Disabled`) must
    /// land on the same ink. It also stays well apart from the "off" tiers
    /// (`HIDDEN_OPACITY` over MUTED is 95, a hidden or locked layer row is
    /// `text_a(102)`), so dimmed-because-off and dimmed-because-unavailable differ.
    pub const DISABLED: Color32 = text_a(79); // ~31%
}

/// Phosphor icon glyphs (Regular weight), by name. Codepoints extracted from
/// `@phosphor-icons/web@2.1.1`. Render with [`icon_text`] so they use the
/// Phosphor font family.
///
/// # Adding an icon
///
/// Look the name up in **`assets/fonts/Phosphor.codepoints.txt`** and paste the
/// codepoint in. `Phosphor.ttf`'s `post` table is version 3.0 and carries no
/// glyph names, so the table was recovered from the font's ligatures; its header
/// says how to regenerate it and why single-word names (`x`, `check`) are missing.
///
/// The `#[allow(dead_code)]` sits on the individual unused constants, each with
/// its reason, not on the module, so the next glyph to lose its last call site
/// warns.
pub mod icon {
    pub const DIAMOND: &str = "\u{e1ec}";
    pub const CARET_DOWN: &str = "\u{e136}";
    pub const CARET_RIGHT: &str = "\u{e13a}";
    /// The unused third of the caret trio: `CARET_DOWN` and `CARET_RIGHT` are the
    /// pair every disclosure draws, and nothing points a caret up yet.
    #[allow(dead_code)]
    pub const CARET_UP: &str = "\u{e13c}";
    pub const MAGNIFYING_GLASS: &str = "\u{e30c}";
    pub const MAGNIFYING_GLASS_PLUS: &str = "\u{e310}";
    pub const FRAME_CORNERS: &str = "\u{e626}";
    pub const SELECTION: &str = "\u{e69a}";
    /// The identity row's three actions, beside the boolean dropdown (§9.4).
    /// Also the Effects panel's **Layer blur** row glyph, as the design draws it.
    pub const CIRCLE_HALF: &str = "\u{e18c}"; // mask
    pub const SELECTION_PLUS: &str = "\u{e69c}"; // group
    pub const SELECTION_SLASH: &str = "\u{e69e}"; // ungroup
    /// A `Path`'s kind glyph in the layers tree, and the *Outline shape* row's,
    /// because that row produces a path.
    pub const POLYGON: &str = "\u{e6d0}";
    pub const LIST: &str = "\u{e2f0}";
    pub const TEXT_T: &str = "\u{e48a}";
    pub const IMAGE: &str = "\u{e2ca}";
    pub const CIRCLE: &str = "\u{e18a}";
    pub const SQUARE: &str = "\u{e45e}";
    pub const TRIANGLE: &str = "\u{e4b0}"; // the polygon tool — it starts as one
    pub const STAR: &str = "\u{e46a}";
    pub const PLAY: &str = "\u{e3d0}"; // present mode
    pub const ASTERISK: &str = "\u{e0aa}"; // a star's point count
    pub const ANGLE: &str = "\u{e7bc}"; // a star's inner ratio
    pub const EYE: &str = "\u{e220}";
    pub const EYE_SLASH: &str = "\u{e224}";
    pub const LOCK_SIMPLE: &str = "\u{e308}";
    pub const LOCK_SIMPLE_OPEN: &str = "\u{e30a}"; // an unlocked layer
    pub const CURSOR: &str = "\u{e1dc}";
    pub const HAND: &str = "\u{e298}";
    pub const LINE_SEGMENT: &str = "\u{e6d2}";
    pub const PEN_NIB: &str = "\u{e3ac}";
    pub const BEZIER_CURVE: &str = "\u{eb00}"; // the node tool: a curve with its handles
    pub const EYEDROPPER: &str = "\u{e568}";
    pub const CHAT: &str = "\u{e15c}";
    pub const ALIGN_LEFT: &str = "\u{e50e}";
    pub const ALIGN_CENTER_HORIZONTAL: &str = "\u{e50a}";
    pub const ALIGN_RIGHT: &str = "\u{e510}";
    pub const ALIGN_TOP: &str = "\u{e512}";
    pub const ALIGN_CENTER_VERTICAL: &str = "\u{e50c}";
    pub const ALIGN_BOTTOM: &str = "\u{e506}";
    pub const ARROW_CLOCKWISE: &str = "\u{e036}";
    pub const CORNERS_OUT: &str = "\u{e1d0}"; // zoom to fit
    pub const NUMBER_SQUARE_ONE: &str = "\u{e36c}"; // actual size (1:1 / 100%)
    pub const PLUS: &str = "\u{e3d4}";
    pub const DOTS_THREE: &str = "\u{e1fe}";
    /// The dashboard's per-file overflow menu.
    ///
    /// Vertical where the editor's overflow is horizontal: a row's own menu reads
    /// as belonging to the row, and the design draws it that way.
    pub const DOTS_THREE_VERTICAL: &str = "\u{e208}";
    /// The dashboard's *Recent*, and the picker's recent-colours row.
    pub const CLOCK_COUNTER_CLOCKWISE: &str = "\u{e1a0}";
    /// The picker's row of the colours in this document.
    pub const FILE: &str = "\u{e230}";
    /// The dashboard's *All files*, and the placeholder on a card with no
    /// thumbnail yet.
    pub const FILES: &str = "\u{e710}";
    /// The dashboard's grid-view toggle, against `ROWS` for the list.
    pub const SQUARES_FOUR: &str = "\u{e464}";
    /// The dashboard's *Edit project*.
    pub const PENCIL_SIMPLE: &str = "\u{e3b4}";
    pub const SHARE_NETWORK: &str = "\u{e408}";
    pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{e038}"; // undo
    pub const FOLDER_OPEN: &str = "\u{e256}";
    pub const FLOPPY_DISK: &str = "\u{e248}"; // save
    pub const MAGNIFYING_GLASS_MINUS: &str = "\u{e30e}";
    pub const X: &str = "\u{e4f6}"; // close / remove a paint row
    /// Mirror in x (and a gradient's stops).
    ///
    /// Not a matched pair with [`FLIP_VERTICAL`]: the horizontal one is two
    /// triangles about a dashed axis, the vertical one hangs triangles off a solid
    /// bar, so side by side the second reads as a flag. Phosphor has no second
    /// flip pair, which is why the image popover's mirror buttons are words
    /// (§15 D182).
    pub const FLIP_HORIZONTAL: &str = "\u{ed6a}";
    /// Mirror in y. See [`FLIP_HORIZONTAL`] — the two do not match.
    pub const FLIP_VERTICAL: &str = "\u{ed6c}";
    pub const ARROWS_CLOCKWISE: &str = "\u{e094}"; // reverse a gradient's stops
    /// The dashboard's sort control, which is one button carrying whichever order
    /// is in force (the design canvas's *Dashboard* artboard, `ph-arrows-down-up`).
    ///
    /// Not [`ARROWS_CLOCKWISE`], which means *reverse*: the button steps to the
    /// next key, and arrows passing each other read as "ordered by".
    pub const ARROWS_DOWN_UP: &str = "\u{e098}";
    /// Opacity, and the **inner shadow**'s row glyph (the design's `ph-drop-half`).
    /// The two never share a card. It is the same droplet as [`DROP_HALF_BOTTOM`],
    /// split vertically rather than horizontally, so the inner and drop shadow read
    /// as two states of one picture.
    pub const DROP_HALF: &str = "\u{e566}";
    /// The **drop shadow**'s row glyph, from the design (`ph-drop-half-bottom`).
    /// See [`DROP_HALF`], which is its pair.
    pub const DROP_HALF_BOTTOM: &str = "\u{eb40}";
    /// The **Filters** row's glyph, from the design (`ph-sun`).
    pub const SUN: &str = "\u{e472}";
    pub const SQUARE_HALF: &str = "\u{e462}"; // stroke alignment
    // Per-corner radius: each elbow points into the corner it sets.
    pub const ARROW_ELBOW_LEFT_DOWN: &str = "\u{e04a}"; // top-left
    pub const ARROW_ELBOW_RIGHT_DOWN: &str = "\u{e050}"; // top-right
    pub const ARROW_ELBOW_LEFT_UP: &str = "\u{e04c}"; // bottom-left
    pub const ARROW_ELBOW_RIGHT_UP: &str = "\u{e052}"; // bottom-right
    // Z-order: one step vs all the way.
    pub const ARROW_UP: &str = "\u{e08e}";
    pub const ARROW_DOWN: &str = "\u{e03e}";
    /// *Bring to front*; [`ARROW_LINE_DOWN`] is *Send to back* (§15 D761). The
    /// arrow-to-a-bar pair is the *all the way* family against the plain
    /// `ARROW_UP`/`ARROW_DOWN` one-step pair, and `ARROW_LINE_LEFT`/`ARROW_LINE_RIGHT`
    /// carry the same *against a wall* reading. The design's mockup draws
    /// `ph-stack` / `ph-stack-simple` here, a recorded deviation because
    /// `ph-stack-simple` is *Flatten* and the rows co-occur.
    pub const ARROW_LINE_UP: &str = "\u{e066}";
    /// *Send to back*; see [`ARROW_LINE_UP`].
    pub const ARROW_LINE_DOWN: &str = "\u{e05c}";
    // Distribute: equal gaps along an axis. The design draws these as the
    // column/row glyphs, which read as "even columns" rather than "push apart".
    pub const ARROWS_OUT_LINE_HORIZONTAL: &str = "\u{e534}";
    pub const ARROWS_OUT_LINE_VERTICAL: &str = "\u{e536}";
    /// Fold and unfold the whole layer tree. The pair reads as "gather these
    /// together" and "push them apart", which is what the button does to the rows —
    /// where the caret it replaced said "this row is open", a claim about one row
    /// that a tree-wide control has no business making.
    pub const ARROWS_IN_LINE_VERTICAL: &str = "\u{e532}";
    pub const COLUMNS: &str = "\u{e546}";
    pub const ROWS: &str = "\u{e5a2}";
    pub const HAND_GRABBING: &str = "\u{e57c}";
    pub const RULER: &str = "\u{e6b8}"; // a guide, named for where it comes from
    /// Dragging a guide back onto its ruler throws it away; rasterized by
    /// `cursor.rs` as the cursor that says so. The plain bin rather than `trash`,
    /// whose lid and handle collapse into noise at a cursor's 19pt.
    pub const TRASH_SIMPLE: &str = "\u{e4a8}";
    pub const CHECK: &str = "\u{e182}"; // a checked row in the View/Snap menus
    // The Transform panel's proportion lock: a whole chain when locked, a broken
    // one when not.
    pub const LINK_SIMPLE: &str = "\u{e2e6}";
    pub const LINK_SIMPLE_BREAK: &str = "\u{e2e8}";
    /// The typography popup's **skip-ink** toggle, beside the underline's line
    /// style (§15 D357).
    ///
    /// The only *horizontal* break in the link set: two chain ends facing each
    /// other across a gap, the picture of what skip-ink does to a band.
    /// `LINK_SIMPLE_BREAK` is diagonal and means the proportion lock.
    pub const LINK_SIMPLE_HORIZONTAL_BREAK: &str = "\u{e2ec}";
    pub const MAGNET: &str = "\u{e680}"; // the Snap menu
    /// The transform origin, in two weights. `CROSSHAIR_SIMPLE` is the read-only
    /// marker and the Transform panel's toggle; `CROSSHAIR` — the heavier one,
    /// whose lines run through the ring — is the marker while it can be dragged.
    /// One glyph in the panel and on the canvas, so the button names the thing it
    /// arms rather than describing it, and the weight says whether it is live.
    pub const CROSSHAIR_SIMPLE: &str = "\u{e1d8}";
    pub const CROSSHAIR: &str = "\u{e1d6}";
    /// A drag grip on a reorderable row — Fill's and Stroke's paint rows.
    pub const DOTS_SIX_VERTICAL: &str = "\u{eae2}";
    /// The rest of a stroke's properties, behind one button: dash, cap and join.
    pub const SLIDERS_HORIZONTAL: &str = "\u{e434}";
    /// The Settings modal, from the top bar (`crate::settings`).
    ///
    /// The upright sliders, deliberately paired with `SLIDERS_HORIZONTAL`, which
    /// opens the rest of one control's properties where this opens the app's own.
    /// Not a `GEAR`: nothing else in the app wears one.
    pub const SLIDERS: &str = "\u{e432}";
    // Undo/redo in the top bar. The design uses the U-turn arrows rather than
    // the circular ones (`ARROW_COUNTER_CLOCKWISE`), which read as "rotate".
    pub const ARROW_U_UP_LEFT: &str = "\u{e08a}";
    pub const ARROW_U_UP_RIGHT: &str = "\u{e08c}";
    /// `flex-wrap: wrap` in the Container card — a line running on and turning back
    /// under itself, the design's picture (§15 D878); `wrap-reverse` is
    /// [`ARROW_U_UP_LEFT`], turning back over.
    pub const ARROW_U_DOWN_LEFT: &str = "\u{e07e}";
    /// The Scale tool, in the rail and — rasterized by `cursor.rs` — as its
    /// cursor, so the button and the pointer are the same drawing.
    pub const RESIZE: &str = "\u{ed6e}";
    /// Select every layer using one Group Colors row's colour. A reticle rather
    /// than a cursor or a checkbox: the button *finds* things.
    pub const SCAN: &str = "\u{ebb6}";
    /// Which sides of a shape a stroke occupies — a box divided.
    pub const SQUARE_SPLIT_HORIZONTAL: &str = "\u{e870}";
    // The four boolean operations, in the order the dropdown lists them.
    pub const UNITE_SQUARE: &str = "\u{e878}";
    pub const SUBTRACT_SQUARE: &str = "\u{ebd4}";
    pub const INTERSECT_SQUARE: &str = "\u{e87a}";
    pub const EXCLUDE_SQUARE: &str = "\u{e880}";
    /// Flatten, at the foot of the same dropdown. Deliberately not a set diagram:
    /// it happens once and discards the operands rather than being a fifth state.
    /// Its ink box is narrower than the squares', but every Phosphor glyph advances
    /// the same, so the label beside it does not shift.
    pub const ARROWS_MERGE: &str = "\u{ed3e}";
    /// Dot and dash *spacing*: the gap between two things.
    pub const ARROWS_LEFT_RIGHT: &str = "\u{e0a0}";
    /// Fit the dash pattern to the shape's corners.
    pub const BOUNDING_BOX: &str = "\u{e6ce}";
    /// A custom dash pattern, typed as a list.
    pub const LIST_DASHES: &str = "\u{e2f4}";
    // The two horizontal sides, completing the set with `ARROW_UP`/`ARROW_DOWN`
    // above: each arrow points at the side whose width it sets.
    pub const ARROW_LEFT: &str = "\u{e058}";
    pub const ARROW_RIGHT: &str = "\u{e06c}";
    // --- typography ------------------------------------------------------
    // Paragraph alignment: a separate set from `ALIGN_*`, which aligns layers.
    pub const TEXT_ALIGN_LEFT: &str = "\u{e484}";
    pub const TEXT_ALIGN_CENTER: &str = "\u{e480}";
    pub const TEXT_ALIGN_RIGHT: &str = "\u{e486}";
    pub const TEXT_ALIGN_JUSTIFY: &str = "\u{e482}";
    /// Letter spacing — the `Aa` glyph, which is what tracking is *of*.
    pub const TEXT_AA: &str = "\u{e6ee}";
    /// Line height: the vertical arrow pair, matching `ARROWS_LEFT_RIGHT`'s use
    /// for dot spacing.
    pub const ARROWS_VERTICAL: &str = "\u{eb04}";
    /// The horizontal twin of [`ARROWS_VERTICAL`] (line height). Nothing draws it:
    /// letter spacing took [`TEXT_AA`].
    #[allow(dead_code)]
    pub const ARROWS_HORIZONTAL: &str = "\u{eb06}";
    pub const TEXT_BOLD: &str = "\u{e5be}";
    pub const TEXT_ITALIC: &str = "\u{e5c0}";
    pub const TEXT_UNDERLINE: &str = "\u{e5c4}";
    pub const TEXT_STRIKETHROUGH: &str = "\u{e5c2}";
    /// Baseline shift, in both directions — the two glyphs a designer knows the
    /// operation by.
    pub const TEXT_SUPERSCRIPT: &str = "\u{ec9a}";
    /// First-line indent, and its inverse for the hanging toggle.
    pub const TEXT_INDENT: &str = "\u{ea1e}";
    pub const TEXT_OUTDENT: &str = "\u{ea1c}";
    /// The paragraph's own start and end edges — the two **block** indents, which
    /// hold every line where `TEXT_INDENT` holds one.
    ///
    /// Arrows rather than text glyphs, so the two kinds of indent do not look like
    /// four states of one control. Each glyph is a bar against one edge with the
    /// arrow pointing **at** it: the bar is the margin the number is measured
    /// from (`ARROW_LINE_LEFT` for the start inset), and swapping the pair would
    /// put each number on the opposite margin.
    pub const ARROW_LINE_LEFT: &str = "\u{e062}";
    pub const ARROW_LINE_RIGHT: &str = "\u{e064}";
    /// Paragraph spacing — the pilcrow, which is the thing being spaced.
    pub const PARAGRAPH: &str = "\u{e960}";
    /// The language a run is shaped for.
    pub const TRANSLATE: &str = "\u{e4a2}";
    /// A line limit — a count, not a size.
    pub const LIST_NUMBERS: &str = "\u{e2f6}";
    /// Two overlapping right angles. Box trim (the box cropped to the type), and
    /// the crop tool on the rail — both callers, so neither name owns it.
    pub const CROP: &str = "\u{e1d4}";
    /// Text direction — the two-way swap.
    pub const SWAP: &str = "\u{e83c}";
    /// A swap property's filter on the Properties card (§15 D1000). `U+E268` beside
    /// it is the three-bar *funnel-simple*.
    pub const FUNNEL: &str = "\u{e266}";
    /// A decoration's thickness — a single stroke, seen end on.
    pub const LINE_VERTICAL: &str = "\u{ed70}";
    /// Where a word may be broken.
    pub const SCISSORS: &str = "\u{eae0}";
    /// "No decoration" — the empty cell of the decoration track. A bare rule,
    /// which is what an underline and a strikethrough both reduce to when there
    /// is no text under or through it.
    pub const MINUS: &str = "\u{e32a}";
    /// How a decoration's ink is broken up — a wave, the one of the four line
    /// styles that cannot be drawn as a straight rule.
    pub const WAVE_SINE: &str = "\u{ea9a}";
    /// Word spacing — the gap between two words being squeezed.
    pub const ARROWS_IN_LINE_HORIZONTAL: &str = "\u{e530}";
    /// *Copy* — the context menu's clipboard group (`docs/context-menus.md` §4).
    pub const COPY: &str = "\u{e1ca}";
    /// *Duplicate*, against [`COPY`]'s two full sheets: one sheet and its echo,
    /// which is what a duplicate leaves behind on the canvas.
    pub const COPY_SIMPLE: &str = "\u{e1cc}";
    /// *Paste*.
    pub const CLIPBOARD: &str = "\u{e196}";
    /// The outline hexagon §15 D981 gives components — an instance's mark. Regular
    /// weight only: the bundled font has no filled hexagon, so a main's mark and
    /// the *Create component* and *Duplicate as component* rows draw it
    /// (`ui::paint_hexagon_filled`, §15 D985) over this glyph's box.
    pub const HEXAGON: &str = "\u{e2ae}";
    /// A **kit** (§5.3e, §15 D1011): the top bar's *Kit* tag, the document menu's
    /// kit rows, and the insert and swap pickers' kit headings.
    pub const PACKAGE: &str = "\u{e390}";
    /// The styles and variables browser's button (§5.3f).
    pub const SWATCHES: &str = "\u{e5b8}";
    /// A style card's *Create style…* (§15 D1063).
    pub const PLUS_SQUARE: &str = "\u{ed4a}";
    /// The browser's `+` menu (G5): *Colour variable* and *Number variable*.
    pub const PALETTE: &str = "\u{e6c8}";
    pub const HASH: &str = "\u{e2a2}";
    /// *New mode* (G5).
    pub const COLUMNS_PLUS_RIGHT: &str = "\u{e542}";
    /// *Publish* (§5.3e).
    pub const UPLOAD_SIMPLE: &str = "\u{e4c0}";
    /// A kit update waiting (§5.3e).
    pub const ARROW_CIRCLE_DOWN: &str = "\u{e028}";
    /// *Detach instance* — "no longer from the main" (§15 D981).
    pub const LINK_BREAK: &str = "\u{e2e4}";
    /// *Go to main component*, everywhere it is offered — the menu row, the
    /// Component card's main link and its *Go to main* button — as the accepted
    /// mockups draw it (§15 D981).
    pub const ARROW_UP_RIGHT: &str = "\u{e092}";
    /// A variant clash's *Select it* — go to the other layer.
    pub const ARROW_SQUARE_OUT: &str = "\u{e5de}";
    /// *Select all instances*.
    pub const SELECTION_ALL: &str = "\u{e746}";
    /// A variant clash — two variants on one combination (§15 D982): the design's
    /// warning glyph, in neutral ink, no new hue.
    pub const WARNING: &str = "\u{e4e0}";
    /// A component property's binding mark — `{}`, used only inside mains (§15
    /// D982).
    pub const BRACKETS_CURLY: &str = "\u{e860}";
    /// A stack seen from the side. Drawn by nothing since *Bring to front* and
    /// *Send to back* took [`ARROW_LINE_UP`]/[`ARROW_LINE_DOWN`] (§15 D761). Kept
    /// under its own `allow` because the design's mockup uses it for *Bring to
    /// front*, so a reversal of that ruling reaches for it.
    #[allow(dead_code)]
    pub const STACK: &str = "\u{e466}";
    /// *Flatten* — the same stack pressed into one sheet.
    pub const STACK_SIMPLE: &str = "\u{e468}";
    /// *Delete*, in [`super::color::DANGER`]. The lidded bin rather than
    /// [`TRASH_SIMPLE`], which the layers panel's own row already spends.
    pub const TRASH: &str = "\u{e4a6}";
    /// *Show grid* — the one View switch with no glyph of its own until a menu
    /// needed one for it.
    pub const GRID_FOUR: &str = "\u{e296}";
    /// *Enter group* / *Enter boolean* — the one-level-in verbs (§5.2, §5.3).
    pub const SIGN_IN: &str = "\u{e428}";
    /// *Export original…* — the Asset row that writes a picture's stored bytes back
    /// out to a file (§15 D225).
    ///
    /// `export` rather than `download` or `download-simple`: all three are a tray
    /// with an arrow, but the downloads point *into* it and read as
    /// [`ARROW_LINE_DOWN`] with side walls, which at 15pt is the whole difference.
    /// `export` points up and out, so it contrasts by direction, and `download`
    /// adds a mark that is noise at icon size.
    pub const EXPORT: &str = "\u{eaf0}";
    /// The Export panel's format dropdown — a picture in a sheet, saying "this row
    /// makes a file" (§7, the design canvas's *Editor* artboard's export rows).
    ///
    /// One glyph for all three formats: Phosphor's `file-png`/`file-jpg`/`file-svg`
    /// are illegible micro-text at 15pt, and the word beside the glyph already
    /// names the format.
    pub const FILE_IMAGE: &str = "\u{ea24}";
    /// *Export as Zip* — the one button that produces an archive rather than
    /// files.
    pub const FILE_ZIP: &str = "\u{e958}";
    /// *JPEG quality* — a dial, for the one export setting that is a trade rather
    /// than a fact (§7).
    pub const GAUGE: &str = "\u{e628}";
    /// *Slashes in names make folders* — the Export menu's switch.
    ///
    /// Not [`FOLDER_OPEN`], which is the same menu's *Show last export folder*:
    /// that folder is being entered, this one is being made. One glyph for two
    /// verbs is the failure §15 D225 names.
    pub const FOLDER_SIMPLE: &str = "\u{e25a}";
    /// *Replace and reset* — the Asset row that swaps the picture and throws the
    /// framing away (§15 D225).
    ///
    /// A broom, because every ring was taken: `ARROW_COUNTER_CLOCKWISE` is *Reset
    /// crop*, `ARROWS_CLOCKWISE` is *reverse*, `ARROW_CLOCKWISE` is rotation. A
    /// fourth ring would be two verbs with one picture.
    pub const BROOM: &str = "\u{ec54}";
    /// The path bar's *Join* — two ends plugged together. Not [`ARROWS_MERGE`],
    /// which is *Flatten*.
    pub const PLUGS_CONNECTED: &str = "\u{eb5a}";
    /// The path bar's *Simplify*.
    pub const MAGIC_WAND: &str = "\u{e6b6}";
    /// The path bar's *Offset* — the outline pushed out on every side. Not
    /// [`CORNERS_OUT`], which is *Zoom to fit*.
    pub const ARROWS_OUT_CARDINAL: &str = "\u{e0a4}";
    /// Every icon this module names, paired with its Phosphor name (lower-kebab).
    ///
    /// Rust cannot reflect over `pub const`s, so this is the one list the tests
    /// read. The name is the join key: `declared_icons` derives it from each
    /// constant's identifier (`FOLDER_SIMPLE` → `"folder-simple"`), and
    /// `the_catalogue_lists_every_icon_it_declares` compares names and pairs
    /// glyphs by name.
    // `#[cfg(test)]`, not `#[allow(dead_code)]`: an allow-listed item is a live
    // root, which would give every constant it names a reader and restore the
    // blanket shield. Compiled out of the bin build, `dead_code` fires on the next
    // unused icon in the plain build, but not under `--all-targets`.
    #[cfg(test)]
    pub const ALL: &[(&str, &str)] = &[
        ("diamond", DIAMOND),
        ("caret-down", CARET_DOWN),
        ("caret-right", CARET_RIGHT),
        ("caret-up", CARET_UP),
        ("magnifying-glass", MAGNIFYING_GLASS),
        ("magnifying-glass-plus", MAGNIFYING_GLASS_PLUS),
        ("frame-corners", FRAME_CORNERS),
        ("selection", SELECTION),
        ("circle-half", CIRCLE_HALF),
        ("selection-plus", SELECTION_PLUS),
        ("selection-slash", SELECTION_SLASH),
        ("polygon", POLYGON),
        ("list", LIST),
        ("text-t", TEXT_T),
        ("image", IMAGE),
        ("circle", CIRCLE),
        ("square", SQUARE),
        ("triangle", TRIANGLE),
        ("star", STAR),
        ("play", PLAY),
        ("asterisk", ASTERISK),
        ("angle", ANGLE),
        ("eye", EYE),
        ("eye-slash", EYE_SLASH),
        ("lock-simple", LOCK_SIMPLE),
        ("lock-simple-open", LOCK_SIMPLE_OPEN),
        ("cursor", CURSOR),
        ("hand", HAND),
        ("line-segment", LINE_SEGMENT),
        ("pen-nib", PEN_NIB),
        ("bezier-curve", BEZIER_CURVE),
        ("eyedropper", EYEDROPPER),
        ("chat", CHAT),
        ("align-left", ALIGN_LEFT),
        ("align-center-horizontal", ALIGN_CENTER_HORIZONTAL),
        ("align-right", ALIGN_RIGHT),
        ("align-top", ALIGN_TOP),
        ("align-center-vertical", ALIGN_CENTER_VERTICAL),
        ("align-bottom", ALIGN_BOTTOM),
        ("arrow-clockwise", ARROW_CLOCKWISE),
        ("corners-out", CORNERS_OUT),
        ("number-square-one", NUMBER_SQUARE_ONE),
        ("plus", PLUS),
        ("dots-three", DOTS_THREE),
        ("share-network", SHARE_NETWORK),
        ("arrow-counter-clockwise", ARROW_COUNTER_CLOCKWISE),
        ("folder-open", FOLDER_OPEN),
        ("floppy-disk", FLOPPY_DISK),
        ("magnifying-glass-minus", MAGNIFYING_GLASS_MINUS),
        ("x", X),
        ("flip-horizontal", FLIP_HORIZONTAL),
        ("flip-vertical", FLIP_VERTICAL),
        ("arrows-clockwise", ARROWS_CLOCKWISE),
        ("arrows-down-up", ARROWS_DOWN_UP),
        ("drop-half", DROP_HALF),
        ("drop-half-bottom", DROP_HALF_BOTTOM),
        ("sun", SUN),
        ("square-half", SQUARE_HALF),
        ("arrow-elbow-left-down", ARROW_ELBOW_LEFT_DOWN),
        ("arrow-elbow-right-down", ARROW_ELBOW_RIGHT_DOWN),
        ("arrow-elbow-left-up", ARROW_ELBOW_LEFT_UP),
        ("arrow-elbow-right-up", ARROW_ELBOW_RIGHT_UP),
        ("arrow-up", ARROW_UP),
        ("arrow-down", ARROW_DOWN),
        ("arrow-line-up", ARROW_LINE_UP),
        ("arrow-line-down", ARROW_LINE_DOWN),
        ("arrows-out-line-horizontal", ARROWS_OUT_LINE_HORIZONTAL),
        ("arrows-out-line-vertical", ARROWS_OUT_LINE_VERTICAL),
        ("arrows-in-line-vertical", ARROWS_IN_LINE_VERTICAL),
        ("columns", COLUMNS),
        ("rows", ROWS),
        ("hand-grabbing", HAND_GRABBING),
        ("ruler", RULER),
        ("trash-simple", TRASH_SIMPLE),
        ("check", CHECK),
        ("link-simple", LINK_SIMPLE),
        ("link-simple-break", LINK_SIMPLE_BREAK),
        ("link-simple-horizontal-break", LINK_SIMPLE_HORIZONTAL_BREAK),
        ("magnet", MAGNET),
        ("crosshair-simple", CROSSHAIR_SIMPLE),
        ("crosshair", CROSSHAIR),
        ("dots-six-vertical", DOTS_SIX_VERTICAL),
        ("sliders-horizontal", SLIDERS_HORIZONTAL),
        ("sliders", SLIDERS),
        ("dots-three-vertical", DOTS_THREE_VERTICAL),
        ("clock-counter-clockwise", CLOCK_COUNTER_CLOCKWISE),
        ("file", FILE),
        ("files", FILES),
        ("squares-four", SQUARES_FOUR),
        ("pencil-simple", PENCIL_SIMPLE),
        ("arrow-u-up-left", ARROW_U_UP_LEFT),
        ("arrow-u-up-right", ARROW_U_UP_RIGHT),
        ("arrow-u-down-left", ARROW_U_DOWN_LEFT),
        ("resize", RESIZE),
        ("scan", SCAN),
        ("square-split-horizontal", SQUARE_SPLIT_HORIZONTAL),
        ("unite-square", UNITE_SQUARE),
        ("subtract-square", SUBTRACT_SQUARE),
        ("intersect-square", INTERSECT_SQUARE),
        ("exclude-square", EXCLUDE_SQUARE),
        ("arrows-merge", ARROWS_MERGE),
        ("arrows-left-right", ARROWS_LEFT_RIGHT),
        ("bounding-box", BOUNDING_BOX),
        ("list-dashes", LIST_DASHES),
        ("arrow-left", ARROW_LEFT),
        ("arrow-right", ARROW_RIGHT),
        ("text-align-left", TEXT_ALIGN_LEFT),
        ("text-align-center", TEXT_ALIGN_CENTER),
        ("text-align-right", TEXT_ALIGN_RIGHT),
        ("text-align-justify", TEXT_ALIGN_JUSTIFY),
        ("text-aa", TEXT_AA),
        ("arrows-vertical", ARROWS_VERTICAL),
        ("arrows-horizontal", ARROWS_HORIZONTAL),
        ("text-bold", TEXT_BOLD),
        ("text-italic", TEXT_ITALIC),
        ("text-underline", TEXT_UNDERLINE),
        ("text-strikethrough", TEXT_STRIKETHROUGH),
        ("text-superscript", TEXT_SUPERSCRIPT),
        ("text-indent", TEXT_INDENT),
        ("text-outdent", TEXT_OUTDENT),
        ("paragraph", PARAGRAPH),
        ("translate", TRANSLATE),
        ("list-numbers", LIST_NUMBERS),
        ("crop", CROP),
        ("swap", SWAP),
        ("funnel", FUNNEL),
        ("line-vertical", LINE_VERTICAL),
        ("scissors", SCISSORS),
        ("minus", MINUS),
        ("wave-sine", WAVE_SINE),
        ("arrows-in-line-horizontal", ARROWS_IN_LINE_HORIZONTAL),
        ("arrow-line-left", ARROW_LINE_LEFT),
        ("arrow-line-right", ARROW_LINE_RIGHT),
        ("copy", COPY),
        ("copy-simple", COPY_SIMPLE),
        ("clipboard", CLIPBOARD),
        ("hexagon", HEXAGON),
        ("package", PACKAGE),
        ("swatches", SWATCHES),
        ("plus-square", PLUS_SQUARE),
        ("palette", PALETTE),
        ("hash", HASH),
        ("columns-plus-right", COLUMNS_PLUS_RIGHT),
        ("upload-simple", UPLOAD_SIMPLE),
        ("arrow-circle-down", ARROW_CIRCLE_DOWN),
        ("link-break", LINK_BREAK),
        ("arrow-up-right", ARROW_UP_RIGHT),
        ("arrow-square-out", ARROW_SQUARE_OUT),
        ("selection-all", SELECTION_ALL),
        ("warning", WARNING),
        ("brackets-curly", BRACKETS_CURLY),
        ("stack", STACK),
        ("stack-simple", STACK_SIMPLE),
        ("trash", TRASH),
        ("grid-four", GRID_FOUR),
        ("sign-in", SIGN_IN),
        ("export", EXPORT),
        ("file-image", FILE_IMAGE),
        ("gauge", GAUGE),
        ("file-zip", FILE_ZIP),
        ("folder-simple", FOLDER_SIMPLE),
        ("broom", BROOM),
        ("plugs-connected", PLUGS_CONNECTED),
        ("magic-wand", MAGIC_WAND),
        ("arrows-out-cardinal", ARROWS_OUT_CARDINAL),
    ];
}

/// Name of the Phosphor font family registered by [`install`].
const PHOSPHOR: &str = "phosphor";

/// Build a `RichText` that renders `glyph` from the Phosphor icon font at
/// `size`, in `col`.
pub fn icon_text(glyph: &str, size: f32, col: egui::Color32) -> egui::RichText {
    egui::RichText::new(glyph)
        .family(egui::FontFamily::Name(PHOSPHOR.into()))
        .size(size)
        .color(col)
}

/// A `FontId` for the Phosphor icon font at `size` (for direct `painter.text`).
pub fn icon_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(PHOSPHOR.into()))
}

/// Install the theme's fonts and style onto the egui context. Call once at
/// startup.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    install_style(ctx);
    // `Ctrl` and `+`/`−` zoom the artwork (`Action::ZoomIn`/`ZoomOut`), not the
    // app: egui's own chord handling would rescale `pixels_per_point` on the same
    // keystroke. The app owns the keymap (`input::resolve`).
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
}

fn install_fonts(ctx: &egui::Context) {
    // Bundled Inter (the same variable font core shapes text with) for all UI
    // text, and Phosphor for icon glyphs.
    static INTER: &[u8] = include_bytes!("../../ondin-core/assets/fonts/Inter-Variable.ttf");
    static PHOSPHOR_TTF: &[u8] = include_bytes!("../assets/fonts/Phosphor.ttf");

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".to_owned(),
        Arc::new(egui::FontData::from_static(INTER)),
    );
    fonts.font_data.insert(
        PHOSPHOR.to_owned(),
        Arc::new(egui::FontData::from_static(PHOSPHOR_TTF)),
    );

    // Inter leads the proportional family (built-in fonts remain as fallbacks).
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());

    // A dedicated family for icon glyphs; falls back to Inter for any stray text.
    fonts.families.insert(
        egui::FontFamily::Name(PHOSPHOR.into()),
        vec![PHOSPHOR.to_owned(), "inter".to_owned()],
    );

    ctx.set_fonts(fonts);
}

fn install_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(install_style_into);
}

fn install_style_into(style: &mut egui::Style) {
    use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

    // Compact type scale (design body is 13px; egui points ≈ CSS px here).
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(12.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(12.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(10.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
    ]
    .into();

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = Some(color::TEXT);
    v.panel_fill = color::BG;
    v.window_fill = color::CARD;
    v.extreme_bg_color = color::FIELD; // text-edit / input backgrounds
    v.faint_bg_color = color::CARD;
    v.hyperlink_color = color::ACCENT;
    v.window_stroke = Stroke::new(1.0, color::DIVIDER);
    v.window_corner_radius = CornerRadius::same(6);

    // Selection (used by text fields, selectable_label highlight, etc.).
    v.selection.bg_fill = color::ACCENT_800;
    v.selection.stroke = Stroke::new(1.0, color::ACCENT_300);

    let r5 = CornerRadius::same(5);
    let w = &mut v.widgets;

    // Non-interactive (labels, separators): no fill, faint divider strokes.
    w.noninteractive.bg_fill = Color32::TRANSPARENT;
    w.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    w.noninteractive.bg_stroke = Stroke::new(1.0, color::DIVIDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, text::MUTED);
    w.noninteractive.corner_radius = r5;

    // Inactive (resting buttons / controls): field ground, hairline border.
    w.inactive.bg_fill = color::FIELD;
    w.inactive.weak_bg_fill = color::FIELD;
    w.inactive.bg_stroke = Stroke::new(1.0, color::FIELD_BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, text::STRONG);
    w.inactive.corner_radius = r5;
    w.inactive.expansion = 0.0;

    // Hovered: a touch lighter, accent-tinted border.
    w.hovered.bg_fill = color::HOVER;
    w.hovered.weak_bg_fill = color::HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, color::ACCENT_700);
    w.hovered.fg_stroke = Stroke::new(1.0, color::TEXT);
    w.hovered.corner_radius = r5;
    w.hovered.expansion = 0.0;

    // Active/pressed and open: accent-tinted ground.
    for ws in [&mut w.active, &mut w.open] {
        ws.bg_fill = color::ACCENT_800;
        ws.weak_bg_fill = color::ACCENT_800;
        ws.bg_stroke = Stroke::new(1.0, color::ACCENT);
        ws.fg_stroke = Stroke::new(1.0, color::ACCENT_100);
        ws.corner_radius = r5;
        ws.expansion = 0.0;
    }

    // egui's "this rect changed id" warning is off: it paints a red outline for a
    // frame whenever a Fill or Stroke list gets shorter, because paint rows are
    // salted with the paint's index (`carried_unit`) and deleting one shifts the
    // top row's id. Its state should not carry over, so the warning is a false
    // positive. No per-widget switch exists; `warn_on_id_clash` stays on.
    //
    // Guarded because `Style::debug` does not exist in a release build, and only
    // `cargo build --release` would catch an unguarded line.
    #[cfg(debug_assertions)]
    {
        style.debug.warn_if_rect_changes_id = false;
    }

    // No label is selectable text: a selectable label shows an I-beam and drags
    // out a highlight, wrong for chrome, especially headers that toggle a card.
    // Off at the style rather than per label. Labels that act as buttons keep an
    // explicit `Sense::click()`, since `selectable` and `sense` are separate
    // fields; a `TextEdit` sets its own cursor.
    style.interaction.selectable_labels = false;

    // Dense spacing (Nocturne is 0.7× density).
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(7.0, 6.0);
    s.button_padding = egui::vec2(8.0, 4.0);
    s.interact_size = egui::vec2(24.0, 24.0);
    s.icon_width = 15.0;
    s.icon_width_inner = 10.0;
    s.menu_margin = egui::Margin::same(6);
    s.indent = 16.0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    /// Every glyph the app names is laid out, read back out of the font atlas, and
    /// has to have ink that differs from the missing-glyph box. A wrong Phosphor
    /// codepoint lays out fine and renders blank or as `.notdef`; the font's `post`
    /// table is version 3.0 and holds no names to check against. It reads
    /// `icon::ALL`, so coverage cannot drift behind the constants. It cannot say
    /// whether a glyph is the one that was wanted: a digit transposed onto a
    /// neighbouring valid icon passes, and only an atlas dump catches that.
    #[test]
    fn every_named_icon_has_a_glyph_behind_it() {
        let ctx = egui::Context::default();
        install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let icons: &[(&str, &str)] = icon::ALL;

        // The glyph's size and alpha out of the atlas, which for premultiplied
        // white text *is* the drawing. Counting ink is not enough: a codepoint the
        // font lacks lays out as a `.notdef` box, so a missing glyph is detected by
        // matching that box's fingerprint.
        let coverage = |glyph: &str| -> (usize, usize, Vec<u8>) {
            let galley = ctx.fonts_mut(|f| {
                f.layout_no_wrap(glyph.to_owned(), icon_font(16.0), egui::Color32::WHITE)
            });
            let Some(uv) = galley
                .rows
                .first()
                .and_then(|r| r.row.glyphs.first())
                .map(|g| g.uv_rect)
            else {
                return (0, 0, Vec::new());
            };
            if uv.is_nothing() {
                return (0, 0, Vec::new());
            }
            let image = ctx.fonts(|f| f.image());
            let (x0, x1) = (u32::from(uv.min[0]), u32::from(uv.max[0]));
            let (y0, y1) = (u32::from(uv.min[1]), u32::from(uv.max[1]));
            let mut px = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = y as usize * image.size[0] + x as usize;
                    px.push(image.pixels.get(i).map_or(0, |p| p.a()));
                }
            }
            ((x1 - x0) as usize, (y1 - y0) as usize, px)
        };

        // A Private Use Area codepoint Phosphor does not define, so the result is
        // the fallback's answer.
        let notdef = coverage("\u{efff}");
        assert!(
            notdef.2.iter().any(|a| *a > 0),
            "the fallback for a missing glyph drew nothing, so it can no longer be \
             told apart from a real one — this test needs a new sentinel"
        );

        for (name, glyph) in icons {
            let got = coverage(glyph);
            let ink = got.2.iter().filter(|a| **a > 0).count();
            assert!(
                ink > 8,
                "{name} (U+{:04X}) inked {ink} texels — the codepoint is wrong or the \
                 font does not have it",
                glyph.chars().next().map(u32::from).unwrap_or(0)
            );
            assert!(
                got != notdef,
                "{name} (U+{:04X}) came back as the *missing-glyph box*, not as an icon \
                 — Phosphor does not define that codepoint",
                glyph.chars().next().map(u32::from).unwrap_or(0)
            );
        }
    }

    /// The two block-indent arrows are different glyphs and each carries its bar on
    /// the edge its number measures from (see `icon::ARROW_LINE_LEFT`). Column ink
    /// is asserted rather than a pixel-exact mirror, since antialiasing is not
    /// mirrored.
    #[test]
    fn the_block_indent_arrows_bar_the_edge_they_measure_from() {
        let ctx = egui::Context::default();
        install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        // Per-column ink, read out of the font atlas.
        let columns = |glyph: &str| -> Vec<u32> {
            let galley = ctx.fonts_mut(|f| {
                f.layout_no_wrap(glyph.to_owned(), icon_font(24.0), egui::Color32::WHITE)
            });
            let uv = galley.rows[0].row.glyphs[0].uv_rect;
            let image = ctx.fonts(|f| f.image());
            let (x0, x1) = (u32::from(uv.min[0]), u32::from(uv.max[0]));
            let mut ink = vec![0_u32; (x1 - x0) as usize];
            for y in u32::from(uv.min[1])..u32::from(uv.max[1]) {
                for x in x0..x1 {
                    let i = y as usize * image.size[0] + x as usize;
                    ink[(x - x0) as usize] += u32::from(image.pixels.get(i).map_or(0, |p| p.a()));
                }
            }
            ink
        };
        let left = columns(icon::ARROW_LINE_LEFT);
        let right = columns(icon::ARROW_LINE_RIGHT);
        assert_ne!(
            left, right,
            "the start and end indents would draw the same icon"
        );
        for (name, ink, bar_at_start) in [
            ("arrow-line-left", &left, true),
            ("arrow-line-right", &right, false),
        ] {
            let (first, last) = (ink[0], ink[ink.len() - 1]);
            let (bar, away) = if bar_at_start {
                (first, last)
            } else {
                (last, first)
            };
            assert!(
                bar > away * 2,
                "{name} should be a solid bar against {} edge and near-empty at the \
                 other; column ink was {ink:?}",
                if bar_at_start { "the start" } else { "the end" }
            );
        }
    }

    /// Every icon constant this module's own source declares, as its Phosphor name
    /// paired with the glyph behind it.
    ///
    /// Rust cannot enumerate `pub const`s, so this parses this file's own source
    /// (`include_str!`), from `pub mod icon {` to the first closing brace at column
    /// 0. The name is derived from the identifier (`FOLDER_SIMPLE` →
    /// `"folder-simple"`), which lets the test below check the pairing as well as
    /// the membership.
    fn declared_icons() -> Vec<(String, String)> {
        let src = include_str!("theme.rs");
        let body = src
            .split_once("pub mod icon {")
            .expect("the icon module moved or was renamed")
            .1
            .split_once("\n}\n")
            .expect("the icon module has no closing brace at column 0")
            .0;
        let mut out = Vec::new();
        for line in body.lines() {
            let Some(rest) = line.trim_start().strip_prefix("pub const ") else {
                continue;
            };
            let Some((name, rest)) = rest.split_once(':') else {
                continue;
            };
            // `ALL` is the catalogue, not an icon in it.
            if name == "ALL" {
                continue;
            }
            let hex = rest
                .split_once("\\u{")
                .unwrap_or_else(|| panic!("`{name}` is not a single `\\u{{…}}` escape: {rest}"))
                .1
                .split_once('}')
                .unwrap_or_else(|| panic!("`{name}` has an unterminated `\\u{{` escape"))
                .0;
            let glyph = u32::from_str_radix(hex, 16)
                .ok()
                .and_then(char::from_u32)
                .unwrap_or_else(|| panic!("`{name}`'s escape U+{hex} is not a character"));
            out.push((
                name.to_ascii_lowercase().replace('_', "-"),
                glyph.to_string(),
            ));
        }
        out
    }

    /// `icon::ALL` has to list every constant the module declares, or the atlas
    /// test silently stops covering the newest icons. A mismatch means an icon was
    /// added without being listed: add it, do not relax this test. Count equality
    /// would miss a count-preserving corruption, so names, pairing and glyph
    /// distinctness are asserted separately.
    ///
    /// flip: `("selection", SELECTION)` → `("diamond", DIAMOND)` → fails at the
    /// duplicate-name assertion; one entry deleted → fails at the membership
    /// assertion; `("selection", DIAMOND)` beside `("diamond", DIAMOND)` → fails
    /// at the pairing assertion; `SELECTION`'s codepoint set to `DIAMOND`'s →
    /// fails at the distinctness assertion, with the other tests green.
    #[test]
    fn the_catalogue_lists_every_icon_it_declares() {
        let declared = declared_icons();
        let by_name: std::collections::BTreeMap<&str, &str> = declared
            .iter()
            .map(|(name, glyph)| (name.as_str(), glyph.as_str()))
            .collect();
        assert_eq!(
            by_name.len(),
            declared.len(),
            "two constants in the icon module derive one Phosphor name, so one of \
             them can never be listed"
        );

        let mut listed: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
        for (name, glyph) in icon::ALL {
            assert!(
                listed.insert(name, glyph).is_none(),
                "`{name}` is listed twice in ALL, which means some other constant \
                 is listed not at all — and the atlas test above only ever sees ALL"
            );
        }

        let missing: Vec<&str> = by_name
            .keys()
            .copied()
            .filter(|name| !listed.contains_key(name))
            .collect();
        let unknown: Vec<&str> = listed
            .keys()
            .copied()
            .filter(|name| !by_name.contains_key(name))
            .collect();
        assert!(
            missing.is_empty() && unknown.is_empty(),
            "ALL and the constants disagree. Declared but not listed: {missing:?} — \
             add them to ALL, or the atlas test stops covering them. Listed but not \
             declared: {unknown:?} — a name in ALL that no constant derives is a \
             Phosphor name nothing draws."
        );

        for (name, glyph) in &listed {
            assert_eq!(
                by_name[name], *glyph,
                "ALL pairs `{name}` with a glyph that is not the one the constant \
                 of that name holds — so one icon is drawn under another's name and \
                 the count, the names and the atlas test all agree it is fine"
            );
        }

        let mut glyphs: Vec<&str> = listed.values().copied().collect();
        glyphs.sort_unstable();
        let before = glyphs.len();
        glyphs.dedup();
        assert_eq!(
            before,
            glyphs.len(),
            "two names in ALL share one codepoint. That is the *two verbs, one \
             picture* failure `icon::BROOM`'s own comment is about, and it is what a \
             copy-paste slip in the table produces; {} of {before} entries are \
             distinct",
            glyphs.len()
        );
    }

    /// The four side arrows have to be four *different* pictures, each pointing at
    /// the side whose width it labels. Four copies of one arrow would read as a
    /// decoration and leave the fields unidentifiable — and is exactly what a
    /// copy-paste slip in the table above produces.
    #[test]
    fn the_four_side_arrows_are_four_different_glyphs() {
        let all = [
            icon::ARROW_UP,
            icon::ARROW_RIGHT,
            icon::ARROW_DOWN,
            icon::ARROW_LEFT,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "two of the side arrows are the same codepoint");
            }
        }
    }
}
