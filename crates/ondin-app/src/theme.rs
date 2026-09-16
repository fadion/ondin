//! Visual theme for the app chrome — the "Nocturne" dark design realized in
//! egui (see `design/Editor.dc.html`, the source of truth for this screen).
//!
//! The Editor design overrides Nocturne's `:root` accent to a steel blue and
//! builds every panel ground by mixing the near-neutral text colour toward
//! black, so the chrome reads as neutral dark grey with a single blue accent.
//! Those computed values are frozen here as `Color32` constants (mixing at
//! runtime every frame would be wasteful and egui has no `color-mix`).
//!
//! egui types stay confined to `ondin-app`; this module is where the design
//! tokens become egui `Style`/`Visuals`/fonts. Change a token here and the
//! whole chrome follows.

use eframe::egui;
use std::sync::Arc;

/// Palette — frozen from `design/Editor.dc.html`'s effective values.
///
/// Grounds are `color-mix(in srgb, var(--color-text) N%, #000)` with
/// text = `#e9e9ed` (233, 233, 237); the accent ramp is the Editor's `:root`
/// override (a steel blue, not Nocturne's stock blurple).
// **No `#[allow(dead_code)]`, and there was one** (§15 D672, `[S18.3-L3-04]`). Its
// receipt read *"full ramp kept as a palette for later phases"* and described a
// state that no longer exists: with the attribute removed,
// `cargo check -p ondin-app` reports **zero** warnings here — every constant and
// both `const fn`s have a production reader outside this file. The palette has a
// working `dead_code` for the first time, which is what will say so when the next
// colour loses its last caller.
//
// ⚠️ **No count here on purpose.** This paragraph said *"all 34"* for one commit
// and the module holds a different number; a count in prose is a gate nobody
// built, and the gate is the two words above it — run the command.
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
    /// ⚠️ **It was 53, and 53 is the same 8% resolved against the wrong ground.**
    /// The design writes `border: 1px solid color-mix(in srgb, var(--color-text)
    /// 8%, transparent)` on a box whose `background` is already `text 5%`, and CSS
    /// paints a background under the border area (`background-clip: border-box` is
    /// the default), so a translucent border composites over the element's *own*
    /// ground rather than over what the element is sitting on. Reading it as 8%
    /// over the card gives `0.08·233 + 0.92·37 = 52.7`; reading it as the design
    /// draws it gives `0.08·233 + 0.92·47 = 61.9`, and the blue channel
    /// `0.08·237 + 0.92·48 = 63.1`.
    ///
    /// The difference is the whole complaint: 53 stands **6 levels** above the 47
    /// it outlines, which is under the threshold at which a 1px line on a dark
    /// panel registers at all — reported as *"so dim I thought it wasn't even
    /// there"*. 62 stands 15 above the field and 25 above the card (§15 D386).
    ///
    /// [`CARD_BORDER`] is the same 8% over the card ground, for the boxes whose
    /// fill really is [`CARD`]; the two differ because a translucent border is
    /// brighter the brighter the thing it encloses, which is the design's rule
    /// rather than an inconsistency.
    pub const FIELD_BORDER: Color32 = Color32::from_rgb(62, 62, 63);

    /// Hairline round a box filled with [`CARD`] — the dashboard's project and
    /// file cards, and the *New file* slot's dashed outline.
    ///
    /// Text 8% over [`CARD`], which is what [`FIELD_BORDER`] used to hold for
    /// everything. Kept as its own name rather than folded back in: these three
    /// are the only strokes in the app that enclose a card ground rather than a
    /// field ground, and against a 37 ground 62 would read as a lit edge — which
    /// is what a *hovered* card already means (`ACCENT_700`).
    pub const CARD_BORDER: Color32 = Color32::from_rgb(53, 53, 54);

    /// The ground every hovered control in the app paints (§15 D484).
    ///
    /// **The single most-repeated colour in the chrome, and it was a literal at
    /// four sites in three files and named at none of them** (`[S18.2-L3-02]`) —
    /// `theme::install`'s two `w.hovered` fills, which is what **egui itself**
    /// paints for every stock widget; `ui::icon_button_padded`, the tool-rail and
    /// top-bar family; `ui::button_face`, the field-row family; and
    /// `inspector::template_cell`, *Frame templates*' cells. Four places that have
    /// to agree, and a change to any one of them splits the hover into two greys
    /// with every gate green — the stock half and the hand-rolled half being the
    /// two that would diverge.
    ///
    /// The argument is `ui::SEGMENT_BORDER`'s — plain backticks, it being in
    /// another module and this being the sort of link a rename breaks silently —
    /// which the chrome already makes about a smaller number: *"it is subtracted in
    /// one place and painted in another, and those two have to agree."* And
    /// [`CARD_BORDER`] above is `(53, 53, 54)`, one level away from this and named,
    /// which is how close the omission was.
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
    /// *not* the chrome accent.
    ///
    /// The panels are steel blue, and artwork is frequently blue too; a
    /// selection outline in the same family disappears into both. This hue is
    /// far enough off the panel accent to read as chrome and light enough to
    /// hold against dark artwork, so a marquee, a bounding box, a layer
    /// highlight or a snap guide is always the app talking rather than part of
    /// the drawing.
    pub const SELECT: Color32 = Color32::from_rgb(0x63, 0xb3, 0xf5);
    /// The same hue at ~55%, for the parts of the chrome that must not compete
    /// with the outline itself: pen control handles, the entered-group hint.
    pub const SELECT_DIM: Color32 = select_a(0x8c);
    /// The layer-row highlight in the layers tree — the same hue as the canvas
    /// selection, at panel strength, so a selected row and its bounding box are
    /// visibly the same statement.
    pub const SELECT_ROW: Color32 = select_a(43);

    /// The **key layer** outline: the one member of a selection that align aligns
    /// to and that a boolean takes as its base.
    ///
    /// **The one place a second chrome hue is right, and it has to be one.** §15
    /// D13 reserves [`SELECT`] for "the app is talking about the selection" and
    /// says to derive anything that wants it faded rather than paste a fourth
    /// literal — but a shade of `SELECT` cannot work here, because every other
    /// member of the selection is already wearing `SELECT` and the mark's whole
    /// job is to separate one of them from the rest. So: a different hue at the
    /// same weight, which reads as the same kind of statement in a different
    /// register rather than as a new visual language. Magenta because it is the
    /// far side of the wheel from the blue and from the steel panels both, and
    /// light because the artwork below is unknown.
    ///
    /// Tuned on screen 2026-08-02: the first draft (`ef8fe0`) was too light and
    /// too desaturated to hold its own beside the blue, and read as lost rather
    /// than as a second statement.
    pub const KEY: Color32 = Color32::from_rgb(0xd2, 0x64, 0xc8);

    /// [`SELECT`] at an alpha, premultiplied.
    ///
    /// Derived rather than written out, so retuning the hue carries every tier
    /// with it — the two below were hand-premultiplied against the old blue and
    /// would have been left behind by a one-line colour change.
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
    /// measure and of the gaps an equal-gap snap has just made (`measure.rs`, §15
    /// D198/D200).
    ///
    /// Both go through `canvas::draw_measure`, so this hue means one thing whether
    /// the app is *reporting* a distance or has just *created* one. That is worth
    /// keeping true: the two arrive in different postures — one on a resting pointer,
    /// one mid-drag — and giving them separate colours would make the same number
    /// look like two different kinds of claim.
    ///
    /// **The third chrome hue, and the last one this scheme has room for.** §15
    /// D13 reserves [`SELECT`] for "the app is talking about the selection" and
    /// [`KEY`] argues its own case for a second; the argument here is the same
    /// shape and it is Figma's, which matters because a measurement is the one
    /// overlay users arrive already knowing: red is what a distance annotation
    /// looks like in every tool that has one. A shade of `SELECT` would also
    /// mislead — the measure is a statement about the *gap* between the selection
    /// and something that is not selected, so wearing the selection's colour
    /// would attach it to one of the two boxes.
    ///
    /// Light rather than the guides' dark red ([`ondin_core::DEFAULT_GUIDE_COLOR`],
    /// `a3332d`), and that separation has to hold: measures are frequently drawn
    /// *to* a guide, so the number and the line it is measuring from must not read
    /// as the same object.
    pub const MEASURE: Color32 = Color32::from_rgb(0xf2, 0x6d, 0x6d);
    /// Ink on [`MEASURE`], for [`BADGE_INK`]'s reason: the pill's ground is light.
    pub const MEASURE_INK: Color32 = Color32::from_rgb(0x2b, 0x0c, 0x0c);

    /// A layer the app cannot draw as the document describes it — today, one
    /// whose picture is missing (§15 D179).
    ///
    /// **Amber rather than red**, and that is the whole of what it means: the
    /// usual cause is a file that moved, which is a thing to fix rather than a
    /// mistake in the document, and the app has nothing else red to grade it
    /// against. It tints the row's *existing* kind glyph rather than adding a
    /// mark: the icon is the one part of a row that cannot be renamed away, and a
    /// tint costs no width in a row that has none to give.
    pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xa0, 0x4a);

    /// The star on a starred document's card (`design/Dashboard.dc.html`'s
    /// `starColor`, `#e0a93c`).
    ///
    /// ⚠️ **Its own constant rather than [`WARN`], which is two points away and
    /// would have done.** The two are near-identical golds meaning opposite
    /// things — one is *this cannot be drawn*, the other is *you marked this* —
    /// and a shared constant is how the day one of them is re-tuned takes the
    /// other with it. They are near-identical because both are the one warm hue
    /// this palette has, not because they are the same idea.
    pub const STAR: Color32 = Color32::from_rgb(0xe0, 0xa9, 0x3c);

    /// A row that destroys something — today, *Delete* in a context menu
    /// (`design/Editor.dc.html`, the `[data-canvas-menu]` block).
    ///
    /// **The design's one red, and the app's first.** It appears exactly once in
    /// the whole export, on that row, which is what makes it a colour with a
    /// meaning rather than a decoration: everything else in the menu is neutral,
    /// so the eye lands on the one row it can lose work with. Named here rather
    /// than written at the call site for the reason every other constant in this
    /// module is — the second destructive row must be able to be the same red
    /// without anyone having to find the first one.
    ///
    /// Distinct from [`WARN`] on purpose: amber says "this layer cannot be drawn
    /// as described", red says "this click removes it", and grading one into the
    /// other would make the amber icon read as a threat.
    pub const DANGER: Color32 = Color32::from_rgb(0xe0, 0x73, 0x6b);

    /// The three steps a *filled* destructive button needs, mirroring
    /// [`ACCENT_900`]/[`ACCENT_700`]/[`ACCENT_200`] — ground, border and ink for
    /// [`super::super::ui::FieldButton::Danger`].
    ///
    /// **Computed from [`DANGER`] by the ratios the accent ramp already uses**
    /// (×0.31 for the ground, ×0.66 for the border, and 61.5% of the way to white
    /// for the ink) rather than picked by eye, so the red button is the blue one's
    /// twin in weight and the pair cannot drift apart at different rates. They are
    /// arithmetic, not design: worth a look on screen, and the only three numbers
    /// to change if the red wants to be louder or quieter.
    /// …and a fourth for the hovered ground, mirroring [`ACCENT_800`], which is
    /// what [`ACCENT_900`] lightens to under the pointer.
    pub const DANGER_900: Color32 = Color32::from_rgb(69, 36, 33);
    pub const DANGER_800: Color32 = Color32::from_rgb(101, 52, 48);
    pub const DANGER_700: Color32 = Color32::from_rgb(148, 76, 71);
    pub const DANGER_200: Color32 = Color32::from_rgb(243, 201, 198);

    /// The ground of an *unselected* point marker in the node tool.
    ///
    /// **Opaque, not the artwork showing through.** A resize handle is hollow on
    /// purpose (§15 D14): the corner it marks is the thing being aimed at, so the
    /// handle must not paint over it. A path anchor is the opposite — the marker
    /// *is* the target, it sits on ink of any colour, and a hollow 7px square on
    /// a dark stroke is a square nobody can see. Near-white rather than the panel
    /// greys, because it has to read on the canvas rather than in a panel.
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
    /// secondary labels — text that is faint because it is not the point, not
    /// because it is unavailable.
    ///
    /// **79 is not chosen, it is matched.** `Ui::disable` fades its scope by
    /// `Visuals::disabled_alpha` — 0.5 — so a [`MUTED`] glyph disabled that way
    /// paints at 79 whatever any constant here says. The app also disables by
    /// *colour*, at `icon_button`'s `enabled: false` and `FieldButton::Disabled`,
    /// and that route used to be `FAINT`. Two routes to one meaning were landing
    /// on two different inks; this is the second one moved onto the first.
    ///
    /// **What it buys is separation from "off".** A hidden paint row is
    /// `HIDDEN_OPACITY` (0.6) over a MUTED glyph — 95 — and a hidden or locked
    /// layer row is `text_a(102)`. Against `FAINT`'s 97 those are 2 and 5 levels
    /// apart, which is no distance at all: *dimmed because off* and *dimmed
    /// because unavailable* were the same grey, so dim alone read as "inert, don't
    /// bother" on a control that was merely switched off. At 79 the gap is 16–23,
    /// the same order as the step that settled `SWATCH_HAIRLINE`.
    ///
    /// Both off tiers are deliberately untouched: `HIDDEN_OPACITY` was set by eye
    /// against the real panel, and `layers.rs` argues its own case for locked and
    /// hidden sharing one grey. This moves the tier that had no such argument.
    pub const DISABLED: Color32 = text_a(79); // ~31%
}

/// Phosphor icon glyphs (Regular weight), by name. Codepoints extracted from
/// `@phosphor-icons/web@2.1.1`. Render with [`icon_text`] so they use the
/// Phosphor font family.
///
/// # Adding an icon
///
/// Look the name up in **`assets/fonts/Phosphor.codepoints.txt`** and paste the
/// codepoint in. That table is the answer to a question this comment used to pose
/// as a chore: `Phosphor.ttf`'s **`post` table is version 3.0**, which carries no
/// glyph names at all, so there is nothing to grep. It was recovered once, from
/// the font's own ligatures, and checked against every constant below; the file's
/// header says how to regenerate it and why single-word names (`x`, `check`) are
/// missing from it.
// 🚨 **The allow is on the items that need it, not on the module** (§15 D672,
// `[S18.3-L3-04]`). It used to sit here — *"full icon catalog kept for later
// phases"* — switched off across all 143 constants to silence **four**, and the
// receipt made that read as policy rather than as the accident it was.
//
// ⚠️ **`icon::ALL` does not shield the rest, and that is the plausible wrong
// answer.** `ALL` names every constant, so the obvious reading is that `dead_code`
// could never fire on one; it fires anyway, because `ALL` is read only from
// `#[cfg(test)]` and in the bin build the whole chain through it is dead. Measured
// both ways — see the note on `ALL` itself, which is where the interesting half
// is: an `#[allow]` there would have put the shield straight back.
//
// So four constants carry their own allow, each with the reason it is kept. That
// is the difference between a catalogue and an accident: a glyph nobody draws is
// now something somebody wrote down, and the **next** one to lose its last call
// site is a warning rather than a silence.
pub mod icon {
    pub const DIAMOND: &str = "\u{e1ec}";
    pub const CARET_DOWN: &str = "\u{e136}";
    pub const CARET_RIGHT: &str = "\u{e13a}";
    /// The unused half of the caret trio (§15 D672). `CARET_DOWN` and
    /// `CARET_RIGHT` are the closed/open pair every disclosure in the app draws;
    /// nothing points a caret up yet, and a trio missing one member is the shape
    /// that reads as deliberate and is not — so it says so here.
    #[allow(dead_code)]
    pub const CARET_UP: &str = "\u{e13c}";
    pub const MAGNIFYING_GLASS: &str = "\u{e30c}";
    pub const MAGNIFYING_GLASS_PLUS: &str = "\u{e310}";
    pub const FRAME_CORNERS: &str = "\u{e626}";
    pub const SELECTION: &str = "\u{e69a}";
    /// The identity row's three actions, beside the boolean dropdown (§9.4).
    /// Confirmed by atlas dump rather than taken from the table — `Phosphor.ttf`'s
    /// `post` table is version 3.0 and holds no glyph names, so a wrong codepoint
    /// lays out perfectly and renders nothing at all.
    ///
    /// Also the Effects panel's **Layer blur** row, which is the design's own
    /// choice (`ph-circle-half`). The two are far apart — a 15pt row glyph in a
    /// list of four against a 30pt action button on the identity card — and the
    /// alternative was picking a second blur picture the design did not ask for.
    pub const CIRCLE_HALF: &str = "\u{e18c}"; // mask
    pub const SELECTION_PLUS: &str = "\u{e69c}"; // group
    pub const SELECTION_SLASH: &str = "\u{e69e}"; // ungroup
    /// A `Path`'s kind glyph in the layers tree — and, since §15 D230, the
    /// *Outline shape* row's, because what that row produces **is** a path.
    /// `context-menus.md` §8's rule that a verb's picture comes from the registry
    /// reads the other way too: a verb whose whole result is a kind should wear that
    /// kind's picture.
    pub const POLYGON: &str = "\u{e6d0}";
    pub const LIST: &str = "\u{e2f0}";
    pub const TEXT_T: &str = "\u{e48a}";
    pub const IMAGE: &str = "\u{e2ca}";
    pub const CIRCLE: &str = "\u{e18a}";
    pub const SQUARE: &str = "\u{e45e}";
    pub const TRIANGLE: &str = "\u{e4b0}"; // the polygon tool — it starts as one
    pub const STAR: &str = "\u{e46a}";
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
    /// **Vertical where the editor's overflow is horizontal**, which is not a
    /// near-miss like `SLIDERS`/`SLIDERS_HORIZONTAL`: a row's own menu is
    /// conventionally vertical (it reads as belonging to the row, not to the
    /// bar above it) and the design draws it that way.
    pub const DOTS_THREE_VERTICAL: &str = "\u{e208}";
    /// The dashboard's *Recent*.
    pub const CLOCK_COUNTER_CLOCKWISE: &str = "\u{e1a0}";
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
    /// **These two are not a matched pair, which is worth knowing before reaching
    /// for them as one.** Read out of the atlas: `flip-horizontal` is two upward
    /// triangles side by side about a *dashed* vertical axis, while
    /// `flip-vertical` hangs two right-triangles off a *solid* bar down the left
    /// and splits them with a *solid double* rule. Different construction,
    /// different weight, different treatment of the mirror line — side by side the
    /// second reads as a flag pointing right rather than as the first one turned.
    /// Phosphor offers no second flip pair to swap in, which is why the image
    /// popover's mirror buttons are words instead (§15 D182).
    pub const FLIP_HORIZONTAL: &str = "\u{ed6a}";
    /// Mirror in y. See [`FLIP_HORIZONTAL`] — the two do not match.
    pub const FLIP_VERTICAL: &str = "\u{ed6c}";
    pub const ARROWS_CLOCKWISE: &str = "\u{e094}"; // reverse a gradient's stops
    /// The dashboard's sort control, which is one button carrying whichever order
    /// is in force (`design/Dashboard.dc.html`, `ph-arrows-down-up`).
    ///
    /// **Not [`ARROWS_CLOCKWISE`], which is the other candidate and means
    /// *reverse*.** These arrows pass each other going opposite ways, which is
    /// "ordered by"; a ring of arrows is "turn this round", and the sort button
    /// does not reverse anything — it steps to the next key.
    pub const ARROWS_DOWN_UP: &str = "\u{e098}";
    /// Opacity — and, since the Effects panel, the **inner shadow**'s row glyph,
    /// which is the picture the design gives it (`ph-drop-half`).
    ///
    /// Two meanings for one drawing, and deliberately so rather than by
    /// oversight: a half-filled drop is "part of this is dark", which is what an
    /// inner shadow does to a shape and what an opacity does to a colour. They
    /// never appear in the same card. The pair below is what makes the shadow
    /// reading work, and it was read out of the atlas rather than assumed: both
    /// are the same droplet outline with half of it filled by Phosphor's zigzag
    /// hatch, split **vertically** here and **horizontally** in
    /// [`DROP_HALF_BOTTOM`]. So the inner and the drop shadow are two states of
    /// one picture rather than two unrelated icons.
    pub const DROP_HALF: &str = "\u{e566}";
    /// The **drop shadow**'s row glyph, from the design (`ph-drop-half-bottom`).
    /// See [`DROP_HALF`], which is its pair.
    pub const DROP_HALF_BOTTOM: &str = "\u{eb40}";
    /// The **Filters** row's glyph, from the design (`ph-sun`).
    ///
    /// Not one of the four adjustment pictures the image card gave up on: those
    /// had to distinguish seven tone controls from each other, where this names a
    /// whole row whose own label is beside it. Brightness leads the four channels
    /// and a sun is what it looks like.
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
    /// **The *all the way* pair.** *Bring to front* draws this and *Send to back*
    /// draws [`ARROW_LINE_DOWN`], against the plain arrows above for the one-step
    /// pair — the family the `// Z-order` line names.
    ///
    /// 🚨 **The app drew neither until §15 D761, and both order ends wore one
    /// picture** (§15 D672, `[S18.3-L3-04]`): `menu.rs` handed `Front` **and**
    /// `Back` `STACK`, as the **first and last** rows of one four-row group.
    /// (⚠️ This said *"one row apart"* for a commit; `Item::ALL` and `restack_rows`
    /// both order them `Front, Forward, Backward, Back`.)
    ///
    /// ⚠️ **Was kept unused, because the tree contained two comments that disagree
    /// and no document was thought to settle it.** The line above this one groups
    /// four glyphs as *"one
    /// step vs all the way"*, which reads as these two having been meant for those
    /// rows; [`STACK`]'s own doc said *"Bring to front / Send to back — a stack
    /// seen from the side, which is the picture the four order rows are about"*,
    /// which endorsed what shipped. `context-menus.md` §4's *Order* table lists
    /// rows and chords and **no icons**, and neither `architecture.md`,
    /// `shortcuts.md` nor §15 assigns a glyph to any of the four.
    ///
    /// 🚨 **Ruled 2026-09-15 and the pair is live: this is *Bring to front* and
    /// [`ARROW_LINE_DOWN`] is *Send to back*** (§15 D761). `Front` and `Back` both
    /// drew [`STACK`], two rows apart in one emitted group. Arrow-to-a-bar against
    /// plain arrow is the family the `// Z-order` comment above already names, and
    /// `ARROW_LINE_LEFT`/`ARROW_LINE_RIGHT` carry the same *against a wall*
    /// reading in the typography panel's block indents.
    ///
    /// ⚠️ **`design/Editor.dc.html` says `ph-stack` / `ph-stack-simple` and §8 makes
    /// it normative for what a row looks like, so this is a recorded deviation.**
    /// The export's answer moves the duplication rather than removing it —
    /// `ph-stack-simple` is *Flatten* there, and the two rows co-occur.
    ///
    /// ⚠️ **The `#[allow(dead_code)]` these carried is gone, and removing it was
    /// not optional**: a stale shield over a now-live constant is the exact
    /// accident D672 was written to fix, and nothing would have reported it.
    pub const ARROW_LINE_UP: &str = "\u{e066}";
    /// The downward half of the pair above, and the same open question.
    ///
    /// ⚠️ **It is *not* the *Send to back* glyph, and two places said it was**
    /// (§15 D672). [`EXPORT`]'s doc named it *"`ARROW_LINE_DOWN`, **Send to back**"*
    /// while describing a shape, and §15 D225 carries the identical parenthetical;
    /// both are corrected. The cost of leaving it is the one the module states two
    /// doc comments below, on [`FOLDER_SIMPLE`]: the next person choosing a glyph
    /// near this family reads that sentence, reaches for this constant, and ships a
    /// second picture for a row that already has one.
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
    /// Dragging a guide back onto its ruler throws it away, and — rasterized by
    /// `cursor.rs` — this is the cursor that says so.
    ///
    /// The plain bin rather than `trash` (U+E4A6), which carries a lid and a
    /// handle: at a cursor's 19pt those details collapse into noise, and the
    /// silhouette is the whole of what has to read.
    pub const TRASH_SIMPLE: &str = "\u{e4a8}";
    pub const CHECK: &str = "\u{e182}"; // a checked row in the View/Snap menus
    // The Transform panel's proportion lock: a whole chain when locked, a broken
    // one when not.
    pub const LINK_SIMPLE: &str = "\u{e2e6}";
    pub const LINK_SIMPLE_BREAK: &str = "\u{e2e8}";
    /// The typography popup's **skip-ink** toggle, beside the underline's line
    /// style (§15 D357).
    ///
    /// A third link glyph, and the only *horizontal* break in the set: two chain
    /// ends facing each other across a gap, which is the picture of what skip-ink
    /// does to a band — read out of the atlas on 2026-08-25 rather than trusted
    /// from the table, against `link-break`, `text-a-underline`, `scissors`,
    /// `arrows-split` and `minus`. Phosphor has nothing that draws a rule stepping
    /// around a descender, so the metaphor is "broken", and the horizontal one is
    /// the one nothing else in the app has claimed — `LINK_SIMPLE_BREAK` above is
    /// diagonal and means the Transform panel's proportion lock, two panels away.
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
    /// **The upright sliders, against the horizontal ones two lines up, and the
    /// pair is deliberate rather than a near-miss.** `SLIDERS_HORIZONTAL` opens the
    /// *rest of one control's* properties — a stroke's dash, cap and join, and the
    /// export row's per-row settings; this opens the app's own. Same picture turned
    /// through a right angle for the same verb at a different scale, which is the
    /// distinction Phosphor draws with the two glyphs and the design uses. What it
    /// must not be is a `GEAR`: nothing in this app wears one, and a cog would be
    /// the only piece of chrome borrowed from a different icon set's idea of
    /// settings.
    pub const SLIDERS: &str = "\u{e432}";
    // Undo/redo in the top bar. The design uses the U-turn arrows rather than
    // the circular ones (`ARROW_COUNTER_CLOCKWISE`), which read as "rotate".
    pub const ARROW_U_UP_LEFT: &str = "\u{e08a}";
    pub const ARROW_U_UP_RIGHT: &str = "\u{e08c}";
    /// The Scale tool, in the rail and — rasterized by `cursor.rs` — as its
    /// cursor, so the button and the pointer are the same drawing.
    pub const RESIZE: &str = "\u{ed6e}";
    /// Select every layer using one Group Colors row's colour. A reticle rather
    /// than a cursor or a checkbox: the button *finds* things.
    pub const SCAN: &str = "\u{ebb6}";
    /// Which sides of a shape a stroke occupies — a box divided.
    pub const SQUARE_SPLIT_HORIZONTAL: &str = "\u{e870}";
    // The four boolean operations, in the order the dropdown lists them. Taken
    // from `assets/fonts/Phosphor.codepoints.txt` rather than recovered by hand,
    // and each one is checked for ink by `every_named_icon_has_a_glyph_behind_it`
    // — a transposed digit here is an invisible menu row.
    pub const UNITE_SQUARE: &str = "\u{e878}";
    pub const SUBTRACT_SQUARE: &str = "\u{ebd4}";
    pub const INTERSECT_SQUARE: &str = "\u{e87a}";
    pub const EXCLUDE_SQUARE: &str = "\u{e880}";
    /// Flatten, at the foot of the same dropdown. **Deliberately not one of the four
    /// set diagrams**: it is not a fifth operation to switch to but a thing that
    /// happens once and discards the operands, and two arrows becoming one line says
    /// that where a fifth square would have claimed kinship with the states above it.
    ///
    /// Read out of the atlas before it was trusted, as `arrows-in-line-vertical` was:
    /// two verticals converging into one stem under an arrowhead. Its ink box is
    /// narrower and taller than the four squares' (16×26 against 24×24 at 32px), which
    /// does **not** shift the label beside it — every Phosphor glyph advances the same
    /// 15.00 at 15px, so `ui::glyph_and_text` starts "Flatten" exactly where it starts
    /// "Union".
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
    // Paragraph alignment. **A separate set from `ALIGN_*` above**, which aligns
    // *layers* to each other: `text-align-left` draws ragged lines of text where
    // `align-left` draws boxes against a rule, and using one glyph for both would
    // make the Align panel and the Type panel look like the same control.
    pub const TEXT_ALIGN_LEFT: &str = "\u{e484}";
    pub const TEXT_ALIGN_CENTER: &str = "\u{e480}";
    pub const TEXT_ALIGN_RIGHT: &str = "\u{e486}";
    pub const TEXT_ALIGN_JUSTIFY: &str = "\u{e482}";
    /// Letter spacing — the `Aa` glyph, which is what tracking is *of*.
    pub const TEXT_AA: &str = "\u{e6ee}";
    /// Line height: the vertical arrow pair, matching `ARROWS_LEFT_RIGHT`'s use
    /// for dot spacing.
    pub const ARROWS_VERTICAL: &str = "\u{eb04}";
    /// The horizontal twin of [`ARROWS_VERTICAL`], which is the line-height glyph
    /// — and nothing draws this one (§15 D672). Letter spacing took [`TEXT_AA`]
    /// instead, on the argument in that constant's doc, so the pair is the unused
    /// half of a symmetry rather than a gap in one.
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
    /// **Arrows rather than a second pair of text glyphs**, so the two kinds of
    /// indent do not look like four states of the same control: the first-line pair
    /// draws lines of text, this pair draws an edge. Each arrow points at the side
    /// whose inset it sets, which is `ARROW_LEFT`/`ARROW_RIGHT`'s rule above rather
    /// than a new one.
    ///
    /// **Read out of the atlas before they were trusted**, and the picture is worth
    /// recording because it can be read the other way: each glyph is a bar against
    /// one edge with the arrow pointing **at** that bar, not away from it. So the bar
    /// is the *margin the number is measured from* — `ARROW_LINE_LEFT` for the start
    /// inset — and not the text being pushed away from it. Swapping the pair to make
    /// the arrows point inward would put each number on the opposite margin.
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
    /// A stack seen from the side. **Drawn by nothing since §15 D761**, which gave
    /// *Bring to front* and *Send to back* the [`ARROW_LINE_UP`]/[`ARROW_LINE_DOWN`]
    /// pair — this constant was on **both** of those rows, which is what the ruling
    /// was about.
    ///
    /// ⚠️ **Kept with its own `allow` and a reason, which is D672's shape for
    /// exactly this case** (`CARET_UP` and `ARROWS_HORIZONTAL` are the others):
    /// it is `design/Editor.dc.html`'s answer for *Bring to front*, and §8 makes
    /// that export normative for what a row looks like — so this is the constant a
    /// reversal of D761 reaches for, one line away, rather than a codepoint
    /// somebody has to recover from `Phosphor.codepoints.txt` again.
    ///
    /// ⚠️ **Its doc said *"which is the picture the four order rows are about"***
    /// and that sentence was one of the two D672 weighed. It is false now; the
    /// history is kept here rather than in a sentence that reads as current.
    ///
    /// 🚨 **Measured while adding this allow, and it corrects D672**: that entry
    /// says *"under `--all-targets` it is live again and shields everything, so the
    /// gate that catches the next dead icon is the plain build"*. Removing this
    /// attribute warns under **both** spellings — `cargo check -p ondin-app` and
    /// `cargo check -p ondin-app --all-targets` — because `icon::ALL` being
    /// `#[cfg(test)]` shields the *test* target while the **bin** target is
    /// compiled without that cfg either way, and the warning comes from the bin.
    /// The gate is better than D672 claims, and the claim is the sort a reader
    /// trims a command on.
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
    /// **`export`, chosen by atlas dump over `download` and `download-simple`, and
    /// the dump is what decided it.** All three are a tray with an arrow; the two
    /// downloads point *into* it and are therefore a vertical stem with a chevron and
    /// a bar at the bottom — which is [`ARROW_LINE_DOWN`] with side walls added.
    /// (⚠️ **This used to gloss that constant as *Send to back***, which was wrong
    /// when it was written — `ARROW_LINE_DOWN` was drawn nowhere at all and the row
    /// drew `STACK`. §15 D672 corrected it here **and in D225**, which no longer
    /// carries the parenthetical; this sentence claimed it *"still carries"* it for
    /// six days and was false the moment it shipped, which is D672's own subject
    /// arriving in D672's own receipt. 🚨 **And the gloss is now *true*** — §15 D761
    /// gave *Send to back* that very constant. The *shape* argument was never
    /// affected either way, which is why the sentence was corrected rather than
    /// removed, and why it can now simply be dropped.) At 15pt those walls are the whole difference. `export` points
    /// **up and out**, so it contrasts by direction rather than by detail, and it is
    /// the glyph Phosphor named for this verb. `download` additionally carries a mark
    /// inside the tray that is noise at icon size.
    pub const EXPORT: &str = "\u{eaf0}";
    /// The Export panel's format dropdown — a picture in a sheet, saying "this row
    /// makes a file" (§7, `design/Editor.dc.html`'s export rows).
    ///
    /// **One glyph for all three formats, not three.** Phosphor carries `file-png`,
    /// `file-jpg` and `file-svg`, and at the 15pt this is drawn at they are one
    /// outline with three letters of illegible micro-text inside it — the word
    /// beside the glyph is already the format, so a per-format glyph would be
    /// decoration that has to be squinted at. The design draws this one on a JPG
    /// row, which is the same call.
    pub const FILE_IMAGE: &str = "\u{ea24}";
    /// *Export as Zip* — the one button that produces an archive rather than
    /// files.
    pub const FILE_ZIP: &str = "\u{e958}";
    /// *JPEG quality* — a dial, for the one export setting that is a trade rather
    /// than a fact (§7).
    pub const GAUGE: &str = "\u{e628}";
    /// *Slashes in names make folders* — the Export menu's switch.
    ///
    /// **Not [`FOLDER_OPEN`]**, which is the top bar's *Open* and the Export
    /// menu's own *Show last export folder* two rows above it: that one is a
    /// folder being **entered**, and this is a folder being **made**.
    ///
    /// ⚠️ **The rule this used to cite says the opposite thing** (§15 D672,
    /// `[S18.3-L3-04]`). It read *"Two rows in one card wearing one glyph is what
    /// `context-menus.md` §8 is about"*; §8's own words are *"name them in the
    /// registry rather than at the call site, **so the same verb cannot be two
    /// pictures in two menus**"* — one verb, one glyph, across the app. The
    /// converse — one glyph, two verbs — is named only in §15 **D225**, and named
    /// there as *"§8's glyph rule failing in the direction nobody checks for"*.
    /// Both readings condemn a `FOLDER_OPEN` here; only one of them is §8's, and
    /// the other is the one this constant is an instance of. **A citation that
    /// resolves is not a citation that agrees.**
    pub const FOLDER_SIMPLE: &str = "\u{e25a}";
    /// *Replace and reset* — the Asset row that swaps the picture and throws the
    /// framing away (§15 D225).
    ///
    /// **A broom, because every ring was taken and they all look alike.** The obvious
    /// picks for "and reset" are the circular arrows, and this app has three of them
    /// already meaning three other things: `ARROW_COUNTER_CLOCKWISE` is *Reset crop*
    /// two rows above this one, `ARROWS_CLOCKWISE` is *reverse* in two places, and
    /// `ARROW_CLOCKWISE` prefixes the Transform panel's rotation field and its
    /// rotate-90° button. (It was the rotate *cursor* too until 2026-08-19, when
    /// that became a drawn arc — for the same reason this comment is about, in
    /// fact: at cursor size a ring says nothing a second ring does not.) A fourth
    /// ring — and
    /// `arrows-counter-clockwise` is the mirror image of the *reverse* one — would be
    /// the "same verb, two pictures" failure §8 of `context-menus.md` names, wearing
    /// its opposite: two verbs, one picture. Sweeping something clean is unmistakable
    /// at 15pt and unmistakably not a rotation.
    pub const BROOM: &str = "\u{ec54}";
    /// Every icon this module names, paired with its Phosphor name.
    ///
    /// **The catalogue has to be enumerable or its test rots.** A wrong Phosphor
    /// codepoint lays out perfectly and renders blank (`Phosphor.ttf`'s `post` table
    /// is **version 3.0**, which carries no glyph names, so these are transcribed by
    /// hand — §15 D754, and this line said *"carries no `post` table"* until it was
    /// measured), and
    /// `every_named_icon_has_a_glyph_behind_it` is what catches one. That test used to
    /// carry its own hand-written list, which reached 28 of 117 names before anybody
    /// noticed — including **none** of the typography glyphs, the ones most recently
    /// added and least verified. Rust cannot reflect over `pub const`s, so the list
    /// cannot be derived; what it can be is *one* list, in the same place as the
    /// constants, with a test that fails when this list and the constants disagree
    /// (`the_catalogue_lists_every_icon_it_declares`).
    ///
    /// The name is the Phosphor one, lower-kebab.
    ///
    /// ⚠️ **Two claims here were true until §15 D551 and are not** (`[A8-L6-04]`),
    /// and they are corrected rather than deleted because the second reverses the
    /// name's whole standing. This said *"a test that fails when the two **counts**
    /// diverge"* — which is what the test did, and a count cannot see a
    /// count-preserving corruption; and it said the name *"is only ever a failure
    /// message"*, which is now false in the load-bearing direction. **The name is
    /// the join key.** `declared_icons` derives the expected name from each
    /// constant's identifier (`FOLDER_SIMPLE` → `"folder-simple"`) and the test
    /// compares name *sets* and pairs glyphs *by* name, so a wrong name in this
    /// list fails membership rather than merely reading oddly in a message.
    /// `arch-scribe` caught both, on the very item the fix is about.
    // 🚨 **`#[cfg(test)]`, not `#[allow(dead_code)]`, and the difference is the
    // whole of §15 D672's mechanism.** `[S18.3-L3-04]`'s own fix sketch says to put
    // the allow on this constant alone — and that **restores the blanket shield**,
    // because an allow-listed item is still a live *root*, so every one of the 143
    // constants it names goes on having a reader. Measured: with the allow here,
    // `cargo check -p ondin-app` reports zero dead constants; with it removed, the
    // leaves that have no other reader appear.
    //
    // Compiling it out of the bin build instead makes the sentence above it true —
    // this really is read only from tests — and is what gives `pub mod icon` a
    // working `dead_code` for the first time.
    //
    // ⚠️ **It works for `cargo check`/`cargo build` and not for `--all-targets`**,
    // which is D302's shape from the other side: under `cfg(test)` this is live
    // again and shields everything. The gate that catches the next dead icon is the
    // plain build, and the test build cannot be it.
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
        ("files", FILES),
        ("squares-four", SQUARES_FOUR),
        ("pencil-simple", PENCIL_SIMPLE),
        ("arrow-u-up-left", ARROW_U_UP_LEFT),
        ("arrow-u-up-right", ARROW_U_UP_RIGHT),
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
    // **`Ctrl` and `+`/`−` zoom the artwork, not the app.** egui reads those chords
    // itself and scales `pixels_per_point`, which in a design tool is the wrong answer
    // twice over: the panels, the rail and the layer tree all grew and shrank, and the
    // canvas zoom — the thing those keys mean everywhere in this class of app, and what
    // `Action::ZoomIn`/`ZoomOut` already do — was fighting it for the same keystroke.
    //
    // Turned off rather than worked around: the app owns the keymap (`input::resolve`),
    // and a chord that two layers both act on is the collision `save_and_cut_are_not_booleans`
    // exists to prevent. Display scaling stays the platform's business, which is where a
    // designer expects it to live.
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

    // **egui's "this rect changed id" warning is off** (§15). It paints a 2px red
    // outline, with no text, for one frame — and in a debug build it fires every time
    // a Fill or Stroke list gets *shorter*, around the top row of the panel.
    //
    // Not a bug being silenced. The paint panels are drawn top-of-the-stack first, so
    // a list of n shows `n-1 .. 0` down the column and each row is salted with the
    // index of the paint it shows (`carried_unit` — the whole reason a hex field
    // survives a live reorder). Delete a paint and the top row's rect keeps its
    // position while the id in it drops by one, and the id that was there is gone
    // from the pass entirely — which is exactly the shape egui's heuristic looks for.
    // It is right about the mechanism and wrong about the conclusion: the row at the
    // top *is* a different paint now, and its state should not carry over.
    //
    // No narrower switch exists — `DebugOptions` is per-context, not per-widget — so
    // the choice is between the false positive on every paint delete and losing the
    // diagnostic. The false positive reads as a rendering fault in the chrome, which
    // is the more expensive of the two. `warn_on_id_clash`, which catches genuine
    // duplicate ids (the trap `OndinApp::panel` documents), stays on.
    //
    // **Guarded, because `Style::debug` does not exist in a release build.** egui
    // puts `DebugOptions` behind `#[cfg(debug_assertions)]` — the overlays it
    // configures are debug-only — so an unguarded line here does not merely have
    // no effect in release, it fails to compile. Nothing in the workspace's gates
    // catches that: `check`, `test` and `clippy` all run the debug cfg, and
    // `cargo build --release -p ondin-app` was the only thing that would say so.
    #[cfg(debug_assertions)]
    {
        style.debug.warn_if_rect_changes_id = false;
    }

    // **No label in this app is a piece of text to select**, and egui's default
    // says otherwise: a selectable label puts an **I-beam** over itself on hover
    // and drags out a highlight. That is right for a document viewer and wrong
    // here — every panel eyebrow (*Appearance*, *Fill*, *Stroke*), every caption
    // and every value label is a piece of *chrome*, and chrome takes the arrow.
    // The headers made it obvious because they are also click targets: the row
    // toggles the card open, so the pointer said "text" over a control that
    // behaves like a button.
    //
    // **Off at the style rather than per label**, because the alternative is a
    // rule 200-odd `ui.label` calls have to remember, and the one that forgets is
    // the one the eye lands on. It costs nothing that is used: the only production
    // call to `Label::selectable` in the workspace already passes `false`
    // (`typography.rs`'s *Show all features* row, which reported this same
    // complaint from its own call site first), and the two labels that act as
    // buttons — that one and `inspector`'s gradient name — keep their explicit
    // `Sense::click()`, because `selectable` and `sense` are separate fields.
    //
    // Text fields are untouched: a `TextEdit` sets its own cursor and should.
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

    /// **A wrong Phosphor codepoint lays out perfectly well and comes back blank.**
    /// `Phosphor.ttf`'s `post` table is version 3.0 and carries no glyph names, so
    /// the constants above are copied out of a recovered ligature table by hand and
    /// there is nothing in the type system to catch a transposed digit — the failure
    /// is an invisible button.
    ///
    /// ⚠️ **Every copy of this sentence in the repository said *"has no `post`
    /// table"* until §15 D754, which lists them, and the table is right there** —
    /// 32 bytes at offset 488604, version `0x00030000`, which is the version that
    /// stores no names. The
    /// consequence is identical and the sentence was not, which matters because a
    /// reader who checks finds the tag in the table directory in one grep and has
    /// every reason to distrust the atlas-dump rule that rests on it. **Say which
    /// version.**
    ///
    /// So every glyph the app names is laid out and read back out of the font atlas,
    /// and has to have ink in it. This is the same technique `cursor.rs` uses to
    /// rasterize its cursors, pointed at the catalogue instead.
    ///
    /// **It reads `icon::ALL` rather than a list of its own, and that is the whole
    /// point.** The private list this used to carry had reached 28 of 117 names —
    /// among them not one of the typography glyphs, which are the newest and the least
    /// verified. A test whose coverage is a second copy of the thing it tests will
    /// always drift behind it; `the_catalogue_lists_every_icon_it_declares` is what
    /// keeps the one remaining copy honest.
    ///
    /// **What this still cannot say is whether a glyph is the one that was wanted.**
    /// A transposed digit landing on a neighbouring *valid* icon passes here, and only
    /// looking at the picture can catch it — the ASCII atlas dump in `CLAUDE.md`, which
    /// is a throwaway probe rather than an assertion because "is this the right drawing"
    /// has no machine answer. The typography set was read that way on 2026-08-03 and all
    /// of it was right: the four `TEXT_ALIGN_*` are distinct and each flush the way its
    /// name says, `TEXT_INDENT`/`TEXT_OUTDENT` point right and left and are not swapped,
    /// `TEXT_AA` is `Aa`, `PARAGRAPH` is a pilcrow, `LIST_NUMBERS` is numbered, the
    /// three `ALIGN_*` put their bar top/middle/bottom, and `ARROW_UP` really points up
    /// (which is what lets it carry the decoration offset's sign). Nothing to fix, and
    /// worth not re-deriving.
    ///
    /// **`SLIDERS` and `SLIDERS_HORIZONTAL` were read the same way on 2026-08-24**, since
    /// they are two codepoints apart and mean two different things in this app (§15 D330).
    /// U+E432 is three *upright* rails with their knobs at three different heights; U+E434
    /// is two *horizontal* rails. They are transposes of each other, they are distinct, and
    /// the constants are the right way round.
    ///
    /// **The Effects panel's four row glyphs were read the same way on 2026-08-24**,
    /// and the reading corrected a comment written above it. `DROP_HALF_BOTTOM` is a
    /// droplet with its bottom half filled by Phosphor's zigzag hatch and `DROP_HALF`
    /// is the same droplet split down the middle — a matched pair, unlike
    /// `FLIP_HORIZONTAL`/`FLIP_VERTICAL`, which is worth knowing because the drop and
    /// the inner shadow sit two rows apart in one card. `CIRCLE_HALF` hatches its half
    /// the same way, so the blur row belongs to the same set. `SUN` is a sun. What the
    /// dump corrected was the guess that `DROP_HALF` fills the *left* half: it fills
    /// the right.
    ///
    /// **`LINK_SIMPLE_HORIZONTAL_BREAK` was read the same way on 2026-08-25**, before
    /// it was named, because the skip-ink toggle it draws had no obvious picture and
    /// the choice was between six. It is two chain ends facing each other across a gap
    /// on one horizontal axis — genuinely horizontal, where `link-break` (U+E2E4) runs
    /// diagonally like the `LINK_SIMPLE_BREAK` already in use. That is the reading the
    /// constant's own comment records; this is the note that the dump happened.
    #[test]
    fn every_named_icon_has_a_glyph_behind_it() {
        let ctx = egui::Context::default();
        install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let icons: &[(&str, &str)] = icon::ALL;

        // The glyph's coverage out of the atlas: its size and its alpha, which for
        // premultiplied white text *is* the drawing.
        //
        // **Counting ink is not enough, and this test proved nothing until it stopped
        // doing that.** A codepoint the font does not carry is not blank — egui falls
        // back and lays out a `.notdef` box, and a 16pt box is far more than the eight
        // texels the old threshold asked for. So the test passed for `\u{efff}`, and
        // passed for every transposed digit that happened to land outside the font.
        // The fingerprint below is what makes a missing glyph detectable: it is the
        // *same picture* every time, so any icon that matches it is not an icon.
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

        // A codepoint deep in the Private Use Area that Phosphor does not define, so
        // whatever comes back is the fallback's answer rather than an icon.
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

    /// **The two block-indent arrows carry their bar on the edge their number
    /// measures from, and are not the same glyph.**
    ///
    /// The pair was picked by reading the atlas (see `icon::ARROW_LINE_LEFT`) and this
    /// is the half of that reading worth keeping: which side the bar sits on *is* the
    /// meaning of the icon, and swapping the two constants would put each number on
    /// the opposite margin with nothing to show it. `every_named_icon_has_a_glyph_behind_it`
    /// cannot catch that — it would also pass on two names pointing at one codepoint,
    /// which the inequality below is for.
    ///
    /// **Not asserted as a pixel-exact mirror**, which was the first attempt: the two
    /// outlines are mirrored but their antialiasing is not (27/167/12 against
    /// 35/180/16 on the same row), so exact equality fails on a pair that is perfectly
    /// correct. Column ink is the property the claim is actually about.
    #[test]
    fn the_block_indent_arrows_bar_the_edge_they_measure_from() {
        let ctx = egui::Context::default();
        install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        // `(width, per-column ink)`, read out of the font atlas — see
        // `every_named_icon_has_a_glyph_behind_it` for why the atlas is the only place
        // a glyph's *drawing* can be got at.
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
    /// Rust cannot enumerate `pub const`s, so the catalogue's only possible witness
    /// is the source text. `include_str!` is resolved at compile time against this
    /// same file, so there is nothing to keep in sync and no path to get wrong; the
    /// module is bounded by its own `pub mod icon {` and the first line that closes
    /// it at that indentation.
    ///
    /// ⚠️ **The name is *derived* from the identifier, which is an assertion in
    /// disguise and is why this reads the escape rather than only counting lines.**
    /// A constant's identifier is its Phosphor name in `SCREAMING_SNAKE`, so
    /// `FOLDER_SIMPLE` must be listed as `"folder-simple"` — measured true for all
    /// 143 entries when this was written. That convention is what lets the test
    /// below check the *pairing* as well as the membership: a `("selection",
    /// DIAMOND)` slip puts the wrong glyph under a name that exists, which no count
    /// and no set of names can see.
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
            // `ALL` is declared in here too, and is the catalogue rather than an
            // icon in it.
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

    /// **`icon::ALL` has to list every constant the module declares**, or the atlas
    /// test above silently stops covering the newest icons — which is exactly what
    /// happened to the private list it replaced (28 of 117, and none of the typography
    /// glyphs).
    ///
    /// A mismatch means an icon was added without being listed. Add it to `ALL` — do
    /// not relax this test, because the whole value of the one above is that its
    /// coverage cannot drift.
    ///
    /// ⚠️ **This asserted a *count* until §15 D551, and a count is narrower than the
    /// rule in this test's own name** (`[A8-L6-04]`, which the review named *the
    /// sixth gate mechanism*: an enforcement test whose predicate is narrower than
    /// the sentence naming it). `ALL.len()` against `body.matches("pub const ")`
    /// cannot see a **count-preserving** corruption. Measured, not argued: replacing
    /// `("selection", SELECTION)` with `("diamond", DIAMOND)` — one entry, the length
    /// unchanged — left all four tests in this module green, with `SELECTION`
    /// thereafter covered by nothing and `diamond` checked twice. That is verbatim
    /// the failure the paragraph above says this test exists to prevent.
    ///
    /// ⚠️ **And D143's recorded flip does not reach it.** That entry states the rule
    /// as membership — *"`icon::ALL` lists every constant, beside the constants"* —
    /// and flip-checks it by *dropping* an entry, which changes the count and is
    /// therefore the one case the count answered. This is CLAUDE.md's second vacuity
    /// shape, *"it asserted a consequence the wrong version also produces"*, in the
    /// codebase's own flagship anti-drift test. The flips below replace D143's.
    ///
    /// ⚠️ **Flip-checks, all four run, each with its failure site recorded because
    /// two of the four predictions were wrong.**
    ///
    /// 1. `("selection", SELECTION)` → `("diamond", DIAMOND)`, the review's own
    ///    corruption. **Predicted the membership assertion; it fails at the
    ///    *duplicate* one** — *"`diamond` is listed twice in ALL"* — because that
    ///    check runs while `ALL` is being read into the map and the missing-name
    ///    check runs after it. The prediction was about which *fact* matters and the
    ///    code is about which *line* runs first; they are not the same question, and
    ///    here the order is fine either way, since a name listed twice means some
    ///    other constant is listed not at all and the message says so.
    /// 2. One entry deleted — D143's own case, still covered. Fails the membership
    ///    assertion, and **names the icon**: *"Declared but not listed:
    ///    [\"selection\"]"*, where the count version said *"declares 143 and ALL
    ///    lists 142"* and left the reader to find which.
    /// 3. `("selection", DIAMOND)` with `("diamond", DIAMOND)` left in place.
    ///    **Passes membership and fails the pairing assertion**, which is the whole
    ///    reason the glyph is read out of the escape rather than only the name: the
    ///    count agrees, the name set agrees, and one icon is drawn under another's
    ///    name.
    /// 4. `SELECTION`'s codepoint set to `DIAMOND`'s. Fails the distinctness
    ///    assertion at *"142 of 143 entries are distinct"* — and ⚠️ **the other
    ///    three tests in this module stay green**, including
    ///    `every_named_icon_has_a_glyph_behind_it`, which is the measurement that
    ///    says distinctness was enforced for four entries out of 143 before this.
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
