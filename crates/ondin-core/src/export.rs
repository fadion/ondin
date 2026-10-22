//! Export settings — what a layer is *for* (§7).
//!
//! A node carries a list of [`ExportSpec`], each naming a file that layer can
//! produce. Nothing here renders: `ondin-export` turns a spec into bytes and
//! `ondin-app` into a file.
//!
//! The specs are document state, so they are saved, undone and readable by the
//! CLI (`ondin export --all`); *Export as…* remains the one-off.
//!
//! Not a `NodeKind` gate: any node can carry specs, and the raster writer refuses
//! a subject with no extent rather than consulting a kind.

use peniko::Color;
use serde::{Deserialize, Serialize};

/// One file a layer produces: a format, a size and where it is put.
///
/// The wire form and the model are the same type (`io::schema` embeds it), so a
/// field added here is a save-format change. Every field but `format` is skipped
/// at its default: a plain 1× PNG writes `{"format":"Png"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportSpec {
    /// What comes out. The only field with no default.
    pub format: ExportFormat,
    /// How big, for the formats that have a size at all.
    #[serde(default, skip_serializing_if = "ExportScale::is_1x")]
    pub scale: ExportScale,
    /// Put in front of the layer's name — `hero/` + `Button` → `hero/Button.png`.
    ///
    /// The scale marker is automatic ([`ExportScale::marker`]), so the prefix is
    /// for grouping: with `PlanOptions::folders_from_names` on, a prefix ending in
    /// `/` is a folder. Sanitized by `ondin_export::plan`, not here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    /// Put after the layer's name, **overriding the scale's own marker** —
    /// `Button` + `-dark` → `Button-dark.png`.
    ///
    /// Empty means "whatever keeps this row distinct" ([`ExportScale::marker`]),
    /// so two specs differing only in scale never write one file. A typed suffix
    /// takes over completely; `@2x` is not appended to a name the user chose.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suffix: String,
    /// What fills the parts of the frame the layer does not cover.
    #[serde(default, skip_serializing_if = "ExportBackground::is_transparent")]
    pub background: ExportBackground,
    /// JPEG quality, 1–100. Inert for other formats but kept when the format
    /// changes, so PNG→JPEG→PNG does not lose the user's number.
    #[serde(
        default = "default_quality",
        skip_serializing_if = "is_default_quality"
    )]
    pub quality: u8,
    /// Drop fully transparent rows and columns from the edges of the result.
    ///
    /// Raster only: it is measured on the rendered alpha, the only thing that
    /// knows where a blur or outside stroke stopped. An SVG's `viewBox` comes from
    /// geometric bounds, which is what this would trim from.
    #[serde(default, skip_serializing_if = "is_false")]
    pub trim: bool,
    /// Pad the result out to a square with [`Self::background`], so an icon set of
    /// mixed sizes exports as one grid of squares.
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
    /// The derived marker is empty for a vector format whatever the scale: an SVG
    /// has no resolution, so `Button@2x.svg` would claim one. Two SVG rows
    /// differing only in scale are a degenerate spec, caught by `plan`'s
    /// de-collision. A typed suffix is used as-is on any format.
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
    /// Unsanitized, deliberately: this is only the naming rule, shared by the CLI
    /// and the panel; `ondin_export::plan` sanitizes the result.
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
/// A multiplier is "the same drawing, denser"; a width or height is "whatever it
/// takes to be this big", for when the consumer dictates the size.
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
    /// The multipliers the dropdown offers.
    pub const COMMON: [f32; 7] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];

    /// The multiplier this asks for, against a subject `w` × `h` world units.
    ///
    /// `Width`/`Height` depend on the subject, so the same spec on two layers is
    /// two multipliers. Guarded at both ends: the extent is floored at 1 (a
    /// horizontal `Line` has no height, as in `png::viewport_over`) and the result
    /// is clamped to something renderable.
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
    /// The inverse of the label, so what a person reads off the row is what they
    /// can type; a bare number is the one added spelling and means `x`. `None`
    /// for anything else and for a value that cannot be a size — zero, negative,
    /// non-finite.
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
    /// Empty at 1×, so the ordinary export keeps the layer's plain name.
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
/// PDF is deliberately absent: it would go through our SVG and a converter, which
/// is a dependency tree and a text-as-outlines decision.
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

    /// Whether this goes through the CPU rasterizer, and so whether a scale, trim
    /// and pad mean anything for it.
    pub fn is_raster(self) -> bool {
        matches!(self, ExportFormat::Png | ExportFormat::Jpeg)
    }

    /// Whether the format can carry transparency. JPEG cannot, so
    /// [`ExportBackground::Transparent`] resolves to a matte for it.
    pub fn keeps_alpha(self) -> bool {
        !matches!(self, ExportFormat::Jpeg)
    }
}

/// What fills the parts of an export the layer itself does not cover.
///
/// `png_of` starts its walk at the layer, so nothing behind it is painted (§15
/// D264); this control covers an icon that wants its frame's background and a
/// JPEG, which has no transparency.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub enum ExportBackground {
    /// Nothing behind. For a JPEG this is white; the stored value is not rewritten.
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

/// JPEG's default quality: 90 rather than the encoder's 75, as a visible artefact
/// costs more than the bytes.
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
        // 200 wide asked to be 512 px: 2.56×, and the height follows.
        assert!((ExportScale::Width(512).factor(200.0, 100.0) - 2.56).abs() < 1e-9);
        assert!((ExportScale::Height(512).factor(200.0, 100.0) - 5.12).abs() < 1e-9);
        assert!((ExportScale::Times(2.0).factor(200.0, 100.0) - 2.0).abs() < 1e-9);
    }

    /// A subject with zero extent on the pinned axis floors to one unit, not an
    /// infinite factor.
    #[test]
    fn a_degenerate_subject_does_not_make_the_factor_infinite() {
        let f = ExportScale::Height(512).factor(200.0, 0.0);
        assert!(f.is_finite(), "got {f}");
        assert_eq!(
            f, 512.0,
            "a zero extent is floored to one unit, not to zero"
        );
    }

    /// The marker is empty at 1× and present elsewhere.
    #[test]
    fn the_marker_is_empty_only_at_1x() {
        assert_eq!(ExportScale::Times(1.0).marker(), "");
        assert_eq!(ExportScale::Times(2.0).marker(), "@2x");
        assert_eq!(ExportScale::Times(0.5).marker(), "@0.5x");
        assert_eq!(ExportScale::Width(512).marker(), "@512w");
    }

    /// A vector file carries no marker however it is scaled; a PNG does.
    #[test]
    fn only_a_raster_spec_marks_its_scale() {
        let png = ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0));
        let svg = ExportSpec::new(ExportFormat::Svg, ExportScale::Times(2.0));
        assert_eq!(png.effective_suffix(), "@2x");
        assert_eq!(svg.effective_suffix(), "");
        assert_eq!(png.file_name("Button"), "Button@2x.png");
        assert_eq!(svg.file_name("Button"), "Button.svg");
    }

    /// A typed suffix replaces the derived one on both formats, rather than
    /// giving `Button-dark@2x.png`.
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

    /// The prefix goes in front of the name and the suffix behind it.
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

    /// `from_label` reads back what `label` writes (round trip over `COMMON`), plus
    /// a bare number and any case, and refuses what cannot be a size.
    ///
    /// flip: `from_label` rejecting a bare number → fails at the bare-number
    /// line; the `COMMON` round trip stays green.
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

        // Nothing that cannot be a size; `inf` is what an unguarded `f32` parse accepts.
        for bad in [
            "", "x", "w", "0", "0x", "0w", "-2", "-2x", "inf", "2px", "abc", "2.5w",
        ] {
            assert_eq!(ExportScale::from_label(bad), None, "{bad:?} was accepted");
        }
    }

    /// A file name is name, then marker, then extension.
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
