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
