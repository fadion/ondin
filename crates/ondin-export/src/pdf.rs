//! PDF export (§7.1) — our SVG, converted.
//!
//! The SVG writer ([`crate::svg::svg_with`]) is the one description of the drawing;
//! `usvg` reads it back and `krilla` draws the result as one page, a point per world
//! unit. A `<filter>` (an effect, a picture adjustment) has no PDF form and is drawn
//! as pixels at [`FILTER_SCALE`] times the page's resolution.
//!
//! Live text is shaped again by `usvg` from exactly the font files our layout used,
//! and embedded as subsets, so it can be selected and searched. Line breaks and
//! baselines are ours; placement along a line is the converter's. Outlined text is
//! our own glyphs and carries no font.

use std::sync::Arc;

use krilla_svg::SurfaceExt;
use ondin_core::peniko::FontData;
use ondin_core::{Brush, Document, ExportText, NodeId, Resolved};

/// How much denser than the page a filtered layer is rasterized: 4 is 288 dpi at
/// a point per world unit, less for a layer past 1250 units, since `krilla-svg`
/// caps a raster at 5000 px a side.
const FILTER_SCALE: f32 = 4.0;

/// A PDF and what it could not say exactly: the SVG's `approximated`, plus what the
/// conversion loses. A PDF is never read back, so the SVG's round-trip notes do not
/// apply.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Export {
    pub pdf: Vec<u8>,
    /// Features drawn differently from the canvas. Deduplicated, in order met.
    pub approximated: Vec<String>,
}

/// These subtrees as one PDF page, framed as [`crate::svg::svg_of`] frames them.
///
/// `None` when the converter refuses the markup, which our own SVG should never
/// make it do.
pub fn pdf_of(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    text: ExportText,
) -> Option<Export> {
    pdf_pages(doc, res, &[(String::new(), origins.to_vec())], text)
}

/// [`pdf_of`] framed on `view`, a world-space box, as [`crate::svg::svg_region`]
/// frames it: what a slice exports.
pub fn pdf_region(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    view: ondin_core::kurbo::Rect,
    text: ExportText,
) -> Option<Export> {
    pdf_framed(
        doc,
        res,
        &[(String::new(), origins.to_vec(), Some(view))],
        text,
    )
}

/// Several pages in one PDF, each its subtrees framed as [`pdf_of`] frames one,
/// at its own size. A page with a name gets a bookmark of that name, so a reader's
/// sidebar lists the frames; unnamed pages get none.
///
/// `None` when `pages` is empty, a page has nothing to frame, or the converter
/// refuses one. The notes are every page's, deduplicated.
pub fn pdf_pages(
    doc: &Document,
    res: &Resolved,
    pages: &[(String, Vec<NodeId>)],
    text: ExportText,
) -> Option<Export> {
    let framed: Vec<(String, Vec<NodeId>, Option<ondin_core::kurbo::Rect>)> = pages
        .iter()
        .map(|(name, origins)| (name.clone(), origins.clone(), None))
        .collect();
    pdf_framed(doc, res, &framed, text)
}

/// [`pdf_pages`] with each page framed on its own box where it names one, and on
/// its subtrees' extent where it does not.
fn pdf_framed(
    doc: &Document,
    res: &Resolved,
    pages: &[(String, Vec<NodeId>, Option<ondin_core::kurbo::Rect>)],
    text: ExportText,
) -> Option<Export> {
    if pages.is_empty() {
        return None;
    }
    let mut document = krilla::Document::new();
    let mut outline = krilla::outline::Outline::new();
    let mut approximated: Vec<String> = Vec::new();
    for (index, (name, origins, view)) in pages.iter().enumerate() {
        if view.is_none() {
            crate::extent(res, origins)?;
        }
        // Each run names the face that drew it (`svg_linking`'s `faces`).
        let svg = crate::svg::svg_linking(doc, res, origins, Default::default(), text, *view, true);
        let shown = shown_nodes(doc, origins);
        for note in approximations(doc, res, &shown, svg.approximated, text) {
            if !approximated.contains(&note) {
                approximated.push(note);
            }
        }

        let missed = std::sync::Mutex::new(Vec::new());
        let tree = page_tree(&svg.svg, fonts_used(res, &shown), &missed)?;
        for (family, drawn_in) in missed.into_inner().unwrap_or_default() {
            let note = format!("{family}, which is not available, drawn in {drawn_in}");
            if !approximated.contains(&note) {
                approximated.push(note);
            }
        }
        let size = krilla::geom::Size::from_wh(tree.size().width(), tree.size().height())?;
        let mut page = document.start_page_with(krilla::page::PageSettings::new(size));
        let mut surface = page.surface();
        surface.draw_svg(
            &tree,
            size,
            krilla_svg::SvgSettings {
                embed_text: true,
                filter_scale: FILTER_SCALE,
            },
        );
        surface.finish();
        page.finish();
        if !name.is_empty() {
            let top = krilla::geom::Point::from_xy(0.0, 0.0);
            outline.push_child(krilla::outline::OutlineNode::new(
                name.clone(),
                krilla::destination::XyzDestination::new(index, top),
            ));
        }
    }
    // Only with a bookmark in it, so a one-page export's bytes are what they were.
    if pages.iter().any(|(name, ..)| !name.is_empty()) {
        document.set_outline(outline);
    }
    let pdf = document.finish().ok()?;
    Some(Export { pdf, approximated })
}

/// One page's SVG read back against exactly `fonts`, each face filed under its
/// file's `svg::face_alias` ahead of its own names: a run names that alias, so
/// it is shaped in the file that drew it on the canvas. One family's subset files
/// all give themselves its name, and usvg's glyph fallback, which a lookup by
/// that name would lean on, stops at the first character none of them has.
fn page_tree(
    svg: &str,
    fonts: Vec<FontData>,
    missed: &std::sync::Mutex<Vec<(String, String)>>,
) -> Option<usvg::Tree> {
    let mut db = usvg::fontdb::Database::new();
    for font in fonts {
        let blob = font.data.id();
        for id in db.load_font_source(usvg::fontdb::Source::Binary(Arc::new(font.data))) {
            let Some(mut face) = db.face(id).cloned() else {
                continue;
            };
            db.remove_face(id);
            let alias = crate::svg::face_alias((blob, face.index));
            face.families
                .insert(0, (alias, usvg::fontdb::Language::English_UnitedStates));
            db.push_face_info(face);
        }
    }
    let opts = usvg::Options {
        font_family: FALLBACK_FAMILY.to_string(),
        fontdb: Arc::new(db),
        font_resolver: usvg::FontResolver {
            select_font: select_font(missed),
            select_fallback: same_family_fallback(),
        },
        // Only our own `data:` pictures are read. A linked picture's path names a
        // file, or an SMB share, that a received document chose.
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..Default::default()
        },
        ..Default::default()
    };
    usvg::Tree::from_str(svg, &opts).ok()
}

/// Core's own fallback family, which a layout that missed its family drew in.
const FALLBACK_FAMILY: &str = "Inter";

/// usvg's glyph fallback, kept to faces of the family the run's face gives
/// itself, for a run that names no file ([`page_tree`]). Never another family's:
/// every run names the face that drew it (`svg_linking`'s `faces`), and usvg
/// redraws a whole line in a fallback face that covers all of it.
fn same_family_fallback() -> usvg::FallbackSelectionFn<'static> {
    Box::new(|c, exclude, db| {
        let base = db.face(*exclude.first()?)?;
        let kin = |face: &usvg::fontdb::FaceInfo| {
            face.families.iter().any(|(name, _)| {
                base.families
                    .iter()
                    .any(|(own, _)| own.eq_ignore_ascii_case(name))
            })
        };
        let covers = |id| {
            db.with_face_data(id, |data, index| {
                ondin_core::text::face_covers(data, index, c)
            }) == Some(true)
        };
        db.faces()
            .filter(|face| !exclude.contains(&face.id) && kin(face) && covers(face.id))
            .min_by_key(|face| {
                (
                    face.style != base.style,
                    face.weight.0.abs_diff(base.weight.0),
                )
            })
            .map(|face| face.id)
    })
}

/// usvg's face lookup, falling back as core's font stack does when no face of the
/// named families is in the page's database — to a family's stand-in
/// (`fallback_fonts::stand_ins`) when the layout used one, else to
/// [`FALLBACK_FAMILY`] — and noting the family and what drew it in `missed`.
/// usvg's own fallback asks for a serif the database never holds, and then skips
/// the run, so the text would be absent from the page.
fn select_font(missed: &std::sync::Mutex<Vec<(String, String)>>) -> usvg::FontSelectionFn<'_> {
    let named = usvg::FontResolver::default_font_selector();
    Box::new(move |font, db| {
        let wanted = |name: &str| {
            font.families()
                .iter()
                .any(|f| matches!(f, usvg::FontFamily::Named(n) if n.eq_ignore_ascii_case(name)))
        };
        if db
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| wanted(name)))
        {
            return named(font, db);
        }
        // The face of `family` nearest the run's style and weight.
        let italic = font.style() != usvg::FontStyle::Normal;
        let closest = |family: &str| {
            db.faces()
                .filter(|face| {
                    face.families
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case(family))
                })
                .min_by_key(|face| {
                    let slanted = face.style != usvg::fontdb::Style::Normal;
                    (slanted != italic, face.weight.0.abs_diff(font.weight()))
                })
                .map(|face| face.id)
        };
        let stand_in = font
            .families()
            .iter()
            .filter_map(|f| match f {
                usvg::FontFamily::Named(name) => Some(name.as_str()),
                _ => None,
            })
            .flat_map(|name| ondin_core::fallback_fonts::stand_ins(name).iter())
            .find_map(|s| closest(s).map(|id| (*s, id)));
        let (drawn_in, id) = match stand_in {
            Some((name, id)) => (name, Some(id)),
            None => (
                FALLBACK_FAMILY,
                closest(FALLBACK_FAMILY).or_else(|| db.faces().next().map(|face| face.id)),
            ),
        };
        if let Ok(mut missed) = missed.lock() {
            let family = font
                .families()
                .iter()
                .map(|f| match f {
                    usvg::FontFamily::Named(name) => name.clone(),
                    generic => generic.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let entry = (family, drawn_in.to_string());
            if !missed.contains(&entry) {
                missed.push(entry);
            }
        }
        id
    })
}

/// What [`pdf_of`] would report, without converting: the export card's note, read
/// on every edit.
pub fn approximated(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    text: ExportText,
) -> Vec<String> {
    let svg = crate::svg::svg_linking(doc, res, origins, Default::default(), text, None, true);
    approximations(doc, res, &shown_nodes(doc, origins), svg.approximated, text)
}

/// The SVG's notes, then the conversion's own.
fn approximations(
    doc: &Document,
    res: &Resolved,
    shown: &[NodeId],
    mut out: Vec<String>,
    text: ExportText,
) -> Vec<String> {
    let mut note = |what: &str| {
        if !out.iter().any(|s| s == what) {
            out.push(what.to_string());
        }
    };
    for &id in shown {
        let Some(node) = doc.get(id) else { continue };
        if ondin_render::effects::any_ink(&doc.resolved_effects(id, node.effects())) {
            note("an effect, drawn as pixels");
            // The whole `<g filter>` is one raster, text and vectors under it too.
            if !node.children().is_empty() {
                note(
                    "a frame or group with an effect, drawn as pixels with everything inside it, text included",
                );
            }
        }
        // `usvg` draws a decoration from the font's own line: its colour,
        // thickness, style, offset and skip-ink are not carried.
        if text == ExportText::Live
            && let Some(parts) = ondin_core::TextRef::of(res.used_kind_of(node))
            && parts
                .spans
                .runs(parts.style, parts.content.len())
                .iter()
                .any(|(_, s)| s.underline.is_some() || s.strikethrough.is_some())
        {
            note("underlines and strikethroughs, drawn as the font's plain line");
        }
        // A family with no face was drawn in another, which the PDF names
        // (`svg_linking`'s `faces`) and so never misses. Whether it had a face is
        // the layout's word (`TextLayout::absent`), not this thread's engine. Shaped
        // again only when some family is absent: this runs on every edit for the
        // export card.
        if text == ExportText::Live
            && let Some(absent) = res.text_layout(id).map(|l| &l.absent)
            && !absent.is_empty()
            && let Some(parts) = ondin_core::TextRef::of(res.used_kind_of(node))
        {
            for line in ondin_core::text::export_lines(parts) {
                for run in &line.runs {
                    if let Some(face) = &run.face
                        && stands_in_for(&run.style.font_family, face)
                        && absent.contains(&run.style.font_family)
                    {
                        note(&format!(
                            "{}, which is not available, drawn in {face}",
                            run.style.font_family
                        ));
                    }
                }
            }
        }
        let paint = doc.resolved_paint(id, node.paint());
        let adjusted = paint.fills.iter().filter(|f| f.visible).any(
            |f| matches!(f.brush.drawn(), Brush::Image(img) if img.image.adjust.pipeline().is_some()),
        );
        if adjusted {
            note("a picture's adjustments, drawn as pixels");
        }
        // `usvg` reads `font-feature-settings` and does not shape with it.
        if text == ExportText::Live
            && let Some(parts) = ondin_core::TextRef::of(res.used_kind_of(node))
            && parts
                .spans
                .runs(parts.style, parts.content.len())
                .iter()
                .any(|(_, s)| !s.features.is_empty())
        {
            note("OpenType features, which embedded text does not apply");
        }
    }
    out
}

/// Whether `face` can have drawn a run because `family` was missing: core's
/// fallback or a stand-in of the family. A script's fallback draws characters the
/// family lacks whether or not it is there, so it says nothing of the family.
fn stands_in_for(family: &str, face: &str) -> bool {
    face.eq_ignore_ascii_case(FALLBACK_FAMILY)
        || ondin_core::fallback_fonts::stand_ins(family)
            .iter()
            .any(|s| s.eq_ignore_ascii_case(face))
}

/// The nodes the SVG writes, in pre-order: a hidden layer is skipped with
/// everything under it, as `svg::emit_node` skips it.
fn shown_nodes(doc: &Document, origins: &[NodeId]) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = origins.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if !doc.shown(id) {
            continue;
        }
        out.push(id);
        stack.extend(node.children().iter().rev().copied());
    }
    out
}

/// Every font file a shown text layer's layout drew from, once each — fallbacks
/// included, since a run is the face parley actually resolved.
fn fonts_used(res: &Resolved, shown: &[NodeId]) -> Vec<FontData> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for layout in shown.iter().filter_map(|id| res.text_layout(*id)) {
        for run in &layout.runs {
            if seen.insert(run.font.data.id()) {
                out.push(run.font.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use usvg::fontdb::{Database, Language, Source};

    /// A run drawn in Inter or in the family's stand-in says the family was
    /// missing; one drawn in a script's fallback does not.
    ///
    /// flip: `stands_in_for` answering true for any face → fails at "a script's
    /// fallback".
    #[test]
    fn only_a_stand_in_or_inter_says_the_family_was_missing() {
        assert!(stands_in_for("Montserrat", "Inter"), "core's fallback");
        assert!(stands_in_for("Calibri", "Carlito"), "a stand-in");
        assert!(
            !stands_in_for("Montserrat", "Noto Sans JP"),
            "a script's fallback"
        );
    }

    /// The glyph fallback reaches a face of the run's own family that has the
    /// character — one family's second subset file — and never another family's.
    ///
    /// flip: `same_family_fallback` answering `None` → fails at "the family's
    /// other file".
    #[test]
    fn the_glyph_fallback_stays_within_the_family() {
        let inter = include_bytes!("../../ondin-core/assets/fonts/Inter-Variable.ttf");
        let emoji = include_bytes!("../../ondin-app/assets/fonts/NotoEmoji.ttf");
        let mut db = Database::new();
        db.load_font_source(Source::Binary(Arc::new(inter.to_vec())));
        let base = db.faces().next().expect("Inter").id;
        let mut other = Database::new();
        other.load_font_source(Source::Binary(Arc::new(emoji.to_vec())));
        let face = other.faces().next().expect("Noto Emoji").clone();
        // Emoji filed under Inter's name stands for a subset file of the family.
        let own = db.push_face_info(usvg::fontdb::FaceInfo {
            families: vec![("Inter".into(), Language::English_UnitedStates)],
            ..face.clone()
        });
        let foreign = db.push_face_info(face);
        let mut db = Arc::new(db);
        let fallback = same_family_fallback();
        assert_eq!(
            fallback('😀', &[base], &mut db),
            Some(own),
            "the family's other file"
        );
        assert_eq!(
            fallback('😀', &[base, own], &mut db),
            None,
            "not another family's"
        );
        assert_eq!(
            db.with_face_data(foreign, |data, index| {
                ondin_core::text::face_covers(data, index, '😀')
            }),
            Some(true),
            "fixture: the other family has the character"
        );
        assert_eq!(
            fallback('A', &[own], &mut db),
            Some(base),
            "back the other way"
        );
    }
}
