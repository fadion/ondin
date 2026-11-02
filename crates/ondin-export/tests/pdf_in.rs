//! PDF import (`pdf_in`): the header sniff, the glyph merge, and a round trip
//! through Ondin's own PDF export.

mod common;

use ondin_core::{IdSource, svg_in};
use ondin_export::pdf_in::{PdfInError, import, looks_like_pdf, merged_glyphs};

/// A PDF is known by `%PDF-` in its first kilobyte, after leading junk as readers
/// allow, and nothing else is.
#[test]
fn a_pdf_is_known_by_its_header() {
    assert!(looks_like_pdf(b"%PDF-1.7\n"));
    assert!(looks_like_pdf(b"\xef\xbb\xbf junk %PDF-1.4"));
    assert!(!looks_like_pdf(
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
    ));
    assert!(!looks_like_pdf(b""));
    let mut late = vec![b' '; 2048];
    late.extend_from_slice(b"%PDF-1.4");
    assert!(!looks_like_pdf(&late), "past the first kilobyte");
    assert_eq!(
        import(b"%PDF-1.4 not really".to_vec()).err(),
        Some(PdfInError::Unreadable)
    );
}

/// Glyph `<use>`s merge into one path per run of one paint on one line: a change
/// of fill or of baseline starts the next, a `<use>` of something not a glyph is
/// left alone, and the markup around the runs is kept as it was.
///
/// flip: `merged_glyphs` comparing lines without the baseline → fails at the
/// count, the second line joining the first.
#[test]
fn glyphs_merge_into_one_path_per_line_and_paint() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 100 100">
<defs><path id="g0" d="M0,0 L10,0 L10,10 Z"/><path id="g1" d="M0,0 L5,0 L5,5 Z"/><rect id="box" width="4" height="4"/></defs>
<rect x="1" y="1" width="2" height="2" fill="#123456"/>
<use xlink:href="#g0" transform="matrix(1 0 0 -1 10 20)" fill="#000000"/>
<use xlink:href="#g1" transform="matrix(1 0 0 -1 22 20)" fill="#000000"/>
<use xlink:href="#g0" transform="matrix(1 0 0 -1 30 20)" fill="#ff0000"/>
<use xlink:href="#g1" transform="matrix(1 0 0 -1 10 40)" fill="#ff0000"/>
<use xlink:href="#box" x="50" y="50"/>
</svg>"##;
    let out = merged_glyphs(svg).expect("parses");
    assert_eq!(out.matches(r#"data-name="Text""#).count(), 3, "{out}");
    assert!(
        out.contains(r##"<rect x="1" y="1" width="2" height="2" fill="#123456"/>"##),
        "the rest is kept: {out}"
    );
    assert!(
        out.contains(r##"<use xlink:href="#box""##),
        "a non-glyph use stays: {out}"
    );
    assert!(!out.contains("#g0\""), "no glyph use is left: {out}");
}

/// Stroked glyphs under a half scale merge with their stroke halved, as the
/// outline leaves the `<use>`'s units; under a squash no one width redraws the
/// stroke, so those glyphs stay `<use>`s. flip: copy the run's attributes as
/// they are in `merged_glyphs` → fails at the count, the squashed glyph merged
/// too, its width and the others' left in the `<use>`'s units.
#[test]
fn a_stroked_runs_width_follows_its_outline_out_of_the_use() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 100 100">
<defs><path id="g0" d="M0,0 L10,0 L10,10 Z"/></defs>
<use xlink:href="#g0" transform="matrix(0.5 0 0 -0.5 10 20)" fill="none" stroke="#000000" stroke-width="4" stroke-dasharray="2 6"/>
<use xlink:href="#g0" transform="matrix(0.5 0 0 -0.5 20 20)" fill="none" stroke="#000000" stroke-width="4" stroke-dasharray="2 6"/>
<use xlink:href="#g0" transform="matrix(1 0 0 -0.5 10 60)" fill="none" stroke="#000000" stroke-width="4"/>
</svg>"##;
    let out = merged_glyphs(svg).expect("parses");
    assert_eq!(out.matches(r#"data-name="Text""#).count(), 1, "{out}");
    assert!(
        out.contains(r#"stroke-width="2""#) && out.contains(r#"stroke-dasharray="1 3""#),
        "{out}"
    );
    assert!(
        out.contains(r#"transform="matrix(1 0 0 -0.5 10 60)""#),
        "the squashed glyph stays a use: {out}"
    );
}

/// A shading clipped to its `/BBox` inside its pattern stays a pattern, for
/// `svg_in` to report, rather than a gradient painting the whole shape.
/// flip: drop the `clip-path` test in `shading` → fails at the pattern.
#[test]
fn a_clipped_shading_pattern_is_not_unwrapped() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
<rect width="100" height="100" fill="url(#v0)"/>
<defs>
<clipPath id="c0"><path d="M0,0 L50,0 L50,50 L0,50 Z"/></clipPath>
<pattern id="v0" patternUnits="userSpaceOnUse" width="100" height="100"><rect width="100" height="100" clip-path="url(#c0)" fill="url(#n0)"/></pattern>
<linearGradient id="n0" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="50" y2="0"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient>
</defs>
</svg>"##;
    let out = ondin_export::pdf_in::unwrapped_shadings(svg).expect("parses");
    assert!(out.contains(r#"<pattern id="v0""#), "{out}");
}

/// A PDF of `objects` (each the text after `N 0 obj`), numbered from 1, the
/// first the catalog, with a cross-reference table at the right offsets.
fn pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend(o);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn stream(dict: &str, body: &str) -> Vec<u8> {
    format!(
        "<< {dict} /Length {} >>\nstream\n{body}\nendstream",
        body.len()
    )
    .into_bytes()
}

/// A 100-point page filling an `n`-point square with a function-based shading,
/// which `hayro-svg` rasterises at the square's size.
fn shaded(n: u32) -> Vec<u8> {
    pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Pattern << /P0 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        stream("", &format!("/Pattern cs /P0 scn 0 0 {n} {n} re f")),
        b"<< /PatternType 2 /Shading 6 0 R >>".to_vec(),
        b"<< /ShadingType 1 /ColorSpace /DeviceGray /Function 7 0 R >>".to_vec(),
        stream(
            "/FunctionType 4 /Domain [0 1 0 1] /Range [0 1]",
            "{ pop pop 0.5 }",
        ),
    ])
}

/// A page whose shading would rasterise past the pixel budget is left out and
/// counted, in bounded time, where drawing it would ask for gigabytes; one
/// within it is drawn. flip: let every page through the filter in `import` →
/// fails at `TooLarge`, the page drawn with a 400-megapixel picture.
#[test]
fn a_shading_past_the_pixel_budget_is_left_out() {
    let small = import(shaded(500)).expect("reads");
    assert_eq!((small.pages.len(), small.too_large), (1, 0));
    assert!(small.pages[0].svg.contains("<image"), "fixture: rasterised");
    let started = std::time::Instant::now();
    assert_eq!(
        import(shaded(20_000)).err(),
        Some(PdfInError::TooLarge),
        "its only page left out"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}

/// A 100-point page stroking an "H" of Helvetica one point high with a
/// function-based shading, the line `width` points wide.
fn stroked_glyph(width: u32) -> Vec<u8> {
    pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Font << /F 8 0 R >> /Pattern << /P0 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        stream("", &format!("/Pattern CS /P0 SCN {width} w BT /F 1 Tf 1 Tr 50 50 Td (H) Tj ET")),
        b"<< /PatternType 2 /Shading 6 0 R >>".to_vec(),
        b"<< /ShadingType 1 /ColorSpace /DeviceGray /Function 7 0 R >>".to_vec(),
        stream(
            "/FunctionType 4 /Domain [0 1 0 1] /Range [0 1]",
            "{ pop pop 0.5 }",
        ),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ])
}

/// A stroked glyph's line widens its box in the page's units, not the
/// outline's 1,000 an em: a line wide enough to take its shading past the
/// pixel budget leaves the page out, a thin one is drawn. flip: inflate the
/// outline's box before placing it in `Cost::draw_glyph_run` → fails at
/// `TooLarge`, the wide stroke read.
#[test]
fn a_stroked_glyph_is_budgeted_at_its_lines_width() {
    let thin = import(stroked_glyph(10)).expect("reads");
    assert_eq!((thin.pages.len(), thin.too_large), (1, 0));
    assert!(thin.pages[0].svg.contains("<image"), "fixture: rasterised");
    // A box about 8,200 points square: 67 megapixels, past the 64.
    assert_eq!(
        import(stroked_glyph(4_100)).err(),
        Some(PdfInError::TooLarge),
        "its only page left out"
    );
}

/// A PDF of one page per side in `sides`, each drawing a picture that declares
/// itself `side` pixels square and holds one byte, so the budget reads its
/// declared size and drawing it costs nothing.
fn pictured(sides: &[u32]) -> Vec<u8> {
    let n = sides.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 3 * i)).collect();
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes(),
    ];
    for (i, side) in sides.iter().enumerate() {
        let at = 3 + 3 * i;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /Im {} 0 R >> >> /Contents {} 0 R >>",
            at + 2, at + 1
        ).into_bytes());
        objects.push(stream("", "q 100 0 0 100 0 0 cm /Im Do Q"));
        objects.push(stream(
            &format!("/Type /XObject /Subtype /Image /Width {side} /Height {side} /ColorSpace /DeviceGray /BitsPerComponent 8"),
            "0",
        ));
    }
    pdf(&objects)
}

/// A page whose picture declares more pixels than the page budget is left out;
/// a small one is read. flip: add nothing in `Cost::draw_image` → fails at
/// `TooLarge`.
#[test]
fn a_picture_past_the_pixel_budget_is_left_out() {
    let small = import(pictured(&[100])).expect("reads");
    assert_eq!((small.pages.len(), small.too_large), (1, 0));
    assert_eq!(
        import(pictured(&[20_000])).err(),
        Some(PdfInError::TooLarge),
        "400 megapixels, past the page's 256"
    );
}

/// Pages each within the page budget are read until together they pass the
/// document's, four pages' worth; the rest are left out and counted. flip:
/// drop the document sums from the filter in `import` → fails at the counts.
#[test]
fn pages_past_the_document_budget_are_left_out() {
    // 225 megapixels a page: four fit in the document's 1,024, a fifth does not.
    let got = import(pictured(&[15_000; 5])).expect("reads");
    assert_eq!((got.pages.len(), got.too_large), (4, 1));
}

/// A page whose content fills with tiling patterns nested `depth` deep, the
/// innermost filling a 300-point square with a function-based shading.
fn nested_tiles(depth: usize) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Pattern << /P 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        stream("", "/Pattern cs /P scn 0 0 100 100 re f"),
    ];
    for i in 0..depth {
        let next = 6 + i;
        let fill = if i + 1 == depth { 300 } else { 100 };
        objects.push(stream(
            &format!(
                "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 100 100] /XStep 100 /YStep 100 /Resources << /Pattern << /P {next} 0 R >> >>"
            ),
            &format!("/Pattern cs /P scn 0 0 {fill} {fill} re f"),
        ));
    }
    let shading = 6 + depth;
    objects.push(format!("<< /PatternType 2 /Shading {} 0 R >>", shading + 1).into_bytes());
    objects.push(
        format!(
            "<< /ShadingType 1 /ColorSpace /DeviceGray /Function {} 0 R >>",
            shading + 2
        )
        .into_bytes(),
    );
    objects.push(stream(
        "/FunctionType 4 /Domain [0 1 0 1] /Range [0 1]",
        "{ pop pop 0.5 }",
    ));
    pdf(&objects)
}

/// Patterns nested past the depth the cost walk follows leave their page out,
/// since `hayro-svg` would still draw what the walk did not count; nested
/// within it, the page reads. flip: skip the subtree without marking it in
/// `Cost::nested` → fails at `TooLarge`, the deep page read.
#[test]
fn patterns_nested_past_the_walk_leave_their_page_out() {
    let within = import(nested_tiles(8)).expect("reads");
    assert_eq!((within.pages.len(), within.too_large), (1, 0));
    assert_eq!(
        import(nested_tiles(9)).err(),
        Some(PdfInError::TooLarge),
        "its only page left out"
    );
}

/// A pattern holding one gradient-filled shape, as `hayro-svg` draws a shading,
/// becomes that gradient under the pattern's id, the pattern's transform folded in;
/// a pattern of tiles stays, for `svg_in` to report.
///
/// flip: `shading` leaving out `patternTransform` → fails at the matrix.
#[test]
fn a_shading_pattern_becomes_its_gradient() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
<rect width="10" height="10" fill="url(#v0)" transform="matrix(10 0 0 10 0 0)"/>
<rect width="100" height="100" fill="url(#t0)"/>
<defs>
<pattern id="v0" patternUnits="userSpaceOnUse" width="5" height="5" patternTransform="matrix(0.5 0 0 0.5 0 0)"><rect width="20" height="20" fill="url(#n0)"/></pattern>
<pattern id="t0" patternUnits="userSpaceOnUse" width="8" height="8"><rect width="4" height="4" fill="#000"/><rect x="4" y="4" width="4" height="4" fill="#000"/></pattern>
<linearGradient id="n0" gradientUnits="userSpaceOnUse" gradientTransform="matrix(20 0 0 20 0 0)" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient>
</defs>
</svg>"##;
    let out = ondin_export::pdf_in::unwrapped_shadings(svg).expect("parses");
    assert!(
        out.contains(r##"<linearGradient id="v0" gradientTransform="matrix(10 0 0 10 0 0)" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient>"##),
        "{out}"
    );
    assert!(out.contains(r#"<pattern id="t0""#), "the tiles stay: {out}");

    let mut ids = IdSource::new(0x9E);
    let root = ids.mint();
    let doc = ondin_core::Document::new(root);
    let read = svg_in::import(&out, &mut ids, doc.first_page(), 0, None).expect("reads");
    let gradients = read
        .tx
        .0
        .iter()
        .filter(|op| {
            matches!(op, ondin_core::Operation::SetFills { fills, .. }
                if fills.iter().any(|f| matches!(f.brush.literal(), Some(ondin_core::Brush::Gradient(_)))))
        })
        .count();
    assert_eq!(gradients, 1, "the shading reads as a gradient fill");
    assert_eq!(
        read.skipped,
        ["pattern (image fill)"],
        "the tiles are reported"
    );
}

/// A page Ondin exported reads back with its text as one layer per line, not a
/// layer per glyph, and the rest of the page as layers.
#[test]
fn a_page_ondin_exported_reads_back_with_a_layer_per_line() {
    let f = common::fixture();
    let pdf = ondin_export::pdf::pdf_of(&f.doc, &f.res, &[f.artboard], Default::default())
        .expect("a PDF");
    let read = import(pdf.pdf).expect("reads");
    assert_eq!((read.pages.len(), read.left_out), (1, 0));
    let mut ids = IdSource::new(0x9D);
    let root = ids.mint();
    let doc = ondin_core::Document::new(root);
    let page = svg_in::import(&read.pages[0].svg, &mut ids, doc.first_page(), 0, None)
        .expect("the page reads as SVG");
    let text_layers = page
        .tx
        .0
        .iter()
        .filter(|op| {
            matches!(op, ondin_core::Operation::CreateNode { name: Some(n), .. } if n == "Text")
        })
        .count();
    assert_eq!(text_layers, 1, "one line of type, one layer");
    assert!(page.shapes > text_layers, "and the shapes beside it");
}
