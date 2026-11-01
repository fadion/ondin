//! PDF import: each page drawn as SVG by `hayro-svg`, for `ondin_core::svg_in` to
//! read as it reads a pasted SVG. One reader for both formats, so a PDF's fills,
//! strokes, gradients, clips and pictures arrive by the paths SVG import already
//! tests. Text arrives as the glyphs' outlines: a PDF places glyphs, not
//! paragraphs. `hayro-svg` draws each glyph as a `<use>` of its outline, which
//! would be a layer per letter, so [`merged_glyphs`] joins each line's into one
//! path first. A shading arrives wrapped in a `<pattern>`, which [`unwrapped_shadings`]
//! turns back into the gradient it holds.

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
    /// Pages left out because drawing them would rasterise more shading or
    /// decode more picture than [`MAX_SHADING_PIXELS`] or [`MAX_PICTURE_PIXELS`].
    pub too_large: usize,
}

/// The most pixels one page's rasterised shadings may cover. `hayro-svg` draws a
/// function-based or mesh shading, and a gradient SVG cannot express, as a
/// picture the size of the filled shape's box at one pixel per point, with no
/// cap, so one shape far off the page asks for any size. Sixty-four million is a
/// full-bleed mesh on an 8,000-point square artboard (Illustrator's largest is
/// about 16,000), or several meshes on ordinary pages: real artwork fits, a
/// crafted coordinate does not.
pub const MAX_SHADING_PIXELS: f64 = 64e6;

/// The most picture pixels one page may decode: room for ten 24-megapixel
/// photographs, refusing a picture that declares absurd dimensions.
pub const MAX_PICTURE_PIXELS: f64 = 256e6;

/// Why a PDF could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfInError {
    /// Encrypted, and the empty password does not open it.
    Encrypted,
    /// Not a PDF this reader can parse, or one with no pages.
    Unreadable,
    /// Every page it would read would rasterise or decode past the budget
    /// ([`MAX_SHADING_PIXELS`], [`MAX_PICTURE_PIXELS`]).
    TooLarge,
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
    let mut too_large = 0;
    let pages = all
        .iter()
        .take(MAX_PAGES)
        .filter(|page| {
            let cost = page_cost(page, &settings);
            let fits = cost.shading <= MAX_SHADING_PIXELS && cost.pictures <= MAX_PICTURE_PIXELS;
            too_large += usize::from(!fits);
            fits
        })
        .map(|page| {
            let (w, h) = page.render_dimensions();
            let svg = hayro_svg::convert(page, &cache, &settings, &render);
            let svg = unwrapped_shadings(&svg).unwrap_or(svg);
            PdfPage {
                svg: merged_glyphs(&svg).unwrap_or(svg),
                size: Size::new(f64::from(w), f64::from(h)),
            }
        })
        .collect::<Vec<_>>();
    if pages.is_empty() && too_large > 0 {
        return Err(PdfInError::TooLarge);
    }
    Ok(PdfImport {
        pages,
        left_out: all.len().saturating_sub(MAX_PAGES),
        too_large,
    })
}

/// What drawing a page would rasterise and decode, in pixels.
#[derive(Default)]
struct Cost {
    shading: f64,
    pictures: f64,
    /// Patterns, masks and glyphs being walked inside one another.
    depth: u32,
}

/// What `hayro-svg` would allocate to draw `page`: the page interpreted onto a
/// device that only adds up each rasterised shading's box and each picture's
/// size, without sampling either.
fn page_cost(
    page: &hayro_svg::hayro_syntax::page::Page<'_>,
    settings: &hayro_svg::hayro_interpret::InterpreterSettings,
) -> Cost {
    use hayro_svg::hayro_interpret::{Context, InterpreterCache, TransformExt, interpret_page};
    let cache = InterpreterCache::new();
    let (w, h) = page.render_dimensions();
    let mut ctx = Context::new(
        page.initial_transform(true).to_kurbo(),
        ondin_core::kurbo::Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
        &cache,
        page.xref(),
        settings.clone(),
    );
    let mut cost = Cost::default();
    interpret_page(page, &mut ctx, &mut cost);
    cost
}

/// How far patterns, masks and Type 3 glyphs are followed inside one another.
const MAX_COST_DEPTH: u32 = 8;

impl Cost {
    /// Add what `paint` filling (or stroking, `stroke` wide) a shape whose box is
    /// `bbox` under `transform` rasterises, as `hayro-svg` sizes it.
    fn paint<'a>(
        &mut self,
        paint: &hayro_svg::hayro_interpret::Paint<'a>,
        bbox: ondin_core::kurbo::Rect,
        transform: Affine,
        stroke: Option<f32>,
    ) {
        use hayro_svg::hayro_interpret::Paint;
        use hayro_svg::hayro_interpret::encode::EncodedShadingType;
        use hayro_svg::hayro_interpret::pattern::Pattern;
        use hayro_svg::hayro_interpret::shading::ShadingType;
        use ondin_core::kurbo::Shape;
        let Paint::Pattern(p) = paint else {
            return;
        };
        match p.as_ref() {
            Pattern::Shading(s) => {
                let w = f64::from(stroke.unwrap_or(0.0));
                let bbox = (transform * bbox.inflate(w, w).to_path(0.0)).bounding_box();
                let area = |r: ondin_core::kurbo::Rect| {
                    r.width().abs().max(1.0).ceil() * r.height().abs().max(1.0).ceil()
                };
                let sampled = |tris: &[hayro_svg::hayro_interpret::shading::Triangle]| -> f64 {
                    // Sampled at whole device pixels, clamped to `u16` as hayro does.
                    let clamp = |v: f64| v.clamp(0.0, f64::from(u16::MAX));
                    tris.iter()
                        .map(|t| {
                            let b = (s.matrix * t.bounding_box().to_path(0.0)).bounding_box();
                            (clamp(b.x1) - clamp(b.x0)) * (clamp(b.y1) - clamp(b.y0))
                        })
                        .sum()
                };
                // A mesh is sampled into device pixels, triangle by triangle,
                // before it is drawn at all.
                let mesh = match s.shading.shading_type.as_ref() {
                    ShadingType::TriangleMesh { triangles, .. } => Some(sampled(triangles)),
                    ShadingType::CoonsPatchMesh { patches, .. } => {
                        let mut tris = Vec::new();
                        patches.iter().for_each(|p| p.to_triangles(&mut tris));
                        Some(sampled(&tris))
                    }
                    ShadingType::TensorProductPatchMesh { patches, .. } => {
                        let mut tris = Vec::new();
                        patches.iter().for_each(|p| p.to_triangles(&mut tris));
                        Some(sampled(&tris))
                    }
                    ShadingType::Dummy => return,
                    _ => None,
                };
                if let Some(samples) = mesh {
                    self.shading += samples + area(bbox);
                    return;
                }
                let encoded = s.encode();
                let native = matches!(
                    &encoded.shading_type,
                    EncodedShadingType::RadialAxial(g) if g.as_svg_gradient(&encoded, bbox, 0.01).is_some()
                );
                if !native {
                    self.shading += area(bbox);
                }
            }
            Pattern::Tiling(t) => {
                if self.depth < MAX_COST_DEPTH {
                    self.depth += 1;
                    t.interpret(
                        self,
                        Affine::translate((-t.bbox.x0, -t.bbox.y0)),
                        stroke.is_some(),
                    );
                    self.depth -= 1;
                }
            }
        }
    }
}

impl<'a> hayro_svg::hayro_interpret::Device<'a> for Cost {
    fn draw_path(
        &mut self,
        path: &BezPath,
        props: hayro_svg::hayro_interpret::DrawProps<'a>,
        mode: &hayro_svg::hayro_interpret::DrawMode,
    ) {
        use hayro_svg::hayro_interpret::DrawMode;
        use ondin_core::kurbo::Shape;
        let bbox = path.bounding_box();
        match mode {
            DrawMode::Fill(_) => self.paint(&props.paint, bbox, props.transform, None),
            DrawMode::Stroke(s) => {
                self.paint(&props.paint, bbox, props.transform, Some(s.line_width));
            }
            DrawMode::FillAndStroke(_, s) => {
                self.paint(&props.paint, bbox, props.transform, None);
                self.paint(&props.paint, bbox, props.transform, Some(s.line_width));
            }
            DrawMode::Invisible => {}
        }
        if let Some(mask) = props.soft_mask {
            self.mask(&mask);
        }
    }

    fn push_clip_path(&mut self, _: &hayro_svg::hayro_interpret::ClipPath) {}

    fn push_transparency_group(
        &mut self,
        _: f32,
        mask: Option<hayro_svg::hayro_interpret::SoftMask<'a>>,
        _: hayro_svg::hayro_interpret::BlendMode,
    ) {
        if let Some(mask) = mask {
            self.mask(&mask);
        }
    }

    fn draw_glyph_run(
        &mut self,
        run: &hayro_svg::hayro_interpret::font::GlyphRun<'_, 'a>,
        props: hayro_svg::hayro_interpret::DrawProps<'a>,
        mode: &hayro_svg::hayro_interpret::DrawMode,
    ) {
        use hayro_svg::hayro_interpret::DrawMode;
        use hayro_svg::hayro_interpret::font::Glyph;
        use ondin_core::kurbo::Shape;
        let stroke = match mode {
            DrawMode::Invisible => return,
            DrawMode::Fill(_) => None,
            DrawMode::Stroke(s) | DrawMode::FillAndStroke(_, s) => Some(s.line_width),
        };
        for glyph in run.glyphs() {
            match &**glyph {
                Glyph::Outline(g) => {
                    let at = props.transform * glyph.transform();
                    self.paint(&props.paint, g.outline().bounding_box(), at, stroke);
                }
                Glyph::Type3(g) => {
                    if self.depth < MAX_COST_DEPTH {
                        self.depth += 1;
                        g.interpret(self, props.transform, glyph.transform(), &props.paint);
                        self.depth -= 1;
                    }
                }
            }
        }
    }

    fn draw_image(
        &mut self,
        image: hayro_svg::hayro_interpret::Image<'a, '_>,
        props: hayro_svg::hayro_interpret::ImageDrawProps<'a>,
    ) {
        self.pictures += f64::from(image.width()) * f64::from(image.height());
        if let Some(mask) = props.soft_mask {
            self.mask(&mask);
        }
    }

    fn pop_clip(&mut self) {}

    fn pop_transparency_group(&mut self) {}
}

impl Cost {
    /// Add what a soft mask's own content draws.
    fn mask(&mut self, mask: &hayro_svg::hayro_interpret::SoftMask<'_>) {
        if self.depth < MAX_COST_DEPTH {
            self.depth += 1;
            mask.interpret(self);
            self.depth -= 1;
        }
    }
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
            // A stroke's lengths are in the `<use>`'s units; the merged outline
            // leaves them, so they scale with it, which only a similarity allows.
            let Some(attrs) = stroke_scaled(&first.3, first.2) else {
                run.clear();
                return;
            };
            let mut path = BezPath::new();
            for (_, d, at, _) in run.iter() {
                if let Ok(outline) = BezPath::from_svg(d) {
                    path.extend((*at * outline).iter());
                }
            }
            let attrs: String = attrs
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
    Some(spliced(svg, runs))
}

/// `svg` with each `<pattern>` that holds only one shape painted by a gradient —
/// how `hayro-svg` draws a PDF shading — replaced by that gradient under the
/// pattern's id, the pattern's and the shape's transforms folded into its
/// `gradientTransform`. `svg_in` reads gradients and not patterns. `None` when the
/// markup does not parse.
///
/// The tile is dropped: `hayro-svg` sizes it to cover the shape it fills.
pub fn unwrapped_shadings(svg: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(svg).ok()?;
    let gradients: std::collections::HashMap<&str, roxmltree::Node<'_, '_>> = doc
        .descendants()
        .filter(|n| n.has_tag_name("linearGradient") || n.has_tag_name("radialGradient"))
        .filter_map(|n| Some((n.attribute("id")?, n)))
        .collect();
    let swaps = doc
        .descendants()
        .filter(|n| n.has_tag_name("pattern"))
        .filter_map(|p| Some((p.range(), shading(svg, &gradients, p)?)))
        .collect();
    Some(spliced(svg, swaps))
}

/// The gradient `pattern` stands for, as markup under the pattern's id, when it is
/// one unclipped shape filled by a user-space gradient in user-space units.
fn shading(
    svg: &str,
    gradients: &std::collections::HashMap<&str, roxmltree::Node<'_, '_>>,
    pattern: roxmltree::Node<'_, '_>,
) -> Option<String> {
    let user_space = |n: roxmltree::Node<'_, '_>, units| {
        n.attribute(units).unwrap_or("objectBoundingBox") == "userSpaceOnUse"
    };
    if !user_space(pattern, "patternUnits")
        || pattern
            .attribute("patternContentUnits")
            .is_some_and(|u| u != "userSpaceOnUse")
        || pattern.has_attribute("viewBox")
    {
        return None;
    }
    let mut shapes = pattern.children().filter(|c| c.is_element());
    let (Some(shape), None) = (shapes.next(), shapes.next()) else {
        return None;
    };
    // A shading's `/BBox` clips it inside the pattern; the bare gradient would
    // paint the whole shape, so a clipped one stays a pattern, reported.
    if shape.has_attribute("clip-path") {
        return None;
    }
    let target = shape
        .attribute("fill")?
        .trim()
        .strip_prefix("url(#")?
        .strip_suffix(')')?;
    let gradient = *gradients.get(target)?;
    if !user_space(gradient, "gradientUnits") || gradient.attributes().any(|a| a.name() == "href") {
        return None;
    }
    let at = |n: roxmltree::Node<'_, '_>, name| {
        n.attribute(name).map_or(Some(Affine::IDENTITY), transform)
    };
    let [a, b, c, d, e, f] = (at(pattern, "patternTransform")?
        * at(shape, "transform")?
        * at(gradient, "gradientTransform")?)
    .as_coeffs();
    let tag = gradient.tag_name().name();
    let attrs: String = gradient
        .attributes()
        .filter(|a| a.name() != "id" && a.name() != "gradientTransform")
        .map(|a| format!(r#" {}="{}""#, a.name(), escape_attr(a.value())))
        .collect();
    let stops = match (gradient.first_child(), gradient.last_child()) {
        (Some(first), Some(last)) => &svg[first.range().start..last.range().end],
        _ => "",
    };
    Some(format!(
        r#"<{tag} id="{}" gradientTransform="matrix({a} {b} {c} {d} {e} {f})"{attrs}>{stops}</{tag}>"#,
        escape_attr(pattern.attribute("id")?)
    ))
}

/// `svg` with each range replaced by its markup, the rest kept byte for byte. The
/// ranges do not overlap.
fn spliced(svg: &str, mut swaps: Vec<(std::ops::Range<usize>, String)>) -> String {
    swaps.sort_by_key(|r| r.0.start);
    let mut out = String::with_capacity(svg.len());
    let mut at = 0;
    for (range, markup) in swaps {
        out.push_str(&svg[at..range.start]);
        out.push_str(&markup);
        at = range.end;
    }
    out.push_str(&svg[at..]);
    out
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

/// A run's attributes for its merged path, its outlines moved out of `at`: a
/// stroked run's width and dashes scaled by `at`'s scale. `None` for a stroked
/// run under a transform that is not a similarity, whose stroke no one width
/// redraws, so its `<use>`s stay as they are.
fn stroke_scaled(attrs: &Attrs, at: Affine) -> Option<Attrs> {
    let stroked = attrs
        .iter()
        .any(|(k, v)| k == "stroke" && v.trim() != "none");
    if !stroked {
        return Some(attrs.clone());
    }
    let [a, b, c, d, _, _] = at.as_coeffs();
    let s = (a * d - b * c).abs().sqrt();
    let near = |x: f64, y: f64| (x - y).abs() <= 1e-6 * s.max(1.0);
    let similar = (near(a, d) && near(b, -c)) || (near(a, -d) && near(b, c));
    if !similar || s == 0.0 {
        return None;
    }
    let scaled = |v: &str| {
        v.split(|ch: char| ch == ',' || ch.is_whitespace())
            .filter(|p| !p.is_empty())
            .map(|p| p.parse::<f64>().map(|x| (x * s).to_string()))
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| parts.join(" "))
            .ok()
    };
    let mut out = Attrs::new();
    for (k, v) in attrs {
        let v = match k.as_str() {
            "stroke-width" | "stroke-dasharray" | "stroke-dashoffset" => scaled(v)?,
            _ => v.clone(),
        };
        out.push((k.clone(), v));
    }
    if !attrs.iter().any(|(k, _)| k == "stroke-width") {
        out.push(("stroke-width".into(), s.to_string()));
    }
    Some(out)
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
