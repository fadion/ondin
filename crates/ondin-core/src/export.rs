//! Export settings — what a layer is *for* (§7).
//!
//! A node carries a list of [`ExportSpec`], and each one names a file that layer
//! can produce: a format, a size, and a prefix saying where it should land.
//! Nothing here renders — the types live in core so
//! the document can hold them and the save format can write them; `ondin-export`
//! turns one into bytes and `ondin-app` into a file on disk.
//!
//! **They are document state, and that is the whole feature.** An export whose
//! settings live in a dialog has to be re-specified every time, which is the
//! thing that makes producing the same asset set twice a chore; kept on the
//! layer they are saved with the file, undone with everything else, and readable
//! by the CLI — so `ondin export --all` can regenerate a project's assets with no
//! window and nobody steering a dialog. The one-off case is not lost: *Export
//! as…* still asks, and is the row for "this once, somewhere else" (§15 D264).
//!
//! **Deliberately not a `NodeKind` gate.** Any node can carry specs, including a
//! group, a frame and the root — "which layers can be exported" is the same
//! question as "which layers have bounds", and the raster writer already answers
//! that by refusing a subject with no extent rather than by consulting a kind.

use peniko::Color;
use serde::{Deserialize, Serialize};

/// One file a layer produces: a format, a size and where it is put.
///
/// **The wire form and the model are the same type**, as they are for `Paint`
/// and `NodeKind` — `io::schema` embeds this directly rather than mirroring it,
/// so a field added here is a save-format change and its `serde` attributes are
/// the migration story. Every field but `format` is skipped at its default, so a
/// document full of plain 1× PNGs writes `{"format":"Png"}` and nothing else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportSpec {
    /// What comes out. The only field with no default: a spec that did not say
    /// would be a spec that produced nothing in particular.
    pub format: ExportFormat,
    /// How big, for the formats that have a size at all.
    #[serde(default, skip_serializing_if = "ExportScale::is_1x")]
    pub scale: ExportScale,
    /// Put in front of the layer's name — `hero/` + `Button` → `hero/Button.png`.
    ///
    /// **A prefix, where Figma's field is a suffix, and the two are for different
    /// jobs.** Figma's exists to keep `Button.png` and `Button@2x.png` apart, so
    /// it has to be typed on every second row and is `@2x` almost every time it is
    /// used at all. Here that separation is automatic ([`ExportScale::marker`]) and the
    /// field is left for the thing it cannot do: *grouping*. With
    /// `PlanOptions::folders_from_names` on, a prefix ending in `/` is a folder,
    /// which is how one export set lands in `hero/` and another in `icons/`
    /// without either layer being renamed.
    ///
    /// Sanitized by `ondin_export::plan`, not here — the same treatment the
    /// layer's own name gets, and by the same function, so a `/` means the same
    /// thing in both halves of a filename.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    /// Put after the layer's name, **overriding the scale's own marker** —
    /// `Button` + `-dark` → `Button-dark.png`.
    ///
    /// **Empty means "whatever keeps this row distinct", not "nothing".** Two
    /// specs differing only in scale must not write one file, and the number that
    /// separates them is already on the row, so an untouched suffix derives itself
    /// ([`ExportScale::marker`]) rather than leaving the user to type `@2x` on
    /// every second row and to notice when they have not. Typing one takes it
    /// over completely: `-dark` is a name a person chose, and appending `@2x` to
    /// it would be the app arguing with them.
    ///
    /// So the field is not a *correctness* burden the way Figma's is — it can only
    /// ever be reached for when the derived answer is not the wanted one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suffix: String,
    /// What fills the parts of the frame the layer does not cover.
    #[serde(default, skip_serializing_if = "ExportBackground::is_transparent")]
    pub background: ExportBackground,
    /// JPEG quality, 1–100. Inert for every other format, and kept rather than
    /// dropped when the format changes: switching PNG→JPEG→PNG must not throw
    /// away a number the user set.
    #[serde(
        default = "default_quality",
        skip_serializing_if = "is_default_quality"
    )]
    pub quality: u8,
    /// Drop fully transparent rows and columns from the edges of the result.
    ///
    /// **Raster only**, because it is measured on pixels: the alpha of the
    /// rendered image is the only thing in this crate's reach that knows where a
    /// blur or an outside stroke actually stopped. An SVG's `viewBox` comes from
    /// geometric bounds, which are exactly the bounds this would be trimming
    /// *from*, so there is nothing for it to do there.
    #[serde(default, skip_serializing_if = "is_false")]
    pub trim: bool,
    /// Pad the result out to a square with [`Self::background`].
    ///
    /// The framing want [`ExportScale::Width`] does not cover: an icon set whose
    /// members are 24×18 and 20×24 exports as one grid of squares, which is what
    /// every icon consumer wants and what no bounds-tight export gives.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pad_square: bool,
}

impl ExportSpec {
    /// A spec at this format's ordinary settings.
    pub fn new(format: ExportFormat, scale: ExportScale) -> Self {
        Self {
            format,
            prefix: String::new(),
            suffix: String::new(),
            scale,
            background: ExportBackground::default(),
            quality: default_quality(),
            trim: false,
            pad_square: false,
        }
    }

    /// What actually goes after the name: [`Self::suffix`] when one was typed,
    /// else the marker the scale derives — `@2x`, `@512w`, and nothing at 1×.
    ///
    /// **The derived half is what makes the typed half optional.** A suffix that
    /// had to be typed is a correctness burden: clear it and two rows quietly
    /// write one file. Deriving the default makes that collision impossible while
    /// leaving the field there for the cases a number cannot express — `-dark`,
    /// `-rtl`, a name a consumer requires.
    ///
    /// **The derived marker is empty for a vector format**, whatever the scale
    /// says: an SVG has no resolution, so `Button@2x.svg` would be a name
    /// asserting something the file does not have. Two SVG rows differing only in
    /// scale are then the same file twice, which is a degenerate spec and is
    /// caught by `plan`'s de-collision rather than by a name that lies. A *typed*
    /// suffix is used as-is on any format, because it is not a claim about pixels.
    pub fn effective_suffix(&self) -> String {
        if !self.suffix.is_empty() {
            return self.suffix.clone();
        }
        match self.format.is_raster() {
            true => self.scale.marker(),
            false => String::new(),
        }
    }

    /// The stem this spec gives a layer called `name` — prefix, name, marker, and
    /// no extension.
    ///
    /// **Unsanitized, deliberately.** This is the naming *rule* and nothing else,
    /// so the CLI and the panel cannot come to disagree about the order of the
    /// three parts; `ondin_export::plan` runs the result through the one function
    /// that knows what a filesystem will accept, which is also what makes a `/` in
    /// the prefix mean exactly what a `/` in the layer's name means.
    pub fn compose(&self, name: &str) -> String {
        format!("{}{name}{}", self.prefix, self.effective_suffix())
    }

    /// [`Self::compose`] with the extension on it.
    pub fn file_name(&self, name: &str) -> String {
        format!("{}.{}", self.compose(name), self.format.extension())
    }

    /// Whether this spec's controls are all live for its format — false when
    /// something on the row is inert and the panel should say so rather than
    /// let it be set to a value that does nothing.
    pub fn scale_applies(&self) -> bool {
        self.format.is_raster()
    }
}

/// A raster's size, as the export panel offers it.
///
/// **Three ways to say it, because designers have three questions.** A
/// multiplier is "the same drawing, denser" — the icon case, where 1× and 2× are
/// the same asset twice. A width or a height is "whatever it takes to be this
/// big" — the hero-image case, where the number is a requirement from the
/// consumer and the drawing's own size is incidental. Figma offers exactly this
/// set (`0.5x…4x`, `512w`, `512h`) and the reasoning is not really Figma's: they
/// are the two independent questions plus the axis you pin.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExportScale {
    /// `n` device pixels per world unit.
    Times(f32),
    /// Whatever multiplier makes the result this many pixels wide.
    Width(u32),
    /// Whatever multiplier makes the result this many pixels tall.
    Height(u32),
}

impl Default for ExportScale {
    fn default() -> Self {
        ExportScale::Times(1.0)
    }
}

impl ExportScale {
    /// The multipliers the dropdown offers, Figma's list.
    pub const COMMON: [f32; 7] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];

    /// The multiplier this asks for, against a subject `w` × `h` world units.
    ///
    /// **`Width`/`Height` need the subject and `Times` does not**, which is why
    /// this takes the box rather than being a field read: the same spec on two
    /// differently-shaped layers is two different multipliers, and that is the
    /// point of it.
    ///
    /// Guarded at both ends. A subject with no extent on the pinned axis would
    /// make this a division by zero — a horizontal `Line` has exactly that, and
    /// `png::viewport_over` already carries the same floor for the same reason —
    /// and the result is clamped to something that can be rendered, so a `0.5×`
    /// of a 1px line asks for one pixel rather than none.
    pub fn factor(self, w: f64, h: f64) -> f64 {
        let f = match self {
            ExportScale::Times(n) => n as f64,
            ExportScale::Width(px) => px as f64 / w.max(1.0),
            ExportScale::Height(px) => px as f64 / h.max(1.0),
        };
        if f.is_finite() { f.max(1e-4) } else { 1.0 }
    }

    /// What this reads as on the row: `2x`, `512w`, `0.5x`.
    pub fn label(self) -> String {
        match self {
            ExportScale::Times(n) => format!("{}x", trim_zeros(n as f64)),
            ExportScale::Width(px) => format!("{px}w"),
            ExportScale::Height(px) => format!("{px}h"),
        }
    }

    /// [`Self::label`] read back, for a command line that has to name one of the
    /// three in a single argument (`ondin export --all --scale-override 512w`).
    ///
    /// **The inverse of the label rather than a syntax of its own**, so the thing
    /// a person reads off the export row is the thing they can type — and so there
    /// is one vocabulary to learn rather than a UI one and a CLI one. A bare
    /// number is `x`, which is the only spelling this adds: it is the dominant
    /// case, and the `--scale` flag beside it takes a bare number for the same
    /// reason.
    ///
    /// `None` for anything that is not one of the four shapes, and for a value
    /// that cannot be a size — zero, negative, and the non-finite multipliers a
    /// `f32` parse will happily produce.
    pub fn from_label(text: &str) -> Option<Self> {
        let text = text.trim();
        let (digits, tail) = match text.chars().last()? {
            c @ ('x' | 'X' | 'w' | 'W' | 'h' | 'H') => (&text[..text.len() - 1], Some(c)),
            _ => (text, None),
        };
        match tail {
            Some('w' | 'W') => digits.parse().ok().filter(|px| *px > 0).map(Self::Width),
            Some('h' | 'H') => digits.parse().ok().filter(|px| *px > 0).map(Self::Height),
            // `None` is the bare number, which means the same as `x`.
            _ => digits
                .parse()
                .ok()
                .filter(|n: &f32| n.is_finite() && *n > 0.0)
                .map(Self::Times),
        }
    }

    /// What this scale adds to a filename — `@2x`, `@512w`.
    ///
    /// Empty at 1×, because the ordinary export wants the layer's plain name and
    /// an `@1x` on every file is noise nobody asked for. That is also what keeps
    /// the *first* row of a set unmarked while the ones beside it are marked,
    /// which is the arrangement every asset pipeline already expects.
    pub fn marker(self) -> String {
        match self {
            ExportScale::Times(n) if (n - 1.0).abs() < f32::EPSILON => String::new(),
            other => format!("@{}", other.label()),
        }
    }

    fn is_1x(&self) -> bool {
        matches!(self, ExportScale::Times(n) if (*n - 1.0).abs() < f32::EPSILON)
    }
}

/// What comes out of an export.
///
/// **PDF is not here yet, deliberately.** The cheapest honest route to it is our
/// own SVG through a converter — §7 already asserts SVG as the
/// model-completeness gate, so PDF would inherit that gate rather than needing a
/// third per-kind writer — but it is a dependency tree and a decision about
/// text-as-outlines, and a dropdown entry that half-works is worse than one that
/// is not there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExportFormat {
    #[default]
    Png,
    Jpeg,
    Svg,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] = [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Svg];

    /// The extension, lowercase and without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Svg => "svg",
        }
    }

    /// What the dropdown shows.
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Png => "PNG",
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::Svg => "SVG",
        }
    }

    /// Whether this goes through the CPU rasterizer — which is also whether a
    /// scale, a trim and a pad mean anything for it.
    pub fn is_raster(self) -> bool {
        matches!(self, ExportFormat::Png | ExportFormat::Jpeg)
    }

    /// Whether the format can carry transparency at all. JPEG cannot, so
    /// [`ExportBackground::Transparent`] resolves to a matte for it rather than
    /// being refused — the value is kept, and what it *means* is what changes.
    pub fn keeps_alpha(self) -> bool {
        !matches!(self, ExportFormat::Jpeg)
    }
}

/// What fills the parts of an export the layer itself does not cover.
///
/// `png_of` puts a subtree on transparency by construction — the walk starts at
/// the layer, so nothing behind it is painted (§15 D264) — which is the right
/// default and the wrong answer often enough to need a control: an icon lifted
/// off its frame usually wants the frame's own background behind it, and a JPEG
/// has nowhere to put transparency at all. One control answers both, rather than
/// JPEG carrying a matte field nothing else reads.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub enum ExportBackground {
    /// Nothing behind. For a JPEG this is white, which is the one place the
    /// stored value and the produced file differ — and the panel says so rather
    /// than rewriting the setting behind the user's back.
    #[default]
    Transparent,
    /// The background of the nearest enclosing frame, or nothing when the layer
    /// is not in one or the frame has no background of its own.
    Frame,
    Solid(Color),
}

impl ExportBackground {
    fn is_transparent(&self) -> bool {
        matches!(self, ExportBackground::Transparent)
    }

    /// What the row shows for it.
    pub fn label(self) -> &'static str {
        match self {
            ExportBackground::Transparent => "Transparent",
            ExportBackground::Frame => "Frame",
            ExportBackground::Solid(_) => "Colour",
        }
    }
}

/// JPEG's default quality. 90 rather than the encoder's own 75: this is a design
/// tool's output, where a visible artefact is a bug report and the extra bytes
/// are nobody's bandwidth.
fn default_quality() -> u8 {
    90
}

fn is_default_quality(q: &u8) -> bool {
    *q == default_quality()
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// `2` not `2.0`, `0.5` not `0.50` — the shortest form that reads back the same.
fn trim_zeros(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scale_to_width_is_the_multiplier_that_reaches_it() {
        // 200 world units wide asked to be 512 px: 2.56×, and the height follows,
        // which is the whole difference from `Times`.
        assert!((ExportScale::Width(512).factor(200.0, 100.0) - 2.56).abs() < 1e-9);
        assert!((ExportScale::Height(512).factor(200.0, 100.0) - 5.12).abs() < 1e-9);
        assert!((ExportScale::Times(2.0).factor(200.0, 100.0) - 2.0).abs() < 1e-9);
    }

    /// A horizontal line has no height at all, and `png::viewport_over` floors
    /// the *view* for exactly this reason. Without the floor here the factor is
    /// an infinity and the pixel count that follows is meaningless rather than
    /// merely wrong.
    #[test]
    fn a_degenerate_subject_does_not_make_the_factor_infinite() {
        let f = ExportScale::Height(512).factor(200.0, 0.0);
        assert!(f.is_finite(), "got {f}");
        assert_eq!(
            f, 512.0,
            "a zero extent is floored to one unit, not to zero"
        );
    }

    /// The marker is empty at 1× and present everywhere else, because two specs
    /// differing only in scale must not name the same file.
    #[test]
    fn the_marker_is_empty_only_at_1x() {
        assert_eq!(ExportScale::Times(1.0).marker(), "");
        assert_eq!(ExportScale::Times(2.0).marker(), "@2x");
        assert_eq!(ExportScale::Times(0.5).marker(), "@0.5x");
        assert_eq!(ExportScale::Width(512).marker(), "@512w");
    }

    /// **A vector file carries no marker however it is scaled**, because it has no
    /// resolution for one to describe — where the same scale on a PNG must mark
    /// it, or the two rows write one file.
    #[test]
    fn only_a_raster_spec_marks_its_scale() {
        let png = ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0));
        let svg = ExportSpec::new(ExportFormat::Svg, ExportScale::Times(2.0));
        assert_eq!(png.effective_suffix(), "@2x");
        assert_eq!(svg.effective_suffix(), "");
        assert_eq!(png.file_name("Button"), "Button@2x.png");
        assert_eq!(svg.file_name("Button"), "Button.svg");
    }

    /// **A typed suffix takes the derived one's place rather than joining it** —
    /// `Button-dark@2x.png` is what appending would give, and it is the app
    /// arguing with a name the user chose.
    ///
    /// Both formats, because the derived half is format-dependent and the typed
    /// half is not: a suffix on an SVG is a name, not a claim about pixels.
    #[test]
    fn a_typed_suffix_replaces_the_derived_one() {
        let mut png = ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0));
        png.suffix = "-dark".into();
        assert_eq!(png.effective_suffix(), "-dark");
        assert_eq!(png.file_name("Button"), "Button-dark.png");

        let mut svg = ExportSpec::new(ExportFormat::Svg, ExportScale::Times(2.0));
        svg.suffix = "-dark".into();
        assert_eq!(svg.file_name("Button"), "Button-dark.svg");
    }

    /// The prefix goes in front and the suffix at the back, so a folder prefix and
    /// a density marker cannot end up on the same side of the name.
    #[test]
    fn a_prefix_leads_and_the_suffix_trails() {
        let spec = ExportSpec {
            prefix: "hero/".into(),
            ..ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0))
        };
        assert_eq!(spec.compose("Button"), "hero/Button@2x");
        assert_eq!(spec.file_name("Button"), "hero/Button@2x.png");
    }

    #[test]
    fn a_plain_png_spec_writes_one_key() {
        let spec = ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0));
        let json = serde_json::to_string(&spec).expect("serialize");
        assert_eq!(
            json, r#"{"format":"Png"}"#,
            "every field but the format is skipped at its default"
        );
        assert_eq!(
            serde_json::from_str::<ExportSpec>(&json).expect("round trip"),
            spec
        );
    }

    /// `from_label` is the inverse of `label`, which is the whole of why it exists:
    /// what a person reads off an export row is what they can type at
    /// `--scale-override`.
    ///
    /// **The round trip over `COMMON` is the assertion that matters**, because it
    /// is the only one that cannot drift when `label`'s formatting changes —
    /// `trim_zeros` is what writes `0.5x` rather than `0.50x`, and a hand-written
    /// table of strings here would go on passing while the two halves disagreed.
    ///
    /// ⚠️ **Flipped against the plausible wrong version**: a `from_label` that
    /// treats a bare number as an error rather than as `x`. Predicted to fail on
    /// the bare-number line, and it does — the `COMMON` round trip stays green,
    /// since `label` never writes a bare number. So the round trip has no teeth
    /// for that one spelling and the explicit line beside it is load-bearing.
    #[test]
    fn a_scale_reads_back_from_the_label_it_writes() {
        for n in ExportScale::COMMON {
            let s = ExportScale::Times(n);
            assert_eq!(
                ExportScale::from_label(&s.label()),
                Some(s),
                "{}",
                s.label()
            );
        }
        for s in [ExportScale::Width(512), ExportScale::Height(1024)] {
            assert_eq!(
                ExportScale::from_label(&s.label()),
                Some(s),
                "{}",
                s.label()
            );
        }

        // A bare number is `x`, the one spelling this adds to the vocabulary.
        assert_eq!(ExportScale::from_label("2"), Some(ExportScale::Times(2.0)));
        assert_eq!(
            ExportScale::from_label("0.5"),
            Some(ExportScale::Times(0.5))
        );
        // Case is not a second spelling to remember, as it is not for `--scale`.
        assert_eq!(
            ExportScale::from_label("512W"),
            Some(ExportScale::Width(512))
        );

        // Nothing that cannot be a size. `0w` and `-2` are the ones a shell
        // produces by accident; `inf` is what an unguarded `f32` parse accepts.
        for bad in [
            "", "x", "w", "0", "0x", "0w", "-2", "-2x", "inf", "2px", "abc", "2.5w",
        ] {
            assert_eq!(ExportScale::from_label(bad), None, "{bad:?} was accepted");
        }
    }

    /// The naming rule is one function so the CLI and the panel cannot disagree
    /// about the order of its three parts.
    #[test]
    fn the_file_name_is_name_then_marker_then_extension() {
        let spec = ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0));
        assert_eq!(spec.file_name("Button"), "Button@2x.png");
        assert_eq!(
            ExportSpec::new(ExportFormat::Jpeg, ExportScale::Times(1.0)).file_name("Hero"),
            "Hero.jpg"
        );
    }
}
