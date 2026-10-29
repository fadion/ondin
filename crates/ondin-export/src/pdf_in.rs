//! PDF import: each page drawn as SVG by `hayro-svg`, for `ondin_core::svg_in` to
//! read as it reads a pasted SVG. One reader for both formats, so a PDF's fills,
//! strokes, gradients, clips and pictures arrive by the paths SVG import already
//! tests. Text arrives as the glyphs' outlines: a PDF places glyphs, not
//! paragraphs. `hayro-svg` draws each glyph as a `<use>` of its outline, which
//! would be a layer per letter, so [`merged_glyphs`] joins each line's into one
//! path first.

use ondin_core::kurbo::{Affine, BezPath, Size};
use std::sync::Arc;

/// The most pages one import reads. Past it the rest are counted, not drawn: a
/// manual of thousands of pages is not something to lay out as frames, and the
/// import runs on the frame — each page costs some tens of milliseconds.
pub const MAX_PAGES: usize = 50;

/// One page, as SVG markup and its size in points (one world unit each).
pub struct PdfPage {
    pub svg: String,
    pub size: Size,
}

/// A PDF's pages, up to [`MAX_PAGES`].
pub struct PdfImport {
    pub pages: Vec<PdfPage>,
    /// Pages past [`MAX_PAGES`], not read.
    pub left_out: usize,
}

/// Why a PDF could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfInError {
    /// Encrypted, and the empty password does not open it.
    Encrypted,
    /// Not a PDF this reader can parse, or one with no pages.
    Unreadable,
}

/// Whether `bytes` look like a PDF: the `%PDF-` header within the first kilobyte,
/// where readers accept it after leading junk.
pub fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(1024)]
        .windows(5)
        .any(|w| w == b"%PDF-")
}

/// Read `bytes` and draw each page, up to [`MAX_PAGES`], as SVG.
pub fn import(bytes: Vec<u8>) -> Result<PdfImport, PdfInError> {
    use hayro_svg::hayro_syntax::{LoadPdfError, Pdf};
    let pdf = Pdf::new(Arc::new(bytes)).map_err(|e| match e {
        LoadPdfError::Decryption(_) => PdfInError::Encrypted,
        _ => PdfInError::Unreadable,
    })?;
    let all = pdf.pages();
    if all.is_empty() {
        return Err(PdfInError::Unreadable);
    }
    let cache = hayro_svg::RenderCache::new();
    let settings = hayro_svg::hayro_interpret::InterpreterSettings::default();
    let render = hayro_svg::SvgRenderSettings::default();
    let pages = all
        .iter()
        .take(MAX_PAGES)
        .map(|page| {
            let (w, h) = page.render_dimensions();
            let svg = hayro_svg::convert(page, &cache, &settings, &render);
            PdfPage {
                svg: merged_glyphs(&svg).unwrap_or(svg),
                size: Size::new(f64::from(w), f64::from(h)),
            }
        })
        .collect();
    Ok(PdfImport {
        pages,
        left_out: all.len().saturating_sub(MAX_PAGES),
    })
}

/// `svg` with each run of sibling `<use>`s of glyph outlines that share their
/// paint and their line merged into one `<path>` named *Text*, the outlines
/// placed by each `<use>`'s transform. `None` when the markup does not parse,
/// leaving the caller the markup as it was.
///
/// A run is one line because a `<use>` on another baseline, at another size or in
/// another paint starts the next. The rest of the markup is kept byte for byte:
/// each run's span is replaced in the text, not re-serialised.
pub fn merged_glyphs(svg: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(svg).ok()?;
    let glyphs: std::collections::HashMap<&str, &str> = doc
        .descendants()
        .filter(|n| n.has_tag_name("path") && n.ancestors().any(|a| a.has_tag_name("defs")))
        .filter_map(|n| Some((n.attribute("id")?, n.attribute("d")?)))
        .collect();
    let line = |a: Affine| {
        let [sx, kx, ky, sy, _, ty] = a.as_coeffs();
        [sx, kx, ky, sy, ty]
    };
    let same_line = |a: Affine, b: Affine| {
        line(a)
            .iter()
            .zip(line(b))
            .all(|(x, y)| (x - y).abs() <= 1e-3 * x.abs().max(y.abs()).max(1.0))
    };
    let mut runs: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for parent in doc.descendants().filter(|n| n.is_element()) {
        let mut run: Vec<(std::ops::Range<usize>, &str, Affine, Attrs)> = Vec::new();
        let mut close = |run: &mut Vec<(std::ops::Range<usize>, &str, Affine, Attrs)>| {
            let (Some(first), Some(last)) = (run.first(), run.last()) else {
                return;
            };
            let mut path = BezPath::new();
            for (_, d, at, _) in run.iter() {
                if let Ok(outline) = BezPath::from_svg(d) {
                    path.extend((*at * outline).iter());
                }
            }
            let attrs: String = first
                .3
                .iter()
                .map(|(k, v)| format!(r#" {k}="{}""#, escape_attr(v)))
                .collect();
            let markup = format!(r#"<path data-name="Text" d="{}"{attrs}/>"#, path.to_svg());
            runs.push((first.0.start..last.0.end, markup));
            run.clear();
        };
        for child in parent.children().filter(|c| c.is_element()) {
            match glyph(&glyphs, child) {
                Some((d, at, rest))
                    if run
                        .last()
                        .is_none_or(|(_, _, a, r)| *r == rest && same_line(*a, at)) =>
                {
                    run.push((child.range(), d, at, rest));
                }
                Some((d, at, rest)) => {
                    close(&mut run);
                    run.push((child.range(), d, at, rest));
                }
                None => close(&mut run),
            }
        }
        close(&mut run);
    }
    runs.sort_by_key(|r| r.0.start);
    let mut out = String::with_capacity(svg.len());
    let mut at = 0;
    for (range, markup) in runs {
        out.push_str(&svg[at..range.start]);
        out.push_str(&markup);
        at = range.end;
    }
    out.push_str(&svg[at..]);
    Some(out)
}

/// A `<use>`'s attributes other than its reference and transform.
type Attrs = Vec<(String, String)>;

/// A glyph `<use>` among `glyphs` (outline data by id): its outline, its
/// placement, and what else it says, which the members of a run must share.
fn glyph<'g>(
    glyphs: &std::collections::HashMap<&str, &'g str>,
    n: roxmltree::Node<'_, '_>,
) -> Option<(&'g str, Affine, Attrs)> {
    if !n.has_tag_name("use") {
        return None;
    }
    let target = n
        .attribute(("http://www.w3.org/1999/xlink", "href"))
        .or_else(|| n.attribute("href"))?
        .strip_prefix('#')?;
    let d = *glyphs.get(target)?;
    let at = n
        .attribute("transform")
        .map_or(Some(Affine::IDENTITY), transform)?;
    let rest = n
        .attributes()
        .filter(|a| a.name() != "href" && a.name() != "transform")
        .map(|a| (a.name().to_string(), a.value().to_string()))
        .collect();
    Some((d, at, rest))
}

/// `s` safe inside a double-quoted attribute.
fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// The affine a `transform` attribute of one `matrix`, `translate` or `scale`
/// says — the three `hayro-svg` writes. `None` for anything else.
fn transform(value: &str) -> Option<Affine> {
    let (name, args) = value.trim().strip_suffix(')')?.split_once('(')?;
    let n: Vec<f64> = args
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    Some(match (name.trim(), n.as_slice()) {
        ("matrix", &[a, b, c, d, e, f]) => Affine::new([a, b, c, d, e, f]),
        ("translate", &[x]) => Affine::translate((x, 0.0)),
        ("translate", &[x, y]) => Affine::translate((x, y)),
        ("scale", &[s]) => Affine::scale(s),
        ("scale", &[x, y]) => Affine::scale_non_uniform(x, y),
        _ => return None,
    })
}
