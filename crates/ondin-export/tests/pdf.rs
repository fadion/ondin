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

/// A blend mode reaches the PDF as its own graphics state, not pixels, and adds
/// nothing to the report.
#[test]
fn a_blend_mode_is_a_pdf_blend_mode() {
    let mut f = common::fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetBlend {
            id: f.rect,
            blend: ondin_core::BlendMode::Multiply,
        }]))
        .expect("a blend");
    f.res = Resolved::rebuild(&f.doc);
    let out = pdf_of(&f.doc, &f.res, &[f.artboard], ExportText::Live).expect("a pdf");
    let text = String::from_utf8_lossy(&out.pdf).into_owned();
    let streams = inflated_streams(&out.pdf).join("\n");
    assert!(
        text.contains("/BM/Multiply") || streams.contains("/BM/Multiply"),
        "no Multiply graphics state"
    );
    assert!(out.approximated.is_empty(), "{:?}", out.approximated);
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

/// A frame with an effect says its contents go to pixels with it, and live
/// text's decorations say they lose their style; outlined text's do not.
///
/// flip: `approximations` without the decoration note → fails at "live".
#[test]
fn a_rasterized_frame_and_decorations_are_reported() {
    let mut f = styled(TextStyle {
        underline: Some(Default::default()),
        ..inter(400)
    });
    let live = approximated(&f.doc, &f.res, &[f.artboard], ExportText::Live);
    let outlined = approximated(&f.doc, &f.res, &[f.artboard], ExportText::Outlines);
    const DECO: &str = "underlines and strikethroughs, drawn as the font's plain line";
    assert!(live.iter().any(|s| s == DECO), "live: {live:?}");
    assert!(!outlined.iter().any(|s| s == DECO), "{outlined:?}");

    f.doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: f.artboard,
            effects: keyed_by_position([Effect::new(EffectKind::DropShadow(Shadow::default()))]),
        }]))
        .expect("a shadow on the frame");
    f.res = Resolved::rebuild(&f.doc);
    let notes = approximated(&f.doc, &f.res, &[f.artboard], ExportText::Live);
    assert!(
        notes
            .iter()
            .any(|s| s.starts_with("a frame or group with an effect")),
        "{notes:?}"
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

/// A linked picture is never read: the PDF of a fill linked to a file on disk is
/// the PDF of one linked to nothing, so a received document cannot make the export
/// open a path, or an SMB share, of its choosing.
///
/// flip: `pdf_pages` keeping usvg's default `resolve_string` → the two PDFs differ.
#[test]
fn a_linked_picture_is_never_read() {
    use ondin_core::peniko::Brush;
    use ondin_core::{Fill, ImageBrush, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource};
    let dir = std::env::temp_dir().join(format!("ondin-pdf-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let on_disk = dir.join("picture.png");
    image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 30, 30, 255]))
        .save(&on_disk)
        .unwrap();
    let linked_to = |path: String| {
        let mut f = common::fixture();
        let id = ImageId("linked".into());
        f.doc
            .apply(&Transaction(vec![
                Operation::AddImage {
                    id: id.clone(),
                    entry: ImageEntry {
                        source: ImageSource::Linked(path),
                        format: ImageFormat::Png,
                        width: 64,
                        height: 64,
                    },
                },
                Operation::SetFills {
                    id: f.rect,
                    fills: keyed_by_position([Fill {
                        brush: Brush::Image(ImageBrush {
                            image: ImageRef::new(id),
                            sampler: Default::default(),
                        })
                        .into(),
                        visible: true,
                    }]),
                },
            ]))
            .expect("link the picture");
        f.res = Resolved::rebuild(&f.doc);
        pdf_of(&f.doc, &f.res, &[f.artboard], ExportText::Live)
            .expect("a pdf")
            .pdf
    };
    let existing = linked_to(on_disk.to_string_lossy().into_owned());
    let missing = linked_to(dir.join("absent.png").to_string_lossy().into_owned());
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(existing, missing, "the linked file was read into the PDF");
}

/// Text in a family the page's fonts do not hold is drawn in Inter, the face the
/// layout fell back to, and noted; usvg's own fallback would leave it out.
///
/// flip: `select_font` returning `named(font, db)` on a miss → no font program.
#[test]
fn a_missing_family_is_drawn_in_inter_and_noted() {
    let f = styled(TextStyle {
        font_family: "No Such Family Xyz".into(),
        ..inter(400)
    });
    let out = pdf_of(&f.doc, &f.res, &[f.artboard], ExportText::Live).expect("a pdf");
    let text = String::from_utf8_lossy(&out.pdf);
    assert!(text.contains("/FontFile2"), "the text has a font program");
    let cmap = inflated_streams(&out.pdf)
        .into_iter()
        .find(|s| s.contains("begincmap"))
        .expect("a ToUnicode map");
    assert!(cmap.contains(&format!("{:04X}", 'H' as u32)), "{cmap}");
    assert_eq!(
        out.approximated,
        vec!["No Such Family Xyz, which is not available, drawn in Inter".to_string()]
    );
}

/// A loaded family whose file has none of the text's characters, so Inter drew
/// all of it, is not reported missing by an export on a worker thread, whose
/// engine holds only the faces the layout drew from.
///
/// flip: `approximations` asking `text::is_family_available` instead of
/// `TextLayout::absent` → fails at "no note".
#[test]
fn a_loaded_family_that_drew_nothing_is_not_reported_missing() {
    let emoji = include_bytes!("../../ondin-app/assets/fonts/NotoEmoji.ttf").to_vec();
    assert!(ondin_core::text::register_fonts_as(emoji, "Emoji Only Pdf"));
    let mut f = styled(TextStyle {
        font_family: "Emoji Only Pdf".into(),
        ..inter(400)
    });
    f.doc
        .apply(&Transaction(vec![Operation::SetText {
            id: f.text,
            content: "Hamburg".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]))
        .expect("letters the emoji font lacks");
    f.res = Resolved::rebuild(&f.doc);
    let notes = std::thread::spawn(move || {
        ondin_core::text::register_font_data(&f.res.text_fonts());
        assert!(
            !ondin_core::text::is_family_available("Emoji Only Pdf"),
            "fixture: the worker never saw the family"
        );
        pdf_of(&f.doc, &f.res, &[f.artboard], ExportText::Live)
            .expect("a pdf")
            .approximated
    })
    .join()
    .expect("the worker");
    assert!(notes.is_empty(), "no note: {notes:?}");
}

/// `font` with its family name records (name ids 1 and 16) shortened to `name`,
/// which must be no longer than each: two files then give themselves one family,
/// as one family's subset files do.
fn renamed(mut font: Vec<u8>, name: &str) -> Vec<u8> {
    let u16_at = |b: &[u8], at: usize| usize::from(u16::from_be_bytes([b[at], b[at + 1]]));
    let tables = u16_at(&font, 4);
    let table = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&rec| &font[rec..rec + 4] == b"name")
        .map(|rec| u32::from_be_bytes(font[rec + 8..rec + 12].try_into().expect("4 bytes")))
        .expect("a name table") as usize;
    let (count, strings) = (u16_at(&font, table + 2), table + u16_at(&font, table + 4));
    for rec in (0..count).map(|i| table + 6 + 12 * i) {
        let (platform, id) = (u16_at(&font, rec), u16_at(&font, rec + 6));
        if id != 1 && id != 16 {
            continue;
        }
        let bytes: Vec<u8> = if platform == 1 {
            name.bytes().collect()
        } else {
            name.encode_utf16().flat_map(u16::to_be_bytes).collect()
        };
        let (len, at) = (u16_at(&font, rec + 8), strings + u16_at(&font, rec + 10));
        assert!(bytes.len() <= len, "the new name fits the record");
        font[at..at + bytes.len()].copy_from_slice(&bytes);
        font[rec + 8..rec + 10].copy_from_slice(&(bytes.len() as u16).to_be_bytes());
    }
    font
}

/// A family split over two files that both give themselves its name draws each
/// character from the file the canvas drew it from, though a run in another
/// family comes first on the line with a character neither file has.
///
/// flip: `run_attrs` naming no `face_alias` → fails at "both files": usvg's glyph
/// fallback stops at the icon, so one file's character is `.notdef` and that
/// file is never embedded.
#[test]
fn each_subset_file_draws_what_it_drew_on_the_canvas() {
    use ondin_core::fallback_fonts::{Subset, subset_alias};
    use ondin_core::text::{face_covers, register_fonts_as};
    use ondin_core::{CharAttr, CharSpans};
    let latin_ext = include_bytes!("../../ondin-core/assets/fonts/Inter-Variable.ttf").to_vec();
    let emoji = include_bytes!("../../ondin-app/assets/fonts/NotoEmoji.ttf").to_vec();
    let icons = include_bytes!("../../ondin-app/assets/fonts/OndinIcons.ttf").to_vec();
    // The family's own file holds the emoji; its `latin-ext` file holds `Ł`.
    let emoji = renamed(emoji, "Inter");
    let icon = (0xE000..=0xF8FF)
        .filter_map(char::from_u32)
        .find(|&c| {
            face_covers(&icons, 0, c)
                && !face_covers(&latin_ext, 0, c)
                && !face_covers(&emoji, 0, c)
        })
        .expect("an icon only the icon font has");
    assert!(face_covers(&emoji, 0, '😀') && !face_covers(&emoji, 0, 'Ł'));
    assert!(register_fonts_as(emoji, "Twin Pdf"));
    assert!(register_fonts_as(
        latin_ext,
        &subset_alias("Twin Pdf", Subset::LatinExt)
    ));
    assert!(register_fonts_as(icons, "Icons Pdf"));

    let mut f = common::fixture();
    let defaults = TextStyle {
        font_family: "Twin Pdf".into(),
        ..inter(400)
    };
    let content = format!("{icon}😀Ł");
    let mut spans = CharSpans::default();
    spans.set(
        0..icon.len_utf8(),
        CharAttr::Family("Icons Pdf".into()),
        &defaults,
    );
    f.doc
        .apply(&Transaction(vec![
            Operation::SetTextStyle {
                id: f.text,
                spans: None,
                style: defaults,
            },
            Operation::SetText {
                id: f.text,
                content,
                spans: Default::default(),
                para_spans: Default::default(),
            },
            Operation::SetTextSpans { id: f.text, spans },
        ]))
        .expect("an icon, then the split family");
    f.res = Resolved::rebuild(&f.doc);
    let out = pdf_of(&f.doc, &f.res, &[f.artboard], ExportText::Live).expect("a pdf");
    let text = String::from_utf8_lossy(&out.pdf);
    let embedded = |name: &str| text.contains(&format!("+{name}"));
    assert!(embedded("OndinIcons"), "fixture: the icon is drawn\n{text}");
    assert!(
        embedded("NotoEmoji") && embedded("Inter"),
        "both files: {:?}",
        text.match_indices("/BaseFont/")
            .map(|(at, _)| &text[at..(at + 40).min(text.len())])
            .collect::<Vec<_>>()
    );
}

/// A subject with nothing to draw is no PDF, as it is no SVG or PNG.
///
/// flip: `pdf_pages` without its `extent` check → a blank 100×100 page.
#[test]
fn a_subject_with_nothing_to_draw_is_no_pdf() {
    let mut f = common::fixture();
    let group = ondin_core::IdSource::new(0xE11E).mint();
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: group,
            parent: f.artboard,
            index: 0,
            kind: ondin_core::NodeKind::Group,
            transform: None,
            name: None,
        }]))
        .expect("an empty group");
    f.res = Resolved::rebuild(&f.doc);
    assert!(pdf_of(&f.doc, &f.res, &[group], ExportText::Live).is_none());
}
