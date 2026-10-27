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
/// a point per world unit.
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
    if pages.is_empty() {
        return None;
    }
    let mut document = krilla::Document::new();
    let mut outline = krilla::outline::Outline::new();
    let mut approximated: Vec<String> = Vec::new();
    for (index, (name, origins)) in pages.iter().enumerate() {
        crate::extent(res, origins)?;
        let svg = crate::svg::svg_with(doc, res, origins, text);
        let shown = shown_nodes(doc, origins);
        for note in approximations(doc, res, &shown, svg.approximated, text) {
            if !approximated.contains(&note) {
                approximated.push(note);
            }
        }

        let mut fonts = usvg::fontdb::Database::new();
        for font in fonts_used(res, &shown) {
            fonts.load_font_source(usvg::fontdb::Source::Binary(Arc::new(font.data)));
        }
        let missed = std::sync::Mutex::new(Vec::new());
        let opts = usvg::Options {
            font_family: FALLBACK_FAMILY.to_string(),
            fontdb: Arc::new(fonts),
            font_resolver: usvg::FontResolver {
                select_font: select_font(&missed),
                ..Default::default()
            },
            // Only our own `data:` pictures are read. A linked picture's path names a
            // file, or an SMB share, that a received document chose.
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_string: Box::new(|_, _| None),
                ..Default::default()
            },
            ..Default::default()
        };
        let tree = usvg::Tree::from_str(&svg.svg, &opts).ok()?;
        drop(opts);
        for family in missed.into_inner().unwrap_or_default() {
            let note = format!("{family}, which is not available, drawn in {FALLBACK_FAMILY}");
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
    if pages.iter().any(|(name, _)| !name.is_empty()) {
        document.set_outline(outline);
    }
    let pdf = document.finish().ok()?;
    Some(Export { pdf, approximated })
}

/// Core's own fallback family, which a layout that missed its family drew in.
const FALLBACK_FAMILY: &str = "Inter";

/// usvg's face lookup, falling back to [`FALLBACK_FAMILY`] when no face of the
/// named families is in the page's database, and noting the family in `missed`.
/// usvg's own fallback asks for a serif the database never holds, and then skips
/// the run, so the text would be absent from the page.
fn select_font(missed: &std::sync::Mutex<Vec<String>>) -> usvg::FontSelectionFn<'_> {
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
            if !missed.contains(&family) {
                missed.push(family);
            }
        }
        let italic = font.style() != usvg::FontStyle::Normal;
        let fallback = db
            .faces()
            .filter(|face| {
                face.families
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(FALLBACK_FAMILY))
            })
            .min_by_key(|face| {
                let slanted = face.style != usvg::fontdb::Style::Normal;
                (slanted != italic, face.weight.0.abs_diff(font.weight()))
            });
        fallback.or_else(|| db.faces().next()).map(|face| face.id)
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
    let svg = crate::svg::svg_with(doc, res, origins, text);
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
