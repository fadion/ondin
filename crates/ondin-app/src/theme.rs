//! Visual theme for the app chrome: the "Nocturne" design realized in egui, in a
//! dark and a light palette.
//!
//! Every panel ground is the near-neutral text colour mixed toward black (dark)
//! or white (light), with a single steel-blue accent. The mixed values are frozen
//! here as `Color32` values, since egui has no `color-mix`.
//!
//! This module is where the design tokens become egui `Style`/`Visuals`/fonts;
//! [`apply`] switches the palette and restyles egui together.

use eframe::egui;
use std::sync::Arc;

/// Palette. The chrome's colours change with the theme, so each is read through
/// a function of its own name ([`color::bg`], [`text::dim`]). The canvas marks —
/// selection, key, component, measure, point — are drawn over artwork rather
/// than over the chrome, and stay constants.
///
/// No `#[allow(dead_code)]` here: `dead_code` reports a colour that loses its
/// last caller.
pub mod color {
    use eframe::egui::Color32;

    /// Declares [`Palette`]: one field per chrome colour, and a function of the
    /// same name returning the current theme's value.
    macro_rules! palette {
        ($($(#[$doc:meta])* $name:ident,)*) => {
            /// One theme's chrome colours.
            struct Palette {
                $($name: Color32,)*
            }
            $(
                $(#[$doc])*
                pub fn $name() -> Color32 {
                    palette().$name
                }
            )*
        };
    }

    palette! {
        /// `--color-text` — the one near-neutral foreground everything derives from.
        text,

        /// The ground of a page with no colour of its own (`Page::ground` is
        /// `None`). The dark value is `ondin_core::DEFAULT_CANVAS_BACKGROUND`,
        /// what everything headless draws such a page on.
        canvas,

        // Grounds (text mixed toward the far end).
        /// Main container, canvas, inspector pane.
        bg,
        /// Top bar.
        topbar,
        /// Layers pane.
        panel,
        /// Tool rail and inspector cards.
        card,
        /// Field/input ground inside a card (text 5% over the card).
        field,
        /// Hairline field border — text 8% over **[`field`], not over [`card`]**.
        ///
        /// A translucent CSS border composites over the element's own background
        /// (`background-clip: border-box`), so the 8% resolves against the field,
        /// not the card; resolved over the card it is too dim to register.
        /// [`card_border`] is the same 8% over the card, for boxes filled with
        /// [`card`].
        field_border,
        /// Hairline round a box filled with [`card`] — the dashboard's project and
        /// file cards, and the *New file* slot's dashed outline.
        ///
        /// Text 8% over [`card`]. Kept apart from [`field_border`]: against the
        /// card that would read as a lit edge, which is what a *hovered* card
        /// already means ([`accent_700`]).
        card_border,
        /// The ground every hovered control paints (§15 D484): egui's stock widgets
        /// (`install_style_into`), `ui::icon_button_padded`, `ui::button_face` and
        /// `inspector::template_cell` all use it, and they must agree or the hover
        /// splits into two greys.
        hover,
        /// A ground a step above [`card`] — the insert picker's hover card, which
        /// floats over the picker's own card.
        raised,
        /// The document tab strip's ground, a step below [`topbar`]; a tab behind
        /// keeps [`tab_behind`], a step above the strip, and hover lifts it to
        /// [`tab_lifted`]. The front tab takes [`topbar`].
        tab_strip,
        /// A tab behind the front one.
        tab_behind,
        /// A tab behind the front one, under the pointer.
        tab_lifted,

        /// The accent (the ramp's 500). The ramp's steps count away from the
        /// ground: 900 is the subtle accent ground, 100 the ink on it.
        accent,
        accent_100,
        accent_200,
        accent_300,
        accent_400,
        accent_600,
        accent_700,
        accent_800,
        accent_900,
        /// A mark on a *solid* accent fill — a switch's knob, a slider's — which
        /// stays near-white in either theme, where [`accent_100`] (the ink on a
        /// pale accent tint) turns dark in the light one.
        on_accent,
        /// The word on a primary button's solid [`accent`] fill
        /// ([`super::super::ui::FieldButton::Primary`]): near-black in dark, where
        /// [`on_accent`]'s white would not reach a reading contrast on the lighter
        /// accent, and white in light.
        accent_ink,

        /// A layer the app cannot draw as the document describes it — today, one
        /// whose picture is missing (§15 D179).
        ///
        /// Amber rather than red: the usual cause is a moved file, a thing to fix
        /// rather than a mistake. It tints the row's existing kind glyph, which costs
        /// no width.
        warn,
        /// The star on a starred document's card.
        ///
        /// Its own colour rather than [`warn`], though near-identical: the two mean
        /// opposite things (*you marked this* / *this cannot be drawn*) and must be
        /// free to be re-tuned apart.
        star,
        /// A row that destroys something — today, *Delete* in a context menu.
        ///
        /// The app's one red, so the eye lands on the row work can be lost with.
        /// Distinct from [`warn`]: amber grades toward "cannot be drawn", red means
        /// "this click removes it".
        danger,
        /// The save dot once the file matches the document (§15 D962) — the app's
        /// one green, at [`danger`]'s OKLCH lightness and chroma, so neither state
        /// is louder than the other.
        saved,
        /// Ink of a green badge, [`saved`]'s twin of [`accent_100`] — the
        /// dashboard's *New* tag.
        saved_100,
        /// Ground of a green badge, [`saved`]'s twin of [`accent_900`].
        saved_900,
        /// A failed status message's ink: [`danger`] moved toward the text, so a
        /// line of small type stays readable.
        error,
        /// The steps a *filled* destructive button needs, mirroring
        /// [`accent_900`]/[`accent_700`]/[`accent_200`] — ground, border and ink for
        /// [`super::super::ui::FieldButton::Danger`] — and [`accent_800`] for its
        /// hovered ground. Derived from [`danger`] by the accent ramp's ratios, so
        /// the red button is the blue one's twin in weight.
        danger_900,
        /// [`danger_900`]'s hovered ground.
        danger_800,
        /// [`danger_900`]'s border.
        danger_700,
        /// [`danger_900`]'s ink.
        danger_200,
    }

    /// The dark palette. Grounds are `color-mix(in srgb, var(--color-text) N%,
    /// #000)` with text `#e9e9ed`: 10% the main ground, 13% the top bar, 14% the
    /// layers pane, 16% the cards, 17% the raised ground.
    const DARK: Palette = Palette {
        text: Color32::from_rgb(233, 233, 237),
        canvas: Color32::from_rgb(23, 23, 24),
        bg: Color32::from_rgb(23, 23, 24),
        topbar: Color32::from_rgb(30, 30, 31),
        panel: Color32::from_rgb(33, 33, 33),
        card: Color32::from_rgb(37, 37, 38),
        field: Color32::from_rgb(47, 47, 48),
        field_border: Color32::from_rgb(62, 62, 63),
        card_border: Color32::from_rgb(53, 53, 54),
        // One step from `card_border`, and not an alias: that is a hairline and this
        // a fill, so they never read as one. It cannot rise: text over it would fall
        // under the 10:1 floor (`every_ink_reads_on_every_ground_in_both_themes`).
        hover: Color32::from_rgb(52, 52, 54),
        raised: Color32::from_rgb(40, 40, 40),
        tab_strip: Color32::from_rgb(14, 14, 15),
        tab_behind: Color32::from_rgb(20, 20, 21),
        tab_lifted: Color32::from_rgb(25, 25, 26),
        accent: Color32::from_rgb(109, 140, 217),
        accent_100: Color32::from_rgb(226, 233, 248),
        accent_200: Color32::from_rgb(196, 211, 241),
        accent_300: Color32::from_rgb(166, 189, 234),
        accent_400: Color32::from_rgb(139, 165, 224),
        accent_600: Color32::from_rgb(87, 115, 189),
        accent_700: Color32::from_rgb(68, 92, 154),
        accent_800: Color32::from_rgb(47, 63, 102),
        accent_900: Color32::from_rgb(33, 44, 70),
        on_accent: Color32::from_rgb(226, 233, 248),
        accent_ink: Color32::from_rgb(16, 19, 27),
        warn: Color32::from_rgb(0xe0, 0xa0, 0x4a),
        star: Color32::from_rgb(0xe0, 0xa9, 0x3c),
        danger: Color32::from_rgb(0xe0, 0x73, 0x6b),
        saved: Color32::from_rgb(0x45, 0xb0, 0x70),
        saved_100: Color32::from_rgb(0xb4, 0xe8, 0xc8),
        saved_900: Color32::from_rgb(0x12, 0x30, 0x20),
        error: Color32::from_rgb(226, 138, 138),
        danger_900: Color32::from_rgb(69, 36, 33),
        danger_800: Color32::from_rgb(101, 52, 48),
        danger_700: Color32::from_rgb(148, 76, 71),
        danger_200: Color32::from_rgb(243, 201, 198),
    };

    /// The light palette, the dark one's mirror: grounds are the text mixed
    /// toward white, lighter as they rise (the main ground lowest, a card
    /// near-white), and a field, a hover or a recessed tab sits *below* its
    /// ground rather than above. The accent and danger ramps keep their roles —
    /// 900 the pale ground, 100 the dark ink on it — and the hues are deepened
    /// to hold contrast on white.
    const LIGHT: Palette = Palette {
        text: Color32::from_rgb(30, 30, 33),
        canvas: Color32::from_rgb(238, 238, 240),
        bg: Color32::from_rgb(236, 236, 238),
        topbar: Color32::from_rgb(245, 245, 246),
        panel: Color32::from_rgb(247, 247, 248),
        card: Color32::from_rgb(252, 252, 252),
        field: Color32::from_rgb(241, 241, 242),
        field_border: Color32::from_rgb(224, 224, 225),
        card_border: Color32::from_rgb(234, 234, 235),
        hover: Color32::from_rgb(232, 232, 234),
        raised: Color32::from_rgb(255, 255, 255),
        tab_strip: Color32::from_rgb(226, 226, 228),
        // Aliases on purpose, in light only: a tab behind is `hover`'s grey, one
        // step off its strip as a hover is off its ground, and a lifted tab meets
        // the page in `canvas`'s. Neither pair is ever drawn side by side.
        tab_behind: Color32::from_rgb(232, 232, 234),
        tab_lifted: Color32::from_rgb(238, 238, 240),
        accent: Color32::from_rgb(74, 108, 196),
        accent_100: Color32::from_rgb(30, 48, 100),
        accent_200: Color32::from_rgb(44, 68, 138),
        accent_300: Color32::from_rgb(58, 88, 172),
        accent_400: Color32::from_rgb(66, 98, 186),
        accent_600: Color32::from_rgb(112, 140, 214),
        accent_700: Color32::from_rgb(160, 182, 234),
        accent_800: Color32::from_rgb(205, 217, 245),
        accent_900: Color32::from_rgb(228, 235, 251),
        on_accent: Color32::from_rgb(255, 255, 255),
        accent_ink: Color32::from_rgb(255, 255, 255),
        warn: Color32::from_rgb(0x96, 0x5f, 0x0a),
        star: Color32::from_rgb(0xaa, 0x73, 0x00),
        danger: Color32::from_rgb(0xc8, 0x3c, 0x34),
        saved: Color32::from_rgb(0x19, 0x7d, 0x46),
        saved_100: Color32::from_rgb(0x14, 0x5a, 0x32),
        saved_900: Color32::from_rgb(0xde, 0xf5, 0xe7),
        error: Color32::from_rgb(190, 50, 50),
        danger_900: Color32::from_rgb(251, 231, 229),
        danger_800: Color32::from_rgb(246, 214, 211),
        danger_700: Color32::from_rgb(226, 150, 144),
        danger_200: Color32::from_rgb(130, 35, 30),
    };

    /// The chrome's two palettes.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Theme {
        #[default]
        Dark,
        Light,
    }

    thread_local! {
        /// Per thread: the UI paints on one, and a test that switches theme
        /// cannot reach a test running beside it.
        static THEME: std::cell::Cell<Theme> = const { std::cell::Cell::new(Theme::Dark) };
    }

    /// The theme the chrome is painting in.
    pub fn theme() -> Theme {
        THEME.get()
    }

    /// Switch the palette alone; [`super::apply`] also restyles egui.
    pub(super) fn set_theme(theme: Theme) {
        THEME.set(theme);
    }

    /// The current theme's palette.
    fn palette() -> &'static Palette {
        match theme() {
            Theme::Dark => &DARK,
            Theme::Light => &LIGHT,
        }
    }

    /// How much stronger the light theme draws a translucent text ink: dark ink
    /// over a light ground reads fainter than light ink over a dark one at the
    /// same alpha. Linear, so a tier halved by `Ui::disable` still lands on
    /// [`super::text::disabled`].
    const LIGHT_INK_GAIN: f32 = 1.2;
    /// The most a gained light ink reaches: short of opaque, so
    /// [`super::text::strong`] stays a step under [`text`] rather than equal to it.
    const LIGHT_INK_CAP: f32 = 230.0;

    /// Text at a given opacity over the ground. Used for the muted label tiers
    /// (38% / 45% / 62% / 88% in the dark theme).
    pub fn text_a(alpha: u8) -> Color32 {
        let alpha = match theme() {
            Theme::Dark => alpha,
            Theme::Light => (alpha as f32 * LIGHT_INK_GAIN)
                .round()
                .min(LIGHT_INK_CAP.max(alpha as f32)) as u8,
        };
        let [r, g, b, _] = text().to_array();
        let at = |c: u8| (c as u32 * alpha as u32 / 255) as u8;
        Color32::from_rgba_premultiplied(at(r), at(g), at(b), alpha)
    }

    /// [`text_a`] composited onto `ground`, opaque: the hairline of a card that
    /// floats over other chrome. egui paints a frame's stroke beside its fill,
    /// not on it, so a translucent one lets what lies under the card show through.
    pub fn text_on(ground: Color32, alpha: u8) -> Color32 {
        ground.blend(text_a(alpha))
    }

    /// The accent at an alpha, for a wash over the chrome.
    pub fn accent_a(alpha: u8) -> Color32 {
        let [r, g, b, _] = accent().to_array();
        Color32::from_rgba_unmultiplied(r, g, b, alpha)
    }

    /// `dark` or `light` by the current theme, for a colour too local to earn
    /// a palette entry.
    pub fn by_theme(dark: Color32, light: Color32) -> Color32 {
        match theme() {
            Theme::Dark => dark,
            Theme::Light => light,
        }
    }

    /// `ink` as the dark theme paints it, for ink on a fixed black wash — present
    /// mode's counter, a library card's chips. The wash stays dark in either
    /// theme, so the ink on it must stay light.
    pub fn on_scrim(ink: impl FnOnce() -> Color32) -> Color32 {
        let was = THEME.replace(Theme::Dark);
        let c = ink();
        THEME.set(was);
        c
    }

    /// The chrome's selection ink: [`SELECT`] in the dark theme, a deepened sky
    /// blue in the light one, where the canvas hue on a pale row reads under 2:1.
    /// For text and glyphs on a chrome ground; the canvas keeps [`SELECT`].
    pub fn select_ink() -> Color32 {
        by_theme(SELECT, Color32::from_rgb(24, 98, 168))
    }

    /// Hairline divider / faint border (text at ~9%).
    pub fn divider() -> Color32 {
        text_a(23)
    }

    /// The size preview's two grounds, fixed whatever the chrome's theme: white,
    /// and the dark theme's canvas, so an icon is judged on the grounds it ships on.
    pub fn light_ground() -> Color32 {
        Color32::WHITE
    }

    /// See [`light_ground`].
    pub fn dark_ground() -> Color32 {
        DARK.canvas
    }

    /// The editor's save dot, red while the document differs from its file
    /// (§15 D962). [`danger`]'s value under its own name: a dirty document is
    /// not a click that destroys something, and the two must be free to part.
    pub fn unsaved() -> Color32 {
        danger()
    }

    /// The selection colour on the canvas — a light sky blue, deliberately
    /// *not* the chrome accent: the panels and much artwork are blue, and an
    /// outline in that family disappears into both.
    pub const SELECT: Color32 = Color32::from_rgb(0x63, 0xb3, 0xf5);
    /// The same hue at ~55%, for the parts of the chrome that must not compete
    /// with the outline itself: pen control handles, the entered-group hint.
    pub const SELECT_DIM: Color32 = select_a(0x8c);
    /// The layer-row highlight in the layers tree, at panel strength.
    pub const SELECT_ROW: Color32 = select_a(43);
    /// The ground inside a selection marquee, the box's and the lasso's loop
    /// alike: faint, so the artwork it sweeps still reads.
    pub const MARQUEE_FILL: Color32 = select_a(28);
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
    /// Ink on [`BADGE`]. The badge ground is light, so the dark theme's near-white
    /// text tier would be unreadable on it; it stays the same in either theme.
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

    /// The ground of an *unselected* point marker in the node tool.
    ///
    /// Opaque rather than hollow like a resize handle (§15 D14): the anchor *is*
    /// the target and sits on ink of any colour. Near-white because it has to
    /// read on the canvas, not in a panel.
    pub const POINT: Color32 = Color32::from_rgb(0xf2, 0xf4, 0xf8);
}

/// Muted text tiers, named for what each is used for.
pub mod text {
    use super::color::text_a;
    use eframe::egui::Color32;
    /// ~88%.
    pub fn strong() -> Color32 {
        text_a(224)
    }
    /// ~62%.
    pub fn muted() -> Color32 {
        text_a(158)
    }
    /// ~45%.
    pub fn dim() -> Color32 {
        text_a(115)
    }
    /// ~38%.
    pub fn faint() -> Color32 {
        text_a(97)
    }
    /// **A control that cannot be used**, as distinct from one that is merely
    /// quiet: ~31%, below [`faint`], which stays the tier for placeholders, hints
    /// and secondary labels.
    ///
    /// 79 is matched, not chosen: `Ui::disable` fades by `Visuals::disabled_alpha`
    /// (0.5), so a [`muted`] glyph disabled that way paints at 79, and colour-based
    /// disabling (`icon_button`'s `enabled: false`, `FieldButton::Disabled`) must
    /// land on the same ink. It also stays well apart from the "off" tiers
    /// (`HIDDEN_OPACITY` over muted is 95, a hidden or locked layer row is
    /// [`off`]), so dimmed-because-off and dimmed-because-unavailable differ.
    pub fn disabled() -> Color32 {
        text_a(79)
    }
    /// **A row that is off** — a hidden or locked layer, a style or variable that
    /// does not apply: ~40%, a step above [`faint`] so the row still reads as its
    /// list's own, and apart from [`disabled`].
    pub fn off() -> Color32 {
        text_a(102)
    }
}

/// The app's own icon glyphs, by name, in `assets/fonts/OndinIcons.ttf`. Render
/// with [`icon_text`] or [`icon_font`] so they use the icon family.
///
/// # Adding an icon
///
/// Draw it into **`assets/icons/glyphs.json`** on the 16-unit grid (a 1-unit
/// stroke centred on half units, so a straight line covers one pixel row at
/// 100%), under the name this module derives from the constant (`FOLDER_SIMPLE` →
/// `"folder-simple"`) and a free Private Use Area codepoint; rebuild the font with
/// `assets/icons/build_font.py`; add the constant and its `ALL` row. The tests
/// check that the JSON, the constants and the font agree, the font by a stamp of
/// the JSON it was built from.
///
/// The `#[allow(dead_code)]` sits on the individual unused constants, each with
/// its reason, not on the module, so the next glyph to lose its last call site
/// warns.
pub mod icon {
    /// The operand others are measured against: a boolean's base, the key layer,
    /// the subtract base's badge in the layers tree.
    pub const DIAMOND: &str = "\u{e1ec}";
    pub const CARET_DOWN: &str = "\u{e136}";
    pub const CARET_RIGHT: &str = "\u{e13a}";
    /// The unused third of the caret trio: `CARET_DOWN` and `CARET_RIGHT` are the
    /// pair every disclosure draws, and nothing points a caret up yet.
    #[allow(dead_code)]
    pub const CARET_UP: &str = "\u{e13c}";
    pub const CARET_LEFT: &str = "\u{e138}";
    /// The layers header's *Expand all*: two carets pointing apart, the tree
    /// opening out, so it reads as an act on every row rather than one row's
    /// open state. Not the vertical gap arrows, which mean the row gap.
    pub const CARETS_OUT: &str = "\u{f041}";
    /// *Collapse all*: [`CARETS_OUT`]'s carets each flipped, pointing together.
    pub const CARETS_IN: &str = "\u{f042}";
    // The contrast check's glyphs: a passing count, a layer it could not measure,
    // a badge waiting on a remeasure, and a non-text finding.
    pub const CHECK_CIRCLE: &str = "\u{e184}";
    pub const PROHIBIT: &str = "\u{e3de}";
    pub const CIRCLE_NOTCH: &str = "\u{eb44}";
    pub const SHAPES: &str = "\u{ec5e}";
    pub const MAGNIFYING_GLASS: &str = "\u{e30c}";
    pub const MAGNIFYING_GLASS_PLUS: &str = "\u{e310}";
    pub const FRAME: &str = "\u{e626}";
    pub const SELECTION: &str = "\u{e69a}";
    /// Contrast, wherever it is checked: View's *Check contrast*, the contrast
    /// bar, *Fix contrast*.
    pub const CIRCLE_HALF: &str = "\u{e18c}";
    /// A mask — *Use as mask*, the identity row's toggle, its mode, its badge in
    /// the layers tree: a circle with a horizon across it.
    pub const MASK: &str = "\u{f038}";
    /// *Even-odd fill*: a ring, its overlap left empty.
    pub const EVEN_ODD: &str = "\u{f039}";
    // The identity row's actions, beside the boolean dropdown (§9.4).
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
    /// View's *Outline*: a box drawn as a hairline with its four handles.
    pub const OUTLINE_VIEW: &str = "\u{f040}";
    pub const TRIANGLE: &str = "\u{e4b0}"; // the polygon tool — it starts as one
    pub const STAR: &str = "\u{e46a}";
    pub const PLAY: &str = "\u{e3d0}"; // present mode
    pub const ASTERISK: &str = "\u{e0aa}"; // a star's point count
    pub const ANGLE: &str = "\u{e7bc}"; // a stroke's mitre limit
    /// A star's inner ratio: a star with its inner radius drawn to a dot.
    pub const STAR_RATIO: &str = "\u{f03b}";
    /// Skew, field and steps: a box leaning over, its top corners marked.
    pub const SKEW: &str = "\u{f03a}";
    pub const EYE: &str = "\u{e220}";
    pub const EYE_SLASH: &str = "\u{e224}";
    pub const LOCK_SIMPLE: &str = "\u{e308}";
    /// A locked layer: [`LOCK_SIMPLE`] with its body filled, since fill marks the
    /// state.
    pub const LOCK_SIMPLE_FILL: &str = "\u{f001}";
    pub const LOCK_SIMPLE_OPEN: &str = "\u{e30a}"; // an unlocked layer
    pub const CURSOR: &str = "\u{e1dc}";
    pub const HAND: &str = "\u{e298}";
    /// The Line tool and layer, and a line's own *Length* field: the line, measured.
    pub const LINE_SEGMENT: &str = "\u{e6d2}";
    /// A stroke's paint, wherever a *Stroke* step is named: a box drawn only as
    /// its edge, the hollow twin of [`SQUARE_FILL`] and the partner of
    /// [`PAINT_BUCKET`]'s *Fill*.
    pub const STROKE: &str = "\u{f045}";
    pub const PEN_NIB: &str = "\u{e3ac}";
    pub const BEZIER_CURVE: &str = "\u{eb00}"; // the node tool: a curve with its handles
    /// *Make corner*, on the path bar and in the menu: a point where two straight
    /// segments meet at an angle. [`NODE_SMOOTH`] is its pair.
    pub const NODE_CORNER: &str = "\u{f026}";
    /// *Make smooth*: a point on a curve, its handle line drawn through it.
    pub const NODE_SMOOTH: &str = "\u{f027}";
    pub const EYEDROPPER: &str = "\u{e568}";
    /// Unused: a comment tool is a non-goal. Kept because dropping it means
    /// rebuilding the font (`assets/icons/build_font.py`).
    #[allow(dead_code)]
    pub const CHAT: &str = "\u{e15c}";
    pub const ALIGN_LEFT: &str = "\u{e50e}";
    pub const ALIGN_CENTER_HORIZONTAL: &str = "\u{e50a}";
    pub const ALIGN_RIGHT: &str = "\u{e510}";
    pub const ALIGN_TOP: &str = "\u{e512}";
    pub const ALIGN_CENTER_VERTICAL: &str = "\u{e50c}";
    pub const ALIGN_BOTTOM: &str = "\u{e506}";
    /// *Rotate 90° clockwise*: an open arc with its arrowhead turning right.
    /// [`ROTATE_LEFT`] is the same drawing mirrored.
    pub const ROTATE_RIGHT: &str = "\u{f028}";
    /// A rotation set freely — the rotation fields, a gradient's angle, the
    /// *Rotate* steps: a 300° arc around a pivot dot, with [`ROTATE_RIGHT`]'s
    /// head. The dot keeps it apart from reset's mirrored arc on the same field.
    pub const ROTATION: &str = "\u{f044}";
    /// *Rotate 90° counter-clockwise*: [`ROTATE_RIGHT`] mirrored.
    pub const ROTATE_LEFT: &str = "\u{f029}";
    /// An ellipse's arc Start: a circle with its start radius and a dot where it
    /// meets the rim.
    pub const ARC_START: &str = "\u{f02b}";
    /// An ellipse's Sweep.
    pub const CHART_PIE_SLICE: &str = "\u{e15a}";
    /// An ellipse's Ratio, the hole.
    pub const CIRCLE_DASHED: &str = "\u{e602}";
    pub const CORNERS_OUT: &str = "\u{e1d0}"; // corner radius
    /// Zoom to fit, wherever it is offered: corner brackets around a magnifier.
    pub const ZOOM_FIT: &str = "\u{f022}";
    pub const CORNERS_IN: &str = "\u{e1ce}"; // fit a frame to its content
    pub const NUMBER_SQUARE_ONE: &str = "\u{e36c}"; // actual size (1:1 / 100%)
    pub const PLUS: &str = "\u{e3d4}";
    pub const DOTS_THREE: &str = "\u{e1fe}";
    /// A row's own overflow menu: a Library file's, a kit's in *Kits used…*.
    ///
    /// Vertical where a head's overflow ([`DOTS_THREE`]) is horizontal: a row's
    /// own menu reads as belonging to the row.
    pub const DOTS_THREE_VERTICAL: &str = "\u{e208}";
    /// The dashboard's *Recent*, and the picker's recent-colours row.
    pub const CLOCK_COUNTER_CLOCKWISE: &str = "\u{e1a0}";
    /// A page: its identity, *Go to* / *Move to page*, a version's page groups,
    /// *Add page*, and *Draft*, which marks a page.
    pub const FILE: &str = "\u{e230}";
    /// The dashboard's *All files*, and the placeholder on a card with no
    /// thumbnail yet.
    pub const FILES: &str = "\u{e710}";
    /// A document, apart from a page ([`FILE`]): a sheet with lines of text. A
    /// Library search's file result, a file picked for import.
    pub const DOCUMENT: &str = "\u{f01e}";
    /// A description, wherever one is written — a variable's, a main's: a card
    /// with lines of writing on it.
    pub const NOTE: &str = "\u{f023}";
    /// The Library: its grid-view toggle (against `ROWS` for the list) and Ctrl+K's
    /// *Go to the library*.
    pub const SQUARES_FOUR: &str = "\u{e464}";
    /// The Pencil tool, its cursor and *Draw path*; the dashboard's *Edit
    /// project*; a History step with no glyph of its own.
    pub const PENCIL_SIMPLE: &str = "\u{e3b4}";
    /// A slice's glyph.
    pub const KNIFE: &str = "\u{e636}";
    /// The Select button's glyph while the marquee is a lasso.
    pub const LASSO: &str = "\u{edc6}";
    pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{e038}"; // reset, restore, reopen
    /// A folder opened or revealed: the library folder, *Show in library*, *Show
    /// last export folder*, a project, History's *Opened*.
    pub const FOLDER_OPEN: &str = "\u{e256}";
    pub const FLOPPY_DISK: &str = "\u{e248}"; // save, and a saved export preset
    pub const MAGNIFYING_GLASS_MINUS: &str = "\u{e30e}";
    pub const X: &str = "\u{e4f6}"; // close / remove a paint row
    // The import card's: clearing its field, and the line under it.
    pub const X_CIRCLE: &str = "\u{e4f8}";
    pub const INFO: &str = "\u{e2ce}";
    /// Mirror in x (and a gradient's stops).
    ///
    /// Two triangles about a dotted axis, one of them filled; [`FLIP_VERTICAL`] is
    /// the same drawing turned, so the two are a matched pair.
    pub const FLIP_HORIZONTAL: &str = "\u{ed6a}";
    /// Mirror in y. See [`FLIP_HORIZONTAL`].
    pub const FLIP_VERTICAL: &str = "\u{ed6c}";
    pub const ARROWS_CLOCKWISE: &str = "\u{e094}"; // reverse: a path's direction, a gradient's stops
    /// The dashboard's sort control, which is one button carrying whichever order
    /// is in force.
    ///
    /// Not [`ARROWS_CLOCKWISE`], which means *reverse*: the button steps to the
    /// next key, and arrows passing each other read as "ordered by".
    pub const ARROWS_DOWN_UP: &str = "\u{e098}";
    /// The **inner shadow**'s row glyph. It is the same droplet as
    /// [`DROP_HALF_BOTTOM`], split vertically rather than horizontally, so the
    /// inner and drop shadow read as two states of one picture.
    pub const DROP_HALF: &str = "\u{e566}";
    /// The **drop shadow**'s row glyph.
    /// See [`DROP_HALF`], which is its pair.
    pub const DROP_HALF_BOTTOM: &str = "\u{eb40}";
    /// The **Filters** row's glyph.
    pub const SUN: &str = "\u{e472}";
    /// The **layer blur**'s row glyph: a disc of soft dots.
    pub const LAYER_BLUR: &str = "\u{f037}";
    pub const SQUARE_HALF: &str = "\u{e462}"; // stroke alignment
    // Per-corner radius: a box drawn in dots with the one corner it sets rounded
    // and solid, one drawing turned into each corner.
    pub const CORNER_TOP_LEFT: &str = "\u{f004}";
    pub const CORNER_TOP_RIGHT: &str = "\u{f005}";
    pub const CORNER_BOTTOM_LEFT: &str = "\u{f006}";
    pub const CORNER_BOTTOM_RIGHT: &str = "\u{f007}";
    /// Radii set per corner: the toggle that opens the four corner fields, and
    /// the point editor's per-corner button. Four tiles, each with its outer
    /// corner rounded, so it does not repeat the radius field's brackets.
    pub const CORNERS_SEPARATE: &str = "\u{f009}";
    /// Corner smoothing: a squircle, the shape smoothing turns a rounded box into.
    pub const SQUIRCLE: &str = "\u{f008}";
    /// The **background blur**'s row glyph: a pane, frosted.
    pub const SQUARE_DOTS: &str = "\u{f00a}";
    /// The **Private mode** pill's glyph: a hat and glasses, a browser's private
    /// window.
    pub const INCOGNITO: &str = "\u{f00b}";
    // Per-side stroke: a box drawn in dots with the one side it sets solid, one
    // drawing turned onto each side.
    pub const SIDE_TOP: &str = "\u{f00d}";
    pub const SIDE_RIGHT: &str = "\u{f00e}";
    pub const SIDE_BOTTOM: &str = "\u{f00f}";
    pub const SIDE_LEFT: &str = "\u{f010}";
    /// Anything set per side (a stroke's sides, a container's padding): four
    /// sides that do not meet, [`CORNERS_SEPARATE`]'s twin.
    pub const SIDES_SEPARATE: &str = "\u{f011}";
    /// *Pad out to a square*: a picture centred in a square drawn in dots.
    pub const PAD_SQUARE: &str = "\u{f012}";
    /// Transparency, in both its places: a layer's opacity (the field, its badge
    /// and its steps) and an export's transparent background. The checkerboard
    /// that stands for no colour.
    pub const CHECKERBOARD: &str = "\u{f013}";
    /// A colour chip in a dropdown's face, where a painter cannot reach: the
    /// outline [`SQUARE`]'s solid twin.
    pub const SQUARE_FILL: &str = "\u{f014}";
    /// A variable collection, wherever one is named: the picker, its menu, a
    /// search's and a list's collection headers, *Move to ▸*.
    pub const DATABASE: &str = "\u{f015}";
    /// A variable's scope — the fields it may bind: *Scope…* and the door to it.
    pub const TARGET: &str = "\u{f016}";
    /// An interaction state — a set's property tied to the browser's hover,
    /// press, focus and disabled — wherever one is marked: a hand, its index
    /// finger raised.
    pub const HAND_POINTING: &str = "\u{f017}";
    /// A breakpoint — a mode a frame picks from its width — wherever one is
    /// named: a monitor with a phone before it.
    pub const DEVICES: &str = "\u{f018}";
    /// A variable mode's *Inherit*: an arrow turning up to the level above.
    pub const ARROW_ELBOW_LEFT_UP: &str = "\u{e04c}";
    // Z-order: one step vs all the way.
    pub const ARROW_UP: &str = "\u{e08e}";
    pub const ARROW_DOWN: &str = "\u{e03e}";
    /// *Bring to front*; [`ARROW_LINE_DOWN`] is *Send to back* (§15 D761). The
    /// arrow-to-a-bar pair is the *all the way* family against the plain
    /// `ARROW_UP`/`ARROW_DOWN` one-step pair, and `ARROW_LINE_LEFT`/`ARROW_LINE_RIGHT`
    /// carry the same *against a wall* reading. Not [`STACK`] / [`STACK_SIMPLE`]:
    /// `STACK` is *Arrange ▸*, which holds these rows, and `STACK_SIMPLE` is *Flatten*.
    pub const ARROW_LINE_UP: &str = "\u{e066}";
    /// *Send to back*; see [`ARROW_LINE_UP`].
    pub const ARROW_LINE_DOWN: &str = "\u{e05c}";
    /// A gap along an axis: the column and row gap fields, their badges and steps,
    /// the canvas padding drag, and a dash's spacing. *Auto width* / *Auto height*
    /// and the flex *Grow* / *Shrink* steps share it, since they size a box along
    /// that axis.
    pub const ARROWS_OUT_LINE_HORIZONTAL: &str = "\u{e534}";
    pub const ARROWS_OUT_LINE_VERTICAL: &str = "\u{e536}";
    /// *Distribute horizontally*, on the Align card and in the menu: two boxes
    /// spaced evenly between two walls. [`DISTRIBUTE_V`] is the same drawing turned.
    pub const DISTRIBUTE_H: &str = "\u{f030}";
    /// *Distribute vertically*: [`DISTRIBUTE_H`] turned.
    pub const DISTRIBUTE_V: &str = "\u{f031}";
    /// A flex layout, added or switched to: three boxes in a row.
    pub const FLEX_LAYOUT: &str = "\u{f032}";
    /// A layout removed: three boxes left where they fall, the Layout card's
    /// *None* cell drawn in outline.
    pub const NO_LAYOUT: &str = "\u{f043}";
    /// *Tidy up*: a row of three boxes over a rule with an arrowhead at each end.
    pub const TIDY_UP: &str = "\u{f02c}";
    pub const COLUMNS: &str = "\u{e546}";
    pub const ROWS: &str = "\u{e5a2}";
    pub const RULER: &str = "\u{e6b8}"; // a guide, named for where it comes from
    /// The library's trash: its sidebar row and its empty state. The guide
    /// cursor's bin is not this glyph but path data in `cursor.rs`.
    pub const TRASH_SIMPLE: &str = "\u{e4a8}";
    pub const CHECK: &str = "\u{e182}"; // a checked row in the View/Snap menus
    /// A binding, wherever one is made: *Bind to variable…*, *Bind property*, a
    /// linked tokens file. [`LINK_BREAK`] undoes it.
    pub const LINK_SIMPLE: &str = "\u{e2e6}";
    /// The Transform panel's proportion lock, locked or not: a box with two
    /// opposite corners held.
    pub const ASPECT_RATIO: &str = "\u{f034}";
    /// A web address — a main's documentation link, a Figma link: a globe.
    pub const GLOBE: &str = "\u{f035}";
    /// The typography popup's **skip-ink** toggle, beside the underline's line
    /// style (§15 D357).
    ///
    /// The only *horizontal* break in the link set: two chain ends facing each
    /// other across a gap, the picture of what skip-ink does to a band.
    pub const LINK_SIMPLE_HORIZONTAL_BREAK: &str = "\u{e2ec}";
    pub const MAGNET: &str = "\u{e680}"; // the Snap menu
    /// The transform origin, in two states. `CROSSHAIR_SIMPLE` is the read-only
    /// marker, the Transform panel's toggle, *Move pivot* and *Reset origin*; `CROSSHAIR`, the same reticle with
    /// a filled centre, is the marker while it can be dragged. One glyph in the
    /// panel and on the canvas, so the button names the thing it arms rather than
    /// describing it, and the fill says whether it is live.
    pub const CROSSHAIR_SIMPLE: &str = "\u{e1d8}";
    pub const CROSSHAIR: &str = "\u{e1d6}";
    /// Absolute position: a layer pinned out of its parent's layout, *Pin* /
    /// *Unpin* and the Position steps. A box held to its frame's edges by two
    /// pins.
    pub const PIN: &str = "\u{f024}";
    /// A drag grip: on a reorderable row (Fill's and Stroke's paint rows) and on a
    /// before-and-after split's divider (`ui::split_handle`).
    pub const DOTS_SIX_VERTICAL: &str = "\u{eae2}";
    /// The rest of a stroke's properties, behind one button: dash, cap and join.
    pub const SLIDERS_HORIZONTAL: &str = "\u{e434}";
    /// The Settings modal, from the top bar (`crate::settings`).
    ///
    /// The upright sliders, deliberately paired with `SLIDERS_HORIZONTAL`, which
    /// opens the rest of one control's properties where this opens the app's own.
    /// Not a `GEAR`: nothing else in the app wears one.
    pub const SLIDERS: &str = "\u{e432}";
    // Undo/redo, wherever offered: the U-turn arrows rather than
    // the circular ones (`ARROW_COUNTER_CLOCKWISE`), which read as "rotate".
    pub const ARROW_U_UP_LEFT: &str = "\u{e08a}";
    pub const ARROW_U_UP_RIGHT: &str = "\u{e08c}";
    /// Wrapping: `flex-wrap: wrap`, and a text's *Wrap* step. A line running into a
    /// wall and turning back under itself. [`WRAP_REVERSE`] is the same drawing
    /// flipped.
    pub const WRAP: &str = "\u{f01a}";
    /// `flex-wrap: wrap-reverse`: [`WRAP`] flipped, the line turning back over.
    pub const WRAP_REVERSE: &str = "\u{f019}";
    /// The Scale tool, in the rail and — rasterized by `cursor.rs` — as its
    /// cursor, so the button and the pointer are the same drawing.
    pub const RESIZE: &str = "\u{ed6e}";
    /// Select what matches: *Select same…*, *Select matching layers*, every layer
    /// using one Group Colors row's colour. A reticle rather than a cursor or a
    /// checkbox: the button *finds* things.
    pub const SCAN: &str = "\u{ebb6}";
    // The four boolean operations, in the order the dropdown lists them.
    pub const UNITE_SQUARE: &str = "\u{e878}";
    pub const SUBTRACT_SQUARE: &str = "\u{ebd4}";
    pub const INTERSECT_SQUARE: &str = "\u{e87a}";
    /// A blend-mode step's History glyph.
    pub const INTERSECT_THREE: &str = "\u{ecc4}";
    pub const EXCLUDE_SQUARE: &str = "\u{e880}";
    /// Colour review's merge: the target row the others fold into, and the *Merge
    /// colours* step.
    pub const ARROWS_MERGE: &str = "\u{ed3e}";
    /// Unused: a split's grip wears [`DOTS_SIX_VERTICAL`], and a gap is
    /// [`ARROWS_OUT_LINE_HORIZONTAL`]. Kept because dropping it means rebuilding
    /// the font.
    #[allow(dead_code)]
    pub const ARROWS_LEFT_RIGHT: &str = "\u{e0a0}";
    /// A box around several things: a multi-selection's identity, Compare's
    /// Overlay, a text's *Fixed size*, and fitting the dash pattern to the
    /// shape's corners.
    pub const BOUNDING_BOX: &str = "\u{e6ce}";
    /// A custom dash pattern, typed as a list.
    pub const LIST_DASHES: &str = "\u{e2f4}";
    /// A dash's length: a span between two ticks, a dot past each end.
    pub const DASH_LENGTH: &str = "\u{f03d}";
    /// Where the dash pattern starts along the path: a row of dashes over an
    /// arrow sliding it along.
    pub const DASH_OFFSET: &str = "\u{f03e}";
    /// A stroke's cap: a stroke's end, rounded, against a dotted line where the
    /// path stops.
    pub const STROKE_CAP: &str = "\u{f03c}";
    // A step one way along a row: *Move left* / *Move right*, *Move earlier* /
    // *Move later*, a flex direction, batch rename's order.
    pub const ARROW_LEFT: &str = "\u{e058}";
    pub const ARROW_RIGHT: &str = "\u{e06c}";
    // --- typography ------------------------------------------------------
    // Paragraph alignment: a separate set from `ALIGN_*`, which aligns layers.
    pub const TEXT_ALIGN_LEFT: &str = "\u{e484}";
    pub const TEXT_ALIGN_CENTER: &str = "\u{e480}";
    pub const TEXT_ALIGN_RIGHT: &str = "\u{e486}";
    pub const TEXT_ALIGN_JUSTIFY: &str = "\u{e482}";
    /// A font family — the `Aa` glyph: the family face, *Replace font*, a missing
    /// font, the default font. `Aa` is also the common picture of letter case, so it
    /// marks the *Case* step, Find's match-case mark and, with nothing closer, the
    /// *OpenType* step.
    pub const TEXT_AA: &str = "\u{e6ee}";
    /// A font size, wherever it is set: the size field and its badge, *Larger* /
    /// *Smaller text*. A small `A` beside a large one.
    pub const TEXT_SIZE: &str = "\u{f033}";
    /// Line height: the vertical arrow pair. [`ARROWS_HORIZONTAL`] is its twin.
    pub const ARROWS_VERTICAL: &str = "\u{eb04}";
    /// Letter spacing, the horizontal twin of [`ARROWS_VERTICAL`] (line height).
    pub const ARROWS_HORIZONTAL: &str = "\u{eb06}";
    /// The **B** toggle and its chord, against [`STROKE_WEIGHT`]'s weight picker.
    pub const TEXT_BOLD: &str = "\u{e5be}";
    /// A weight, wherever it is set or bound: a stroke's and a font's. Three
    /// rules, each heavier than the one above.
    pub const STROKE_WEIGHT: &str = "\u{f025}";
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
    ///
    /// `ARROW_LINE_RIGHT` also carries [`ARROW_LINE_UP`]'s *all the way* reading
    /// for flex *No wrap*: a line run into the wall, against [`WRAP`]'s turning
    /// back.
    pub const ARROW_LINE_LEFT: &str = "\u{e062}";
    pub const ARROW_LINE_RIGHT: &str = "\u{e064}";
    /// Paragraph spacing — the pilcrow, which is the thing being spaced.
    pub const PARAGRAPH: &str = "\u{e960}";
    /// The language a run is shaped for.
    pub const TRANSLATE: &str = "\u{e4a2}";
    /// A line limit — a count, not a size.
    pub const LIST_NUMBERS: &str = "\u{e2f6}";
    /// Two overlapping right angles: cutting to a box. Box trim (the box cropped
    /// to the type), *Clip content*, and the crop tool on the rail.
    pub const CROP: &str = "\u{e1d4}";
    /// A swap, one thing put in another's place: *Replace text* and its step, an
    /// instance swap property, *Use suggestion*, and the Text direction step.
    pub const SWAP: &str = "\u{e83c}";
    /// A swap property's filter on the Properties card (§15 D1000).
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
    /// The outline hexagon §15 D981 gives components — an instance's mark.
    /// [`HEXAGON_FILL`] is a main's.
    pub const HEXAGON: &str = "\u{e2ae}";
    /// A main component's mark, and the *Create component* and *Duplicate as
    /// component* rows': [`HEXAGON`] filled, since fill marks the state.
    pub const HEXAGON_FILL: &str = "\u{f000}";
    /// A layer local to an instance, not in its main: [`HEXAGON`] with a plus
    /// beside it.
    pub const INSTANCE_LOCAL: &str = "\u{f036}";
    /// A variant set, a component set's mains taken as one: [`HEXAGON_FILL`]
    /// inside corner brackets.
    pub const VARIANT_SET: &str = "\u{f021}";
    /// A **kit** (§5.3e, §15 D1011): the top bar's *Kit* tag, the document menu's
    /// kit rows, and the insert and swap pickers' kit headings.
    pub const PACKAGE: &str = "\u{e390}";
    /// A document open in a tab: the library card's *Open* chip.
    pub const BROWSERS: &str = "\u{e0f6}";
    /// A pinned tab, in the place of its ×, and the tab menu's *Pin tab*.
    pub const PUSH_PIN: &str = "\u{e3e2}";
    /// The tab menu's *Unpin tab*.
    pub const PUSH_PIN_SLASH: &str = "\u{e3e4}";
    /// The styles and variables browser's button (§5.3f).
    pub const SWATCHES: &str = "\u{e5b8}";
    /// A style card's *Create style…* (§15 D1063).
    pub const PLUS_SQUARE: &str = "\u{ed4a}";
    /// Colours as a set: the browser's `+` menu's *Colour variable* and *Number
    /// variable*, Group colors, the picker's row of this document's colours, and
    /// Tools' *Colour review…*.
    pub const PALETTE: &str = "\u{e6c8}";
    pub const HASH: &str = "\u{e2a2}";
    /// A boolean variable: a switch, its knob on.
    pub const TOGGLE: &str = "\u{f01f}";
    /// A column added to a table: the browser's *New mode*, the chart data
    /// window's *Add column*.
    pub const COLUMNS_PLUS_RIGHT: &str = "\u{e542}";
    /// A row added to a table: the chart data window's *Add row*.
    pub const ROWS_PLUS_BOTTOM: &str = "\u{e59e}";
    /// A chart's key column, its categories (a scatter's X): the data window's
    /// header mark and *Use as category*.
    pub const KEY: &str = "\u{e2d6}";
    /// A variable mode, wherever a frame picks one: two sheets, one behind the
    /// other.
    pub const MODE: &str = "\u{f02d}";
    /// *Make default*: [`MODE`] with its front sheet filled, the mode every unset
    /// frame reads.
    pub const MODE_DEFAULT: &str = "\u{f046}";
    /// *Publish* (§5.3e).
    pub const UPLOAD_SIMPLE: &str = "\u{e4c0}";
    /// An update waiting to be taken: a kit's (§5.3e) — waiting, *Accept kit
    /// updates* and the *Update kit* step — and the app's *Restart to update*.
    pub const ARROW_CIRCLE_DOWN: &str = "\u{e028}";
    /// Every detach, row and step — *Detach instance*, a variable or a style
    /// detached, *Detach from path*, *Make local*, *Unlink tokens file*: "no
    /// longer from" what it was tied to.
    pub const LINK_BREAK: &str = "\u{e2e4}";
    /// *Go to main component*, everywhere it is offered — the menu row, the
    /// Component card's main link and its *Go to main* button, *Go to main in*
    /// a kit (§15 D981).
    pub const ARROW_UP_RIGHT: &str = "\u{e092}";
    /// Open elsewhere: a main's documentation link in the browser, from the main's
    /// field and from an instance's card alike, and *Open kit*.
    pub const ARROW_SQUARE_OUT: &str = "\u{e5de}";
    /// Unused: a documentation link is [`GLOBE`] and opening it
    /// [`ARROW_SQUARE_OUT`]. Kept because dropping it means rebuilding the font.
    #[allow(dead_code)]
    pub const BOOK_OPEN: &str = "\u{e0e6}";
    /// Comparing with a version: two states, each arrow carrying one into the other.
    pub const GIT_DIFF: &str = "\u{f00c}";
    /// Select a known set of layers: *Select all*, *Select all instances*, every
    /// *Select N layers*, a variant clash's *Select it*.
    pub const SELECTION_ALL: &str = "\u{e746}";
    /// A variant clash — two variants on one combination (§15 D982): a
    /// warning glyph, in neutral ink, no new hue.
    pub const WARNING: &str = "\u{e4e0}";
    /// A variable made or set — *Extract into variable…*, *New variable*, a
    /// variable's value — and code: *Copy as CSS*, the code exports.
    pub const BRACKETS_CURLY: &str = "\u{e860}";
    /// A layer a component property binds, its badge in the layers tree: the
    /// instance's hexagon with a dot inside and a line out to the property.
    pub const PROPERTY_BOUND: &str = "\u{f03f}";
    /// A component set's root element, the tag it is written out as: angle
    /// brackets and a slash.
    pub const CODE: &str = "\u{f01b}";
    /// A stack seen from the side: the context menu's *Arrange ▸*.
    pub const STACK: &str = "\u{e466}";
    /// *Flatten*, in the context menu and at the foot of the boolean dropdown —
    /// the same stack pressed into one sheet.
    pub const STACK_SIMPLE: &str = "\u{e468}";
    /// *Divide*, beside *Flatten*: the two overlapping squares of the boolean
    /// family cut into their three faces, each outlined apart.
    pub const DIVIDE: &str = "\u{f048}";
    /// The shape builder tool: a small circle and a large one overlapping on the
    /// rail's rising diagonal, the face they share filled.
    pub const SHAPE_BUILDER: &str = "\u{f047}";
    /// *Transform again*: a bent arrow with a second head behind its first, the
    /// step taken once more.
    pub const TRANSFORM_AGAIN: &str = "\u{f049}";
    /// *Repeat around origin…*: eight ticks turned about a pivot dot.
    pub const REPEAT_AROUND: &str = "\u{f04a}";
    /// *Blend shapes…*: a small circle growing into a square through a rounded
    /// step between them. Not a gradient: *Blend* already means blend mode.
    pub const BLEND_SHAPES: &str = "\u{f04b}";
    /// The Spiral tool: two turns of a spiral wound out from the middle.
    pub const SPIRAL: &str = "\u{f04c}";
    /// The Arc tool: a quarter circle between two end dots, the line segment's
    /// drawing bent. Not `CIRCLE_NOTCH`, which is the loading spinner.
    pub const ARC: &str = "\u{f04d}";
    /// *Warp…* and its undo step: an arched envelope with its mesh lines.
    pub const WARP: &str = "\u{f04e}";
    /// The warp bar's *Free distort*: a quadrilateral pulled out of square.
    pub const WARP_FREE: &str = "\u{f04f}";
    /// The warp bar's *Arc*: a band bent into an arch.
    pub const WARP_ARC: &str = "\u{f050}";
    /// The warp bar's *Flag*: a band with one wave through it.
    pub const WARP_FLAG: &str = "\u{f051}";
    /// The warp bar's *Bulge*: a box whose top and bottom swell outward.
    pub const WARP_BULGE: &str = "\u{f052}";
    /// *Trace image…* and its undo step: a curve with its two anchors inside a
    /// picture's corner marks.
    pub const TRACE: &str = "\u{f053}";
    /// *Split at point*: a path broken at a point, its two new ends pulled apart.
    /// Not [`SCISSORS`], which means *Cut*.
    pub const SPLIT_POINT: &str = "\u{f054}";
    /// The Knife tool and its step: a disc cut across, its halves slid apart along
    /// the cut. Not [`KNIFE`], which means *Slice*.
    pub const KNIFE_CUT: &str = "\u{f055}";
    /// *Pixel-fit*: an arrow pushing a square's corner onto the grid. Not
    /// [`GRID_FOUR`], which means the View menu's grid.
    pub const PIXEL_FIT: &str = "\u{f056}";
    /// The top bar's Tools menu: a toolbox, its handle up and its lid's band
    /// latched. Not [`MAGIC_WAND`], which means clean-up.
    pub const TOOLBOX: &str = "\u{f057}";
    /// Drawing new values: *Fill with content*'s *Shuffle* and a row's *Draw
    /// another*. A die showing three.
    pub const DICE: &str = "\u{f058}";
    /// *Fill with content…* and its undo step: lines of text with a small die
    /// beside the last. Not [`MAGIC_WAND`], which means clean-up.
    pub const FILL_CONTENT: &str = "\u{f059}";
    /// The assets browser — the rail's button, *Assets…*, *Insert icon*: two
    /// pictures stacked. Not [`IMAGE`], which is *Place image…*.
    pub const ASSETS: &str = "\u{f05a}";
    /// A service the network cannot reach: the signal's arcs struck through.
    pub const WIFI_SLASH: &str = "\u{f05b}";
    /// A credit — the inspector's *Credit* and *Credits…*: two quotation marks.
    pub const QUOTES: &str = "\u{f05c}";
    /// *Push to main*, an instance's differences written into its main: an arrow
    /// rising into a hexagon outline. Not [`ARROW_UP_RIGHT`] (go to the main) nor
    /// [`UPLOAD_SIMPLE`] (publish).
    pub const PUSH_TO_MAIN: &str = "\u{f05d}";
    /// A chart — the rail's Chart slot, a chart's layers row, the palette: three
    /// ascending columns on a baseline. [`CHART_COLUMN`]'s columns are of mixed
    /// height, so the tool and the kind read apart.
    pub const CHART: &str = "\u{f05e}";
    /// The Column chart kind: three columns of mixed height on a baseline.
    pub const CHART_COLUMN: &str = "\u{f05f}";
    /// The Bar chart kind: three bars off a vertical baseline.
    pub const CHART_BAR: &str = "\u{f060}";
    /// The Line chart kind: a polyline over two axes.
    pub const CHART_LINE: &str = "\u{f061}";
    /// The Area chart kind: the line closed down to the baseline.
    pub const CHART_AREA: &str = "\u{f062}";
    /// The Pie chart kind: a ring with a gap. Not [`CHART_PIE_SLICE`], which
    /// means an arc's sweep.
    pub const CHART_DONUT: &str = "\u{f063}";
    /// The Scatter chart kind: dots over two axes.
    pub const CHART_SCATTER: &str = "\u{f064}";
    /// The Sankey chart kind: one node bar whose two flows part to two bars.
    pub const CHART_SANKEY: &str = "\u{f065}";
    /// A chart's data: a table with a header row — the data window and *Edit
    /// data…*.
    pub const TABLE: &str = "\u{f066}";
    /// The Align card's visual-centre toggle, its palette row and its undo step:
    /// a weight on a stand, a triangle balanced over a post on its base.
    pub const VISUAL_CENTRE: &str = "\u{f067}";
    /// The size preview, its View row, palette row and card head: three squares
    /// nested in one corner.
    pub const SIZE_PREVIEW: &str = "\u{f068}";
    /// *Delete*, in [`super::color::danger()`]. The lidded bin rather than
    /// [`TRASH_SIMPLE`], which the library's Trash already spends.
    pub const TRASH: &str = "\u{e4a6}";
    /// *Show grid*, the pixel grid. A layout grid is [`COLUMNS`] / [`ROWS`] and a
    /// CSS grid [`GRID_LAYOUT`].
    pub const GRID_FOUR: &str = "\u{e296}";
    /// A container laid out as a CSS grid (`display: grid`): a box split into one
    /// tall cell and two short ones.
    pub const GRID_LAYOUT: &str = "\u{f02e}";
    /// An image fill's Tile scale: four tiles, the first one filled.
    pub const IMAGE_TILE: &str = "\u{f02f}";
    /// *Pixel preview*: a square whose one corner is stepped into pixels.
    pub const PIXEL_PREVIEW: &str = "\u{f020}";
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
    /// Every import, row and step — *Import SVG*, *Import PDF*, *Import tokens…*,
    /// the Library's *Import files*: an arrow down into a tray, [`EXPORT`]'s
    /// opposite.
    pub const IMPORT: &str = "\u{f01d}";
    /// The Export panel's format dropdown — a picture in a sheet, saying "this row
    /// makes a file" (§7).
    ///
    /// One glyph for all three formats: the word beside it already names the
    /// format, and a format written into the glyph is illegible at 15pt.
    pub const FILE_IMAGE: &str = "\u{ea24}";
    /// *Copy as SVG*: a sheet with a curve on it, [`FILE_IMAGE`]'s vector twin.
    pub const FILE_SVG: &str = "\u{f01c}";
    /// *Export as Zip* — the one button that produces an archive rather than
    /// files.
    pub const FILE_ZIP: &str = "\u{e958}";
    /// *JPEG quality* — a dial, for the one export setting that is a trade rather
    /// than a fact (§7).
    pub const GAUGE: &str = "\u{e628}";
    /// The resource meter's CPU readout.
    pub const CPU: &str = "\u{e610}";
    /// The resource meter's memory readout.
    pub const MEMORY: &str = "\u{e9c4}";
    /// *Slashes in names make folders* — the Export menu's switch.
    ///
    /// Not [`FOLDER_OPEN`], which is the same menu's *Show last export folder*:
    /// that folder is being entered, this one is being made. One glyph for two
    /// verbs is the failure §15 D225 names.
    pub const FOLDER_SIMPLE: &str = "\u{e25a}";
    /// *Re-export on save* — the Export menu's switch: a tray with a turning arrow
    /// over it. Not `FLOPPY_DISK`, which is the same menu's *Save as a preset*.
    pub const EXPORT_SYNC: &str = "\u{f02a}";
    /// *Replace and reset* — the Asset row that swaps the picture and throws the
    /// framing away (§15 D225).
    ///
    /// A broom, because every ring was taken: `ARROW_COUNTER_CLOCKWISE` is *Reset
    /// crop*, `ARROWS_CLOCKWISE` is *reverse*, `ROTATE_RIGHT` is rotation. A
    /// fourth ring would be two verbs with one picture.
    pub const BROOM: &str = "\u{ec54}";
    /// *Join*, on the path bar and in the context menu — two ends plugged together.
    pub const PLUGS_CONNECTED: &str = "\u{eb5a}";
    /// The path bar's *Simplify*.
    pub const MAGIC_WAND: &str = "\u{e6b6}";
    /// The path bar's *Offset* — the outline pushed out on every side. Not
    /// [`ZOOM_FIT`].
    pub const ARROWS_OUT_CARDINAL: &str = "\u{e0a4}";
    /// A History row's *Resize*.
    pub const ARROWS_OUT: &str = "\u{e0a2}";
    /// A History row's *Fill*.
    pub const PAINT_BUCKET: &str = "\u{e392}";
    /// *Rename*, wherever a name is edited — a layer, a page, a variable, a style,
    /// a version, batch rename — and its step: a text cursor.
    pub const CURSOR_TEXT: &str = "\u{e7d8}";
    /// An agent acting: a History row's mark for a step made over MCP, and the
    /// assistant everywhere it shows.
    pub const SPARKLE: &str = "\u{e6a2}";
    /// *Stop* a running turn: the assistant's send button while it runs.
    pub const STOP: &str = "\u{e46c}";
    /// *Regenerate*: ask the last question again.
    pub const ARROW_CLOCKWISE: &str = "\u{e036}";
    /// Every icon this module names, paired with its name in
    /// `assets/icons/glyphs.json` (lower-kebab).
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
        ("caret-left", CARET_LEFT),
        ("carets-out", CARETS_OUT),
        ("carets-in", CARETS_IN),
        ("check-circle", CHECK_CIRCLE),
        ("prohibit", PROHIBIT),
        ("circle-notch", CIRCLE_NOTCH),
        ("shapes", SHAPES),
        ("magnifying-glass", MAGNIFYING_GLASS),
        ("magnifying-glass-plus", MAGNIFYING_GLASS_PLUS),
        ("frame", FRAME),
        ("selection", SELECTION),
        ("circle-half", CIRCLE_HALF),
        ("mask", MASK),
        ("even-odd", EVEN_ODD),
        ("selection-plus", SELECTION_PLUS),
        ("selection-slash", SELECTION_SLASH),
        ("polygon", POLYGON),
        ("list", LIST),
        ("text-t", TEXT_T),
        ("image", IMAGE),
        ("circle", CIRCLE),
        ("square", SQUARE),
        ("outline-view", OUTLINE_VIEW),
        ("triangle", TRIANGLE),
        ("star", STAR),
        ("play", PLAY),
        ("asterisk", ASTERISK),
        ("angle", ANGLE),
        ("star-ratio", STAR_RATIO),
        ("skew", SKEW),
        ("eye", EYE),
        ("eye-slash", EYE_SLASH),
        ("lock-simple", LOCK_SIMPLE),
        ("lock-simple-fill", LOCK_SIMPLE_FILL),
        ("lock-simple-open", LOCK_SIMPLE_OPEN),
        ("cursor", CURSOR),
        ("hand", HAND),
        ("line-segment", LINE_SEGMENT),
        ("stroke", STROKE),
        ("pen-nib", PEN_NIB),
        ("bezier-curve", BEZIER_CURVE),
        ("node-corner", NODE_CORNER),
        ("node-smooth", NODE_SMOOTH),
        ("eyedropper", EYEDROPPER),
        ("chat", CHAT),
        ("align-left", ALIGN_LEFT),
        ("align-center-horizontal", ALIGN_CENTER_HORIZONTAL),
        ("align-right", ALIGN_RIGHT),
        ("align-top", ALIGN_TOP),
        ("align-center-vertical", ALIGN_CENTER_VERTICAL),
        ("align-bottom", ALIGN_BOTTOM),
        ("rotate-right", ROTATE_RIGHT),
        ("rotate-left", ROTATE_LEFT),
        ("rotation", ROTATION),
        ("arc-start", ARC_START),
        ("chart-pie-slice", CHART_PIE_SLICE),
        ("circle-dashed", CIRCLE_DASHED),
        ("corners-out", CORNERS_OUT),
        ("zoom-fit", ZOOM_FIT),
        ("corners-in", CORNERS_IN),
        ("number-square-one", NUMBER_SQUARE_ONE),
        ("plus", PLUS),
        ("dots-three", DOTS_THREE),
        ("arrow-counter-clockwise", ARROW_COUNTER_CLOCKWISE),
        ("folder-open", FOLDER_OPEN),
        ("floppy-disk", FLOPPY_DISK),
        ("magnifying-glass-minus", MAGNIFYING_GLASS_MINUS),
        ("x", X),
        ("x-circle", X_CIRCLE),
        ("info", INFO),
        ("flip-horizontal", FLIP_HORIZONTAL),
        ("flip-vertical", FLIP_VERTICAL),
        ("arrows-clockwise", ARROWS_CLOCKWISE),
        ("arrows-down-up", ARROWS_DOWN_UP),
        ("drop-half", DROP_HALF),
        ("drop-half-bottom", DROP_HALF_BOTTOM),
        ("sun", SUN),
        ("layer-blur", LAYER_BLUR),
        ("square-half", SQUARE_HALF),
        ("corner-top-left", CORNER_TOP_LEFT),
        ("corner-top-right", CORNER_TOP_RIGHT),
        ("corner-bottom-left", CORNER_BOTTOM_LEFT),
        ("corner-bottom-right", CORNER_BOTTOM_RIGHT),
        ("corners-separate", CORNERS_SEPARATE),
        ("squircle", SQUIRCLE),
        ("square-dots", SQUARE_DOTS),
        ("incognito", INCOGNITO),
        ("side-top", SIDE_TOP),
        ("side-right", SIDE_RIGHT),
        ("side-bottom", SIDE_BOTTOM),
        ("side-left", SIDE_LEFT),
        ("sides-separate", SIDES_SEPARATE),
        ("pad-square", PAD_SQUARE),
        ("checkerboard", CHECKERBOARD),
        ("square-fill", SQUARE_FILL),
        ("database", DATABASE),
        ("target", TARGET),
        ("hand-pointing", HAND_POINTING),
        ("devices", DEVICES),
        ("book-open", BOOK_OPEN),
        ("git-diff", GIT_DIFF),
        ("arrow-elbow-left-up", ARROW_ELBOW_LEFT_UP),
        ("arrow-up", ARROW_UP),
        ("arrow-down", ARROW_DOWN),
        ("arrow-line-up", ARROW_LINE_UP),
        ("arrow-line-down", ARROW_LINE_DOWN),
        ("arrows-out-line-horizontal", ARROWS_OUT_LINE_HORIZONTAL),
        ("arrows-out-line-vertical", ARROWS_OUT_LINE_VERTICAL),
        ("tidy-up", TIDY_UP),
        ("distribute-h", DISTRIBUTE_H),
        ("distribute-v", DISTRIBUTE_V),
        ("flex-layout", FLEX_LAYOUT),
        ("no-layout", NO_LAYOUT),
        ("columns", COLUMNS),
        ("rows", ROWS),
        ("ruler", RULER),
        ("trash-simple", TRASH_SIMPLE),
        ("check", CHECK),
        ("link-simple", LINK_SIMPLE),
        ("aspect-ratio", ASPECT_RATIO),
        ("globe", GLOBE),
        ("link-simple-horizontal-break", LINK_SIMPLE_HORIZONTAL_BREAK),
        ("magnet", MAGNET),
        ("crosshair-simple", CROSSHAIR_SIMPLE),
        ("crosshair", CROSSHAIR),
        ("pin", PIN),
        ("dots-six-vertical", DOTS_SIX_VERTICAL),
        ("sliders-horizontal", SLIDERS_HORIZONTAL),
        ("sliders", SLIDERS),
        ("dots-three-vertical", DOTS_THREE_VERTICAL),
        ("clock-counter-clockwise", CLOCK_COUNTER_CLOCKWISE),
        ("file", FILE),
        ("files", FILES),
        ("document", DOCUMENT),
        ("note", NOTE),
        ("squares-four", SQUARES_FOUR),
        ("pencil-simple", PENCIL_SIMPLE),
        ("knife", KNIFE),
        ("lasso", LASSO),
        ("arrow-u-up-left", ARROW_U_UP_LEFT),
        ("arrow-u-up-right", ARROW_U_UP_RIGHT),
        ("wrap", WRAP),
        ("wrap-reverse", WRAP_REVERSE),
        ("resize", RESIZE),
        ("scan", SCAN),
        ("unite-square", UNITE_SQUARE),
        ("subtract-square", SUBTRACT_SQUARE),
        ("intersect-square", INTERSECT_SQUARE),
        ("intersect-three", INTERSECT_THREE),
        ("exclude-square", EXCLUDE_SQUARE),
        ("arrows-merge", ARROWS_MERGE),
        ("arrows-left-right", ARROWS_LEFT_RIGHT),
        ("bounding-box", BOUNDING_BOX),
        ("list-dashes", LIST_DASHES),
        ("dash-length", DASH_LENGTH),
        ("dash-offset", DASH_OFFSET),
        ("stroke-cap", STROKE_CAP),
        ("arrow-left", ARROW_LEFT),
        ("arrow-right", ARROW_RIGHT),
        ("text-align-left", TEXT_ALIGN_LEFT),
        ("text-align-center", TEXT_ALIGN_CENTER),
        ("text-align-right", TEXT_ALIGN_RIGHT),
        ("text-align-justify", TEXT_ALIGN_JUSTIFY),
        ("text-aa", TEXT_AA),
        ("text-size", TEXT_SIZE),
        ("arrows-vertical", ARROWS_VERTICAL),
        ("arrows-horizontal", ARROWS_HORIZONTAL),
        ("text-bold", TEXT_BOLD),
        ("stroke-weight", STROKE_WEIGHT),
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
        ("hexagon-fill", HEXAGON_FILL),
        ("instance-local", INSTANCE_LOCAL),
        ("variant-set", VARIANT_SET),
        ("package", PACKAGE),
        ("browsers", BROWSERS),
        ("push-pin", PUSH_PIN),
        ("push-pin-slash", PUSH_PIN_SLASH),
        ("swatches", SWATCHES),
        ("plus-square", PLUS_SQUARE),
        ("palette", PALETTE),
        ("hash", HASH),
        ("toggle", TOGGLE),
        ("columns-plus-right", COLUMNS_PLUS_RIGHT),
        ("rows-plus-bottom", ROWS_PLUS_BOTTOM),
        ("key", KEY),
        ("mode", MODE),
        ("mode-default", MODE_DEFAULT),
        ("upload-simple", UPLOAD_SIMPLE),
        ("arrow-circle-down", ARROW_CIRCLE_DOWN),
        ("link-break", LINK_BREAK),
        ("arrow-up-right", ARROW_UP_RIGHT),
        ("arrow-square-out", ARROW_SQUARE_OUT),
        ("selection-all", SELECTION_ALL),
        ("warning", WARNING),
        ("brackets-curly", BRACKETS_CURLY),
        ("property-bound", PROPERTY_BOUND),
        ("code", CODE),
        ("stack", STACK),
        ("stack-simple", STACK_SIMPLE),
        ("divide", DIVIDE),
        ("shape-builder", SHAPE_BUILDER),
        ("transform-again", TRANSFORM_AGAIN),
        ("repeat-around", REPEAT_AROUND),
        ("blend-shapes", BLEND_SHAPES),
        ("spiral", SPIRAL),
        ("arc", ARC),
        ("warp", WARP),
        ("warp-free", WARP_FREE),
        ("warp-arc", WARP_ARC),
        ("warp-flag", WARP_FLAG),
        ("warp-bulge", WARP_BULGE),
        ("trace", TRACE),
        ("split-point", SPLIT_POINT),
        ("knife-cut", KNIFE_CUT),
        ("pixel-fit", PIXEL_FIT),
        ("toolbox", TOOLBOX),
        ("dice", DICE),
        ("fill-content", FILL_CONTENT),
        ("assets", ASSETS),
        ("wifi-slash", WIFI_SLASH),
        ("quotes", QUOTES),
        ("push-to-main", PUSH_TO_MAIN),
        ("chart", CHART),
        ("chart-column", CHART_COLUMN),
        ("chart-bar", CHART_BAR),
        ("chart-line", CHART_LINE),
        ("chart-area", CHART_AREA),
        ("chart-donut", CHART_DONUT),
        ("chart-scatter", CHART_SCATTER),
        ("chart-sankey", CHART_SANKEY),
        ("table", TABLE),
        ("visual-centre", VISUAL_CENTRE),
        ("size-preview", SIZE_PREVIEW),
        ("trash", TRASH),
        ("grid-four", GRID_FOUR),
        ("grid-layout", GRID_LAYOUT),
        ("image-tile", IMAGE_TILE),
        ("pixel-preview", PIXEL_PREVIEW),
        ("sign-in", SIGN_IN),
        ("export", EXPORT),
        ("import", IMPORT),
        ("file-image", FILE_IMAGE),
        ("file-svg", FILE_SVG),
        ("gauge", GAUGE),
        ("cpu", CPU),
        ("memory", MEMORY),
        ("file-zip", FILE_ZIP),
        ("folder-simple", FOLDER_SIMPLE),
        ("export-sync", EXPORT_SYNC),
        ("broom", BROOM),
        ("plugs-connected", PLUGS_CONNECTED),
        ("magic-wand", MAGIC_WAND),
        ("arrows-out-cardinal", ARROWS_OUT_CARDINAL),
        ("arrows-out", ARROWS_OUT),
        ("paint-bucket", PAINT_BUCKET),
        ("cursor-text", CURSOR_TEXT),
        ("sparkle", SPARKLE),
        ("stop", STOP),
        ("arrow-clockwise", ARROW_CLOCKWISE),
    ];
}

/// Name of the icon font family registered by [`install`].
const ICONS: &str = "ondin-icons";

/// Build a `RichText` that renders `glyph` from the icon font at `size`, in `col`.
pub fn icon_text(glyph: &str, size: f32, col: egui::Color32) -> egui::RichText {
    egui::RichText::new(glyph)
        .family(egui::FontFamily::Name(ICONS.into()))
        .size(size)
        .color(col)
}

/// A `FontId` for the icon font at `size` (for direct `painter.text`).
pub fn icon_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(ICONS.into()))
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
    // text, and the app's own icon font for icon glyphs.
    static INTER: &[u8] = include_bytes!("../../ondin-core/assets/fonts/Inter-Variable.ttf");
    static ICONS_TTF: &[u8] = include_bytes!("../assets/fonts/OndinIcons.ttf");
    static NOTO_EMOJI: &[u8] = include_bytes!("../assets/fonts/NotoEmoji.ttf");

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".to_owned(),
        Arc::new(egui::FontData::from_static(INTER)),
    );
    fonts.font_data.insert(
        ICONS.to_owned(),
        Arc::new(egui::FontData::from_static(ICONS_TTF)),
    );

    // A current Noto Emoji, monochrome, ahead of egui's own, which predates
    // Unicode 12 and draws 🟪 or 🟡 in an imported page name as tofu. Scaled as
    // egui scales its copy.
    fonts.font_data.insert(
        "noto-emoji".to_owned(),
        Arc::new(
            egui::FontData::from_static(NOTO_EMOJI).tweak(egui::FontTweak {
                scale: 0.81,
                ..Default::default()
            }),
        ),
    );

    // Inter leads both text families, the emoji next (built-in fonts remain as
    // fallbacks).
    let proportional = fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default();
    proportional.insert(0, "inter".to_owned());
    proportional.insert(1, "noto-emoji".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(1, "noto-emoji".to_owned());

    // A dedicated family for icon glyphs; falls back to Inter for any stray text.
    fonts.families.insert(
        egui::FontFamily::Name(ICONS.into()),
        vec![ICONS.to_owned(), "inter".to_owned()],
    );

    ctx.set_fonts(fonts);
}

fn install_style(ctx: &egui::Context) {
    let preference = match color::theme() {
        color::Theme::Dark => egui::ThemePreference::Dark,
        color::Theme::Light => egui::ThemePreference::Light,
    };
    ctx.set_theme(preference);
    ctx.all_styles_mut(install_style_into);
}

/// Paint the chrome in `theme` from the next frame: the palette, and egui's own
/// style, whose widget colours are copied out of the palette when installed.
pub fn apply(ctx: &egui::Context, theme: color::Theme) {
    color::set_theme(theme);
    install_style(ctx);
    ctx.request_repaint();
}

/// Bring the chrome in line with `pref`, resolving *System* against the OS (dark
/// when it does not say). Cheap when nothing changed, so it runs every frame —
/// which is also how an OS switch under *System* is followed. True when it
/// changed the theme: a `Ui` built earlier in the pass still holds the old style.
pub fn follow(ctx: &egui::Context, pref: crate::prefs::ThemePref) -> bool {
    use crate::prefs::ThemePref;
    let want = match pref {
        ThemePref::Dark => color::Theme::Dark,
        ThemePref::Light => color::Theme::Light,
        ThemePref::System => match ctx.system_theme() {
            Some(egui::Theme::Light) => color::Theme::Light,
            _ => color::Theme::Dark,
        },
    };
    let changed = want != color::theme();
    if changed {
        apply(ctx, want);
    }
    changed
}

fn install_style_into(style: &mut egui::Style) {
    use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

    // egui's text styles, from the chrome's type scale (`ui::*_PT`).
    use crate::ui::{BODY_PT, CAPTION_PT, CONTROL_PT, HEAD_PT};
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(HEAD_PT, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(BODY_PT, FontFamily::Proportional),
        ),
        // A button's word, a dropdown row, a `DragValue`'s number.
        (
            TextStyle::Button,
            FontId::new(CONTROL_PT, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(CAPTION_PT, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(CONTROL_PT, FontFamily::Monospace),
        ),
    ]
    .into();

    let v = &mut style.visuals;
    v.dark_mode = color::theme() == color::Theme::Dark;
    v.override_text_color = Some(color::text());
    v.panel_fill = color::bg();
    v.window_fill = color::card();
    v.extreme_bg_color = color::field(); // text-edit / input backgrounds
    v.faint_bg_color = color::card();
    v.hyperlink_color = color::accent();
    // Opaque on the window's own fill: a popup's stroke is painted beside its
    // fill, so a translucent one shows the fields under a dropdown.
    v.window_stroke = Stroke::new(1.0, color::card().blend(color::divider()));
    v.window_corner_radius = CornerRadius::same(6);
    // egui's default is darker and tighter than a menu's.
    v.popup_shadow = crate::ui::MENU_SHADOW;

    // Selection (used by text fields, selectable_label highlight, etc.).
    v.selection.bg_fill = color::accent_800();
    v.selection.stroke = Stroke::new(1.0, color::accent_300());

    let r5 = CornerRadius::same(5);
    let w = &mut v.widgets;

    // Non-interactive (labels, separators): no fill, faint divider strokes.
    w.noninteractive.bg_fill = Color32::TRANSPARENT;
    w.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    w.noninteractive.bg_stroke = Stroke::new(1.0, color::divider());
    w.noninteractive.fg_stroke = Stroke::new(1.0, text::muted());
    w.noninteractive.corner_radius = r5;

    // Inactive (resting buttons / controls): field ground, hairline border.
    w.inactive.bg_fill = color::field();
    w.inactive.weak_bg_fill = color::field();
    w.inactive.bg_stroke = Stroke::new(1.0, color::field_border());
    w.inactive.fg_stroke = Stroke::new(1.0, text::strong());
    w.inactive.corner_radius = r5;
    w.inactive.expansion = 0.0;

    // Hovered and pressed: the hover ground, the hairline unchanged, as
    // `ui::button_face` hovers. Open (a dropdown's list is up) keeps that face and
    // takes the accent_700 hairline that ties the list to its field.
    for ws in [&mut w.hovered, &mut w.active, &mut w.open] {
        ws.bg_fill = color::hover();
        ws.weak_bg_fill = color::hover();
        ws.bg_stroke = Stroke::new(1.0, color::field_border());
        ws.fg_stroke = Stroke::new(1.0, color::text());
        ws.corner_radius = r5;
        ws.expansion = 0.0;
    }
    w.open.bg_stroke = Stroke::new(1.0, color::accent_700());

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
    s.menu_margin = egui::Margin::same(crate::ui::MENU_PAD as i8);
    s.indent = 16.0;
    // Every dropdown's list is as tall as its rows up to this, then scrolls.
    s.combo_height = crate::ui::DROPDOWN_MAX_H;
}

/// WCAG contrast of the premultiplied `ink` composited over the opaque `ground`.
#[cfg(test)]
pub(crate) fn contrast(ink: egui::Color32, ground: egui::Color32) -> f32 {
    let lum = |c: [f32; 3]| {
        let lin = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    };
    let g = over(ink, ground);
    let (x, y) = (
        lum([
            g[0] as f32 / 255.0,
            g[1] as f32 / 255.0,
            g[2] as f32 / 255.0,
        ]),
        lum([
            ground[0] as f32 / 255.0,
            ground[1] as f32 / 255.0,
            ground[2] as f32 / 255.0,
        ]),
    );
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The premultiplied `top` composited over the opaque `ground`, opaque.
#[cfg(test)]
pub(crate) fn over(top: egui::Color32, ground: egui::Color32) -> egui::Color32 {
    let a = top.a() as f32 / 255.0;
    let c = |i: usize| (top[i] as f32 + ground[i] as f32 * (1.0 - a)).round() as u8;
    egui::Color32::from_rgb(c(0), c(1), c(2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    /// Emoji a Penpot page name carries, Unicode 12's coloured squares and circles
    /// among them, draw a glyph rather than the missing-glyph box, in both text
    /// families. `Fonts::has_glyph` answers false for every emoji whatever the
    /// fonts hold, so this compares atlas rects with an unassigned code point's.
    /// flip: drop "noto-emoji" from the proportional family → fails on 🟪.
    #[test]
    fn emoji_in_a_name_draw_a_glyph_not_tofu() {
        let ctx = egui::Context::default();
        install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        for font in [
            egui::FontId::proportional(12.0),
            egui::FontId::monospace(12.0),
        ] {
            let uv = |s: &str| {
                let g = ctx.fonts_mut(|f| {
                    f.layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE)
                });
                g.rows[0].glyphs[0].uv_rect
            };
            let tofu = uv("\u{10FFFD}");
            for c in ["🟪", "🟡", "❄", "🔶", "😀", "🫠"] {
                assert_ne!(uv(c), tofu, "{c} in {font:?}");
            }
        }
    }

    /// Every glyph the app names is laid out, read back out of the font atlas, and
    /// has to have ink that differs from the missing-glyph box: a wrong codepoint,
    /// or a font not rebuilt after `glyphs.json` changed, lays out fine and renders
    /// blank or as `.notdef`. It reads `icon::ALL`, so coverage cannot drift behind
    /// the constants; `the_glyph_source_names_every_icon_at_its_codepoint` pins
    /// each name to its codepoint.
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

        // A Private Use Area codepoint the icon font does not define, so the result
        // is the fallback's answer.
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
                 — the icon font does not define that codepoint; rebuild it from \
                 assets/icons/glyphs.json",
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

    /// Every icon constant this module's own source declares, as its name paired
    /// with the glyph behind it.
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
            "two constants in the icon module derive one name, so one of \
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
             glyph nothing draws."
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

    /// `assets/icons/glyphs.json`, the font's source, draws exactly the icons
    /// `icon::ALL` lists, each at the constant's codepoint. A drawing under another
    /// name or codepoint builds into a font that inks every glyph, so the atlas
    /// test cannot see it.
    ///
    /// flip: `hexagon-fill`'s codepoint in the JSON set to `U+E2AE` → fails at the
    /// codepoint assertion.
    #[test]
    fn the_glyph_source_names_every_icon_at_its_codepoint() {
        #[derive(serde::Deserialize)]
        struct Glyph {
            name: String,
            codepoint: String,
        }
        let source: Vec<Glyph> = serde_json::from_str(include_str!("../assets/icons/glyphs.json"))
            .expect("glyphs.json parses");
        let drawn: std::collections::BTreeMap<&str, &str> = source
            .iter()
            .map(|g| (g.name.as_str(), g.codepoint.as_str()))
            .collect();
        assert_eq!(
            drawn.len(),
            source.len(),
            "a name is drawn twice in glyphs.json"
        );

        let listed: std::collections::BTreeSet<&str> = icon::ALL.iter().map(|(n, _)| *n).collect();
        let undrawn: Vec<&&str> = listed.iter().filter(|n| !drawn.contains_key(**n)).collect();
        let unlisted: Vec<&&str> = drawn.keys().filter(|n| !listed.contains(**n)).collect();
        assert!(
            undrawn.is_empty() && unlisted.is_empty(),
            "glyphs.json and icon::ALL disagree. Listed but not drawn: {undrawn:?}. \
             Drawn but not listed: {unlisted:?}"
        );

        for (name, glyph) in icon::ALL {
            let want = format!("U+{:04X}", glyph.chars().next().map_or(0, u32::from));
            assert_eq!(
                drawn[name], want,
                "glyphs.json draws `{name}` at a codepoint the constant does not hold"
            );
        }
    }

    /// `OndinIcons.ttf` carries the stamp `build_font.py` writes from the
    /// `glyphs.json` it was built from, so a drawing edited without a rebuild
    /// fails here; the atlas test only sees that every codepoint inks.
    ///
    /// flip: one character of a path in glyphs.json changed without a rebuild →
    /// fails at the stamp assertion.
    #[test]
    fn the_font_was_built_from_the_current_glyphs() {
        // FNV-1a 64 with `\r` dropped, as `build_font.py`'s `source_stamp` does: a
        // CRLF checkout must stamp alike.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in include_str!("../assets/icons/glyphs.json")
            .bytes()
            .filter(|b| *b != b'\r')
        {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        let stamp = format!("glyphs {h:016x}");
        let font = include_bytes!("../assets/fonts/OndinIcons.ttf");
        assert!(
            font.windows(stamp.len()).any(|w| w == stamp.as_bytes()),
            "OndinIcons.ttf was not built from the current glyphs.json (want \
             `{stamp}`); rebuild it with assets/icons/build_font.py"
        );
    }

    /// Every chrome ink stays legible on every chrome ground in both palettes:
    /// each tier keeps a floor the dark theme was tuned to, and the light
    /// palette has to meet the same floors.
    /// flip: `text_a` without `LIGHT_INK_GAIN` → fails at `muted` over `bg`, Light;
    /// `on_scrim` returning `ink()` → `strong` on a wash; `select_ink` as `SELECT`
    /// → `select_ink/row`; no `LIGHT_INK_CAP` → "a step under text" (all Light).
    #[test]
    fn every_ink_reads_on_every_ground_in_both_themes() {
        let grounds = [
            ("bg", color::bg as fn() -> egui::Color32),
            ("topbar", color::topbar),
            ("panel", color::panel),
            ("card", color::card),
            ("field", color::field),
            ("hover", color::hover),
            ("raised", color::raised),
        ];
        // Text on a ground at 4.5:1 and up; a glyph or a hint at 3:1; the two
        // quiet tiers below that by design.
        let inks = [
            ("text", color::text as fn() -> egui::Color32, 10.0),
            ("strong", text::strong, 8.0),
            ("muted", text::muted, 4.5),
            ("dim", text::dim, 3.0),
            ("faint", text::faint, 2.5),
            ("disabled", text::disabled, 2.0),
            ("accent", color::accent, 3.5),
            ("accent_300", color::accent_300, 4.5),
            ("warn", color::warn, 3.0),
            ("star", color::star, 3.0),
            ("danger", color::danger, 3.0),
            ("saved", color::saved, 3.0),
            ("error", color::error, 4.5),
        ];
        for theme in [color::Theme::Dark, color::Theme::Light] {
            color::set_theme(theme);
            for (i, ink, floor) in &inks {
                for (g, ground) in &grounds {
                    let c = contrast(ink(), ground());
                    assert!(
                        c >= *floor,
                        "{theme:?}: {i} over {g} is {c:.2}, under {floor}"
                    );
                }
            }
            // A ramp's ink on its own grounds, and white type on a filled accent.
            for (name, ink, ground) in [
                ("accent_100/900", color::accent_100(), color::accent_900()),
                ("accent_100/800", color::accent_100(), color::accent_800()),
                ("accent_300/800", color::accent_300(), color::accent_800()),
                ("danger_200/900", color::danger_200(), color::danger_900()),
                ("saved_100/900", color::saved_100(), color::saved_900()),
                ("accent_ink/accent", color::accent_ink(), color::accent()),
                ("accent_ink/400", color::accent_ink(), color::accent_400()),
                ("on_accent/accent", color::on_accent(), color::accent()),
                (
                    "on_accent/accent_600",
                    color::on_accent(),
                    color::accent_600(),
                ),
            ] {
                let c = contrast(ink, ground);
                let floor = if name.starts_with("on_accent") {
                    2.5
                } else {
                    4.5
                };
                assert!(c >= floor, "{theme:?}: {name} is {c:.2}, under {floor}");
            }
            // The chrome's selection ink on a selected layer row and on the
            // library's ground.
            let row = over(color::SELECT_ROW, color::panel());
            for (name, ground) in [("select_ink/row", row), ("select_ink/bg", color::bg())] {
                let c = contrast(color::select_ink(), ground);
                assert!(c >= 4.5, "{theme:?}: {name} is {c:.2}, under 4.5");
            }
            // Ink on its fixed black wash, over the lightest and darkest artwork:
            // present mode's pills (180), a library card's chips (150). A mark's
            // hue on a wash over white is a short chip word, so its floor is low.
            for (name, ink, wash, floor) in [
                ("strong", color::on_scrim(text::strong), 180, 4.5),
                ("kit", color::on_scrim(|| color::text_a(179)), 150, 3.0),
                ("danger", color::on_scrim(color::danger), 150, 1.5),
                ("warn", color::on_scrim(color::warn), 150, 1.5),
            ] {
                for art in [egui::Color32::WHITE, egui::Color32::BLACK] {
                    let ground = over(egui::Color32::from_black_alpha(wash), art);
                    let c = contrast(ink, ground);
                    assert!(
                        c >= floor,
                        "{theme:?}: {name} on a {wash} wash over {art:?} is {c:.2}"
                    );
                }
            }
            assert_ne!(
                text::strong(),
                color::text(),
                "{theme:?}: strong is a step under text"
            );
        }
    }

    /// Applying a theme swaps the palette and egui's own style together, so a
    /// stock widget and a hand-painted one agree.
    /// flip: `install_style_into` writing `dark_mode = true` → fails at `dark_mode`.
    #[test]
    fn applying_a_theme_restyles_egui_with_it() {
        let ctx = egui::Context::default();
        install(&ctx);
        apply(&ctx, color::Theme::Light);
        assert_eq!(color::theme(), color::Theme::Light);
        let v = ctx.global_style().visuals.clone();
        assert!(!v.dark_mode, "egui is told the theme is light");
        assert_eq!(v.panel_fill, color::bg());
        assert_eq!(
            v.panel_fill, LIGHT_BG,
            "the fixture reached the light palette"
        );
        follow(&ctx, crate::prefs::ThemePref::Dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, color::bg());
        assert!(ctx.global_style().visuals.dark_mode, "and back");
    }

    /// The light palette's main ground, to check a fixture reached it.
    const LIGHT_BG: egui::Color32 = egui::Color32::from_rgb(236, 236, 238);
}
