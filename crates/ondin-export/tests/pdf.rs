//! PDF export: our SVG converted, with live text embedded or outlined.

mod common;

use ondin_core::kurbo::Vec2;
use ondin_core::{
    Effect, EffectKind, ExportText, FeatureSetting, Operation, Resolved, Shadow, Tag, TextStyle,
    Transaction, keyed_by_position,
};
use ondin_export::pdf::{approximated, pdf_of, pdf_pages};

/// The fixture with its text node restyled.
fn styled(style: TextStyle) -> common::Fixture {
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            spans: None,
            style,
        }]))
        .expect("style the text");
    f.res = Resolved::rebuild(&f.doc);
    f
}

fn inter(weight: u16) -> TextStyle {
    TextStyle {
        font_family: "Inter".into(),
        font_size: 16.0,
        weight,
        ..Default::default()
    }
}

/// The PDF's bytes as text, for its uncompressed dictionaries.
fn pdf_text(f: &common::Fixture, text: ExportText) -> String {
    let out = pdf_of(&f.doc, &f.res, &[f.artboard], text).expect("a pdf");
    String::from_utf8_lossy(&out.pdf).into_owned()
}

/// Every `FlateDecode` stream in `pdf`, inflated.
fn inflated_streams(pdf: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = pdf;
    while let Some(at) = find(rest, b"stream\n") {
        let body = &rest[at + b"stream\n".len()..];
        let Some(end) = find(body, b"\nendstream") else {
            break;
        };
        if let Ok(bytes) = miniz_oxide::inflate::decompress_to_vec_zlib(&body[..end]) {
            out.push(String::from_utf8_lossy(&bytes).into_owned());
        }
        // Past `endstream`, whose own tail would match `stream\n` again.
        rest = &body[end + b"\nendstream".len()..];
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The `/W` array: the advance of every glyph in the embedded subset.
fn widths(pdf: &str) -> &str {
    let at = pdf.find("/W[").expect("a CID font's widths");
    &pdf[at..at + pdf[at..].find(']').expect("the array closes")]
}

/// Live text embeds a font program, and its `ToUnicode` map covers every
/// character, so a reader can select and search the words.
#[test]
fn live_text_embeds_its_font_and_maps_every_character() {
    let f = common::fixture();
    let out = pdf_of(&f.doc, &f.res, &[f.artboard], ExportText::Live).expect("a pdf");
    let text = String::from_utf8_lossy(&out.pdf);
    assert!(text.contains("/FontFile2"), "a TrueType font program");
    assert!(
        text.contains("+Inter"),
        "a subset of the face the layout used"
    );
    let cmap = inflated_streams(&out.pdf)
        .into_iter()
        .find(|s| s.contains("begincmap"))
        .expect("a ToUnicode map");
    for c in "Hi<there>".chars() {
        let hex = format!("{:04X}", c as u32);
        assert!(cmap.contains(&hex), "{c} ({hex}) is unmapped:\n{cmap}");
    }
}

/// Outlined text is paths: no font is embedded at all.
#[test]
fn outlined_text_embeds_no_font() {
    let f = common::fixture();
    let text = pdf_text(&f, ExportText::Outlines);
    assert!(text.starts_with("%PDF-"));
    for absent in ["/Font", "/ToUnicode"] {
        assert!(!text.contains(absent), "{absent} in an outlined PDF");
    }
}

/// A bold run embeds the bold instance of the variable face, not its default.
///
/// flip: `svg::run_attrs` writing every weight as 400 → fails at the inequality.
#[test]
fn a_bold_run_embeds_the_bold_instance() {
    let regular = pdf_text(&styled(inter(400)), ExportText::Live);
    let bold = pdf_text(&styled(inter(700)), ExportText::Live);
    assert_ne!(
        widths(&regular),
        widths(&bold),
        "the same glyphs at 700 must advance as bold Inter does"
    );
}

/// A planned file renders with its spec's text setting, for SVG as for PDF.
///
/// flip: `plan::render` writing SVG through `svg_of` → fails at the `<text` assertion.
#[test]
fn a_planned_file_follows_its_text_setting() {
    use ondin_core::{ExportFormat, ExportScale, ExportSpec};
    let f = common::fixture();
    let mut spec = ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0));
    spec.text = ExportText::Outlines;
    let file = ondin_export::PlannedFile {
        node: f.text,
        spec,
        path: "t.svg".into(),
        pixels: None,
        clamped: false,
    };
    let bytes = ondin_export::plan::render(&f.doc, &f.res, &file).expect("bytes");
    let svg = String::from_utf8(bytes).unwrap();
    assert!(!svg.contains("<text"), "{svg}");
    assert!(svg.contains("<path d="), "{svg}");
}

/// The page is the export's box at a point per world unit.
#[test]
fn the_page_is_a_point_per_world_unit() {
    let f = common::fixture();
    let text = pdf_text(&f, ExportText::Live);
    assert!(text.contains("/MediaBox[0 0 400 300]"), "{text}");
}

/// An effect becomes pixels in a PDF and the report says so; the card's
/// `approximated` agrees with the export's own list.
#[test]
fn an_effect_is_reported_as_pixels() {
    const NOTE: &str = "an effect, drawn as pixels";
    let mut f = common::fixture();
    let plain = pdf_of(&f.doc, &f.res, &[f.rect], ExportText::Live).expect("a pdf");
    assert!(!plain.approximated.iter().any(|s| s == NOTE));

    f.doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: f.rect,
            effects: keyed_by_position([Effect::new(EffectKind::DropShadow(Shadow {
                offset: Vec2::new(0.0, 4.0),
                blur: 8.0,
                ..Shadow::default()
            }))]),
        }]))
        .expect("a shadow");
    f.res = Resolved::rebuild(&f.doc);
    let shadowed = pdf_of(&f.doc, &f.res, &[f.rect], ExportText::Live).expect("a pdf");
    assert!(
        shadowed.approximated.iter().any(|s| s == NOTE),
        "{:?}",
        shadowed.approximated
    );
    assert_eq!(
        approximated(&f.doc, &f.res, &[f.rect], ExportText::Live),
        shadowed.approximated
    );
}

/// OpenType features are reported for embedded text, which `usvg` shapes without
/// them, and not for outlines, which are our own shaping.
#[test]
fn features_are_reported_for_live_text_only() {
    const NOTE: &str = "OpenType features, which embedded text does not apply";
    let f = styled(TextStyle {
        features: vec![FeatureSetting {
            tag: Tag::parse("ss01").unwrap(),
            value: 1,
        }],
        ..inter(400)
    });
    let live = approximated(&f.doc, &f.res, &[f.artboard], ExportText::Live);
    let outlined = approximated(&f.doc, &f.res, &[f.artboard], ExportText::Outlines);
    assert!(live.iter().any(|s| s == NOTE), "{live:?}");
    assert!(!outlined.iter().any(|s| s == NOTE), "{outlined:?}");
}

/// Two frames make one PDF of two pages, each at its own size, with a bookmark
/// per page; a one-page export carries no outline.
///
/// flip: `pdf_pages` drawing only the first page → fails at the page count.
#[test]
fn several_frames_make_one_pdf_of_several_pages() {
    let f = common::fixture();
    let pages = [
        ("Board".to_string(), vec![f.artboard]),
        ("Card".to_string(), vec![f.rect]),
    ];
    let out = pdf_pages(&f.doc, &f.res, &pages, ExportText::Live).expect("a pdf");
    let text = String::from_utf8_lossy(&out.pdf);
    let page_count = text.matches("/Type/Page").count() - text.matches("/Type/Pages").count();
    assert_eq!(page_count, 2, "{text}");
    assert!(text.contains("/MediaBox[0 0 400 300]"), "the frame's page");
    let rect = f.res.world_bounds(f.rect).unwrap();
    let card = format!("/MediaBox[0 0 {} {}]", rect.width(), rect.height());
    assert!(text.contains(&card), "the card's own page, {card}: {text}");
    assert!(text.contains("/Outlines"), "a bookmark per page");
    assert_eq!(text.matches("/Dest").count(), 2, "{text}");

    let one = pdf_text(&f, ExportText::Live);
    assert!(
        !one.contains("/Outlines"),
        "a one-page export has no outline"
    );
    assert!(pdf_pages(&f.doc, &f.res, &[], ExportText::Live).is_none());
}
