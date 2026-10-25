//! From a set of layers to a set of named files (§7).
//!
//! The one place an [`ExportSpec`] is interpreted: `Width(512)` becomes a multiplier
//! against a layer's box, `Background::Frame` a colour, a layer's name a unique
//! filename. The panel, the CLI's `--all` and the zip all plan through it, so the
//! same file is the same bytes under the same name from every door.
//!
//! Nothing here writes to disk: planning is pure and rendering returns bytes.

use ondin_core::peniko::Color;
use ondin_core::{
    Document, ExportBackground, ExportFormat, ExportScale, ExportSpec, NodeId, NodeKind, Resolved,
};

/// How a set of layers is laid out under the destination folder.
#[derive(Clone, Debug, Default)]
pub struct PlanOptions {
    /// Read `/` in a layer's name as a folder separator, so a layer called
    /// `icons/close` writes `icons/close.png`.
    ///
    /// Off by default: a layer named `before/after` should be one file
    /// `before-after`, not a folder, so the slash is sanitized away unless asked.
    pub folders_from_names: bool,
    /// Replace every spec's scale with this one (`ondin export --all
    /// --scale-override 1x`, the fast-preview pass) (§15 D361).
    ///
    /// It lives here because a scale is also half the filename through
    /// [`ExportScale::marker`]; overriding after planning would render at 1× under
    /// names saying `@2x`. Specs that become identical collapse to one file, only
    /// under an override, instead of de-colliding into `Close (2).png` junk.
    pub scale_override: Option<ExportScale>,
    /// What a subject with no specs of its own exports as — *Export all frames*'
    /// 1× PNG. `None` plans nothing for it.
    pub unset: Option<ExportSpec>,
}

/// One file an export will produce.
///
/// `pixels` lets the panel show the size before anything is written; `clamped`
/// flags a request the rasterizer could not meet.
#[derive(Clone, Debug)]
pub struct PlannedFile {
    /// The layer this renders.
    pub node: NodeId,
    pub spec: ExportSpec,
    /// Where it goes under the destination, `/`-separated and already
    /// de-collided against every other file in the same plan.
    pub path: String,
    /// The size a raster will come out at. `None` for SVG, which has no pixels.
    pub pixels: Option<(u32, u32)>,
    /// Whether [`Self::pixels`] is smaller than the spec asked for, because the
    /// request was past what the CPU rasterizer can allocate.
    pub clamped: bool,
}

/// Everything `subjects` would produce, in order, with no two files sharing a
/// name.
///
/// A layer with no specs contributes [`PlanOptions::unset`], by default nothing. The caller
/// filters and orders `subjects` (`build::in_document_order` keeps a group and its
/// own child from both being planned) (§15 D267).
pub fn plan(
    doc: &Document,
    res: &Resolved,
    subjects: &[NodeId],
    opts: PlanOptions,
) -> Vec<PlannedFile> {
    let mut taken: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for &node in subjects {
        let Some(n) = doc.get(node) else { continue };
        // Forced first, then deduplicated, since the override is what makes rows
        // equal. Per node: two layers may share a spec and their names separate
        // the files. Values, not keyed items, since equal specs carry different ids.
        let mut specs: Vec<ExportSpec> = ondin_core::item::values(n.exports());
        if specs.is_empty() {
            specs.extend(opts.unset.clone());
        }
        if let Some(forced) = opts.scale_override {
            for spec in &mut specs {
                spec.scale = forced;
            }
            let mut seen: Vec<ExportSpec> = Vec::new();
            specs.retain(|s| match seen.contains(s) {
                true => false,
                false => {
                    seen.push(s.clone());
                    true
                }
            });
        }
        for spec in &specs {
            // Composed first, sanitized second: the prefix and the name are two
            // halves of one path, so the folder rule must see both.
            let stem = path_stem(&spec.compose(n.name()), &opts);
            let wanted = format!("{stem}.{}", spec.format.extension());
            let path = de_collide(wanted, &mut taken);
            let (pixels, clamped) = match spec.format.is_raster() {
                true => raster_size(res, node, spec),
                false => (None, false),
            };
            out.push(PlannedFile {
                node,
                spec: spec.clone(),
                path,
                pixels,
                clamped,
            });
        }
    }
    out
}

/// The pixel size a raster spec produces for this layer, and whether it had to be
/// clamped to get there.
///
/// `None` when the layer has no bounds (an empty group), the refusal `png_of`
/// makes too. Public because the panel shows it without a render; it is the size
/// before trim and pad, which are not knowable without rasterizing.
pub fn raster_size(res: &Resolved, node: NodeId, spec: &ExportSpec) -> (Option<(u32, u32)>, bool) {
    // `ink_bounds`, because `png::raster_of` frames from it (§5.3a); the editing
    // box would be off by the reach of a shadow.
    let Some(b) = res.ink_bounds(node) else {
        return (None, false);
    };
    let (w, h) = (b.width().max(1.0), b.height().max(1.0));
    let f = spec.scale.factor(w, h);
    let (want_w, want_h) = ((w * f).ceil(), (h * f).ceil());
    let vp = crate::png::viewport_at(b, format_capped(f, w, h, spec.format));
    let clamped = want_w > vp.pixel_size.0 as f64 || want_h > vp.pixel_size.1 as f64;
    (Some(vp.pixel_size), clamped)
}

/// Render one planned file to bytes. `None` when the layer has no bounds to
/// frame — the one refusal both writers already make.
///
/// The caller must hand over the committed document and `Resolved`, so a gesture
/// halfway through a drag cannot leak an uncommitted position into a file
/// (§15 D264). Nothing here can check that.
pub fn render(doc: &Document, res: &Resolved, file: &PlannedFile) -> Option<Vec<u8>> {
    let origins = [file.node];
    match file.spec.format {
        ExportFormat::Svg => Some(crate::svg::svg_of(doc, res, &origins).into_bytes()),
        ExportFormat::Png | ExportFormat::Jpeg | ExportFormat::Webp => {
            let opts = raster_opts(doc, res, file.node, &file.spec);
            let r = crate::png::raster_of(doc, res, &origins, &opts)?;
            Some(encode(&r, &file.spec))
        }
    }
}

/// A raster encoded in `spec`'s format. Public because the panel's preview
/// rasterizes once and both encodes and draws the buffer. An SVG spec has no
/// encoder and gets PNG bytes.
pub fn encode(r: &crate::png::Raster, spec: &ExportSpec) -> Vec<u8> {
    match spec.format {
        ExportFormat::Jpeg => crate::jpeg::encode_rgba8(&r.rgba, r.width, r.height, spec.quality),
        ExportFormat::Webp => crate::webp::encode_rgba8(&r.rgba, r.width, r.height),
        ExportFormat::Png | ExportFormat::Svg => {
            crate::png::encode_rgba8(&r.rgba, r.width, r.height)
        }
    }
}

/// Resolve a spec against a layer: the multiplier it works out to, and the colour
/// its background asks for.
///
/// Public because the app's preview rasterizes once and both encodes and draws
/// the same buffer; going through [`render`] too would be a second render.
pub fn raster_opts(
    doc: &Document,
    res: &Resolved,
    node: NodeId,
    spec: &ExportSpec,
) -> crate::png::RasterOpts {
    // The box `raster_size` measures and `png::raster_of` frames from, or
    // `Width(n)` resolves against the wrong box.
    let b = res.ink_bounds(node);
    let (w, h) = b.map_or((1.0, 1.0), |b| (b.width().max(1.0), b.height().max(1.0)));
    crate::png::RasterOpts {
        scale: format_capped(spec.scale.factor(w, h), w, h, spec.format),
        background: background_color(doc, node, spec),
        trim: spec.trim,
        pad_square: spec.pad_square,
    }
}

/// `scale` capped so a `w`×`h` box fits the format's own side limit, which for
/// WebP is below the rasterizer's. Half a pixel short of the cap, so `ceil` of
/// the product cannot land one past it; trim and square padding only shrink or
/// reach the longer side.
fn format_capped(scale: f64, w: f64, h: f64, format: ExportFormat) -> f64 {
    match format {
        ExportFormat::Webp => scale.min((f64::from(crate::webp::MAX_SIDE) - 0.5) / w.max(h)),
        _ => scale,
    }
}

/// What colour goes behind this export, or `None` for transparency.
///
/// `Frame` answers `None` for a gradient or image background rather than
/// approximating it with a colour (§7's model-completeness gate).
///
/// JPEG's white is resolved here, not in the encoder: the stored setting stays
/// `Transparent` and only its meaning for a format with no alpha is white.
pub fn background_color(doc: &Document, node: NodeId, spec: &ExportSpec) -> Option<Color> {
    let asked = match spec.background {
        ExportBackground::Transparent => None,
        ExportBackground::Solid(c) => Some(c),
        ExportBackground::Frame => frame_background(doc, node),
    };
    match (asked, spec.format.keeps_alpha()) {
        (Some(c), _) => Some(c),
        (None, true) => None,
        (None, false) => Some(Color::WHITE),
    }
}

/// The solid background of the nearest enclosing frame, if it has one.
///
/// The frame containing the layer, so a frame exporting itself uses its own
/// ground (`png_of` leaves it out, since a frame's fills are painted by its own
/// visit).
///
/// Answers only where a single colour is exactly right (§15 D400). Over the
/// visible fills in z-order:
///
/// - the topmost is an opaque solid: that colour;
/// - the only visible fill is a solid, opaque or not: that colour;
/// - anything else (gradient or picture on top, translucent solid over something,
///   nothing visible): `None`, rather than compositing a stack down to one colour.
///
/// A hidden fill is skipped.
fn frame_background(doc: &Document, node: NodeId) -> Option<Color> {
    let mut at = Some(node);
    while let Some(id) = at {
        let n = doc.get(id)?;
        if matches!(n.kind(), NodeKind::Artboard { .. }) {
            // Resolved: a variable-bound frame uses the mode in force.
            let paint = doc.resolved_paint(id, n.paint());
            let visible: Vec<&ondin_core::Brush> = paint
                .fills
                .iter()
                .filter(|f| f.visible)
                .map(|f| f.brush.drawn())
                .collect();
            let ondin_core::Brush::Solid(c) = visible.last()? else {
                return None;
            };
            // `to_rgba8().a` is the committed byte, as in the written file.
            return (visible.len() == 1 || c.to_rgba8().a == 255).then_some(*c);
        }
        at = n.parent();
    }
    None
}

/// A layer's name as the leading part of a path — sanitized, and split on `/`
/// only when the caller asked for folders.
///
/// Windows is the strict one and therefore the rule: `<>:"\|?*`, control
/// characters, a trailing dot or space and device names (`CON`, `NUL`, `COM1`) are
/// refused or silently transformed by the filesystem. A name that sanitizes away
/// to nothing becomes `Layer`.
fn path_stem(name: &str, opts: &PlanOptions) -> String {
    let parts: Vec<&str> = match opts.folders_from_names {
        true => name.split('/').collect(),
        false => vec![name],
    };
    let cleaned: Vec<String> = parts
        .iter()
        .map(|p| sanitize_segment(p))
        .filter(|p| !p.is_empty())
        .collect();
    match cleaned.is_empty() {
        true => "Layer".to_string(),
        false => cleaned.join("/"),
    }
}

/// `s` as one file or folder name the strictest filesystem takes: separators and
/// Windows' forbidden characters as `-`, no trailing dot or space, a device name
/// prefixed with `_`. Empty when nothing of it is usable.
pub fn sanitize_segment(s: &str) -> String {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if (c as u32) < 0x20 => '-',
            c => c,
        })
        .collect();
    // A trailing dot or space is legal to write on Windows and then not there.
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    let head = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&head.as_str()) {
        out.insert(0, '_');
    }
    out.trim_start().to_string()
}

/// Make `wanted` unique against everything already planned, and record it.
///
/// Needed beyond the scale marker, which only separates rows on one layer: two
/// layers can share a name, and a spec can repeat. The result is ` (2)` before the
/// extension (Explorer's spelling), compared case-insensitively because
/// `Icon.png` and `icon.png` are one file on Windows and macOS.
///
/// Scoped to the plan: `taken` never reads the filesystem, so a file already on
/// disk at a planned path is replaced by the writer. Disclosing that belongs to
/// the door that writes without a save dialog (§15 D636).
fn de_collide(wanted: String, taken: &mut Vec<String>) -> String {
    let key = |s: &str| s.to_lowercase();
    if !taken.iter().any(|t| key(t) == key(&wanted)) {
        taken.push(wanted.clone());
        return wanted;
    }
    let (stem, ext) = match wanted.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (wanted.clone(), String::new()),
    };
    for n in 2.. {
        let candidate = format!("{stem} ({n}){ext}");
        if !taken.iter().any(|t| key(t) == key(&candidate)) {
            taken.push(candidate.clone());
            return candidate;
        }
    }
    unreachable!("the counter is unbounded")
}

/// Pack named files into a zip archive.
///
/// Always deflate, even though PNG and JPEG barely shrink: SVG halves, and
/// compression that depended on content would be unpredictable.
///
/// Folders are implied by `/` in a name; no directory entries are written.
pub fn zip(files: &[(String, Vec<u8>)]) -> std::io::Result<Vec<u8>> {
    use std::io::Write as _;
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut out);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in files {
            w.start_file(name.clone(), opts)?;
            w.write_all(bytes)?;
        }
        w.finish()?;
    }
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Size;
    use ondin_core::{ExportScale, Fill, IdSource, Operation, Transaction};

    /// A document with one 100×50 rect in a frame, both named.
    fn doc_with_rect(exports: Vec<ExportSpec>) -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, rect) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                },
                transform: None,
                name: Some("Board".into()),
            },
            Operation::SetFills {
                id: frame,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: ondin_core::Brush::Solid(Color::from_rgba8(10, 20, 30, 255)).into(),
                    visible: true,
                }]),
            },
            Operation::CreateNode {
                id: rect,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 50.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: Some("Button".into()),
            },
            Operation::SetExports {
                id: rect,
                exports: ondin_core::keyed_by_position(exports),
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, frame, rect)
    }

    /// `unset` stands in only for a layer with no specs: the frame gets it, the
    /// rect keeps its own SVG row.
    ///
    /// flip: `plan` filling `unset` whatever `specs` holds → fails at
    /// "a layer's own rows win".
    #[test]
    fn unset_plans_only_the_layers_without_specs() {
        let (doc, res, frame, rect) = doc_with_rect(vec![ExportSpec::new(
            ExportFormat::Svg,
            ExportScale::Times(1.0),
        )]);
        let opts = PlanOptions {
            unset: Some(ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0))),
            ..PlanOptions::default()
        };
        let paths: Vec<String> = plan(&doc, &res, &[frame, rect], opts)
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["Board.png", "Button.svg"], "a layer's own rows win");
        assert!(
            plan(&doc, &res, &[frame], PlanOptions::default()).is_empty(),
            "and without `unset` a bare layer plans nothing"
        );
    }

    /// A WebP row renders a `.webp` that is WebP bytes at the planned size.
    ///
    /// flip: `encode` sending `Webp` to the PNG arm → fails at the format check.
    #[test]
    fn a_webp_spec_renders_webp_at_the_planned_size() {
        let (doc, res, _, rect) = doc_with_rect(vec![ExportSpec::new(
            ExportFormat::Webp,
            ExportScale::Times(2.0),
        )]);
        let file = &plan(&doc, &res, &[rect], PlanOptions::default())[0];
        assert_eq!(file.path, "Button@2x.webp");
        let bytes = render(&doc, &res, file).expect("a rect has bounds");
        assert_eq!(
            image::guess_format(&bytes).ok(),
            Some(image::ImageFormat::WebP)
        );
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!(Some((img.width(), img.height())), file.pixels);
    }

    /// A WebP past the format's side limit is planned clamped to it and renders
    /// at the planned size, where PNG's cap would let it through.
    ///
    /// flip: `format_capped` passing WebP's scale through → fails at "the long
    /// side lands on the cap" (and `render` would panic in the encoder).
    #[test]
    fn a_webp_past_its_side_limit_is_clamped_not_a_panic() {
        let (mut doc, _, _, rect) = doc_with_rect(vec![ExportSpec::new(
            ExportFormat::Webp,
            ExportScale::Times(2.0),
        )]);
        doc.apply(&Transaction(vec![Operation::SetGeometry {
            id: rect,
            geometry: ondin_core::GeometryPatch::Size(Size::new(4.0, 9000.0)),
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let file = &plan(&doc, &res, &[rect], PlanOptions::default())[0];
        let (w, h) = file.pixels.expect("a rect has bounds");
        assert_eq!(h, crate::webp::MAX_SIDE, "the long side lands on the cap");
        assert!(w <= 8, "and the short side scales with it: {w}");
        assert!(file.clamped, "the panel is told the request was cut");
        let bytes = render(&doc, &res, file).expect("a rect has bounds");
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!(Some((img.width(), img.height())), file.pixels);
    }

    /// 2× of a 100×50 is 200×100, and `Width(512)` scales the unpinned axis too.
    #[test]
    fn the_planned_pixel_size_is_the_scale_applied_to_the_box() {
        let (doc, res, _, rect) = doc_with_rect(vec![
            ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0)),
            ExportSpec::new(ExportFormat::Png, ExportScale::Width(512)),
        ]);
        let files = plan(&doc, &res, &[rect], PlanOptions::default());
        assert_eq!(files[0].pixels, Some((200, 100)));
        assert_eq!(files[1].pixels, Some((512, 256)));
        assert!(!files[0].clamped && !files[1].clamped);
    }

    /// SVG has no pixels and no density marker.
    #[test]
    fn an_svg_spec_plans_no_pixel_size() {
        let (doc, res, _, rect) = doc_with_rect(vec![ExportSpec::new(
            ExportFormat::Svg,
            ExportScale::Times(3.0),
        )]);
        let files = plan(&doc, &res, &[rect], PlanOptions::default());
        assert_eq!(files[0].pixels, None);
        assert_eq!(
            files[0].path, "Button.svg",
            "a vector file carries no density marker"
        );
    }

    /// Specs differing only in scale are separated by the scale marker; identical
    /// ones fall to the de-collision, which must not overwrite. Both halves, since
    /// the identical case alone would pass with no marker at all.
    #[test]
    fn nothing_in_a_plan_shares_a_filename() {
        let (doc, res, _, rect) = doc_with_rect(vec![
            ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0)),
            ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0)),
        ]);
        let names: Vec<String> = plan(&doc, &res, &[rect], PlanOptions::default())
            .iter()
            .map(|f| f.path.clone())
            .collect();
        assert_eq!(names, ["Button.png", "Button@2x.png"]);

        let png = || ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0));
        let (doc, res, _, rect) = doc_with_rect(vec![png(), png(), png()]);
        let names: Vec<String> = plan(&doc, &res, &[rect], PlanOptions::default())
            .iter()
            .map(|f| f.path.clone())
            .collect();
        assert_eq!(names, ["Button.png", "Button (2).png", "Button (3).png"]);
    }

    /// The prefix goes through the same sanitizer as the name, so the folder rule
    /// governs both halves.
    #[test]
    fn a_prefix_is_a_folder_under_the_same_rule_as_a_name() {
        let spec = ExportSpec {
            prefix: "hero/".into(),
            ..ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0))
        };
        let (doc, res, _, rect) = doc_with_rect(vec![spec]);
        assert_eq!(
            plan(&doc, &res, &[rect], PlanOptions::default())[0].path,
            "hero-Button@2x.png",
            "with folders off a slash is not a separator anywhere"
        );
        assert_eq!(
            plan(
                &doc,
                &res,
                &[rect],
                PlanOptions {
                    folders_from_names: true,
                    ..PlanOptions::default()
                }
            )[0]
            .path,
            "hero/Button@2x.png"
        );
    }

    /// The Windows-forbidden set, including the trailing dot the filesystem drops.
    #[test]
    fn a_name_that_windows_would_refuse_is_repaired() {
        assert_eq!(sanitize_segment("a:b?c"), "a-b-c");
        assert_eq!(sanitize_segment("trailing."), "trailing");
        assert_eq!(sanitize_segment("NUL"), "_NUL");
        assert_eq!(sanitize_segment("con.png"), "_con.png");
        // Not reserved: the device names are whole segments, not prefixes.
        assert_eq!(sanitize_segment("console"), "console");
    }

    /// The slash rule is opt-in; off, a `/` is replaced rather than left as a folder.
    #[test]
    fn a_slash_is_a_folder_only_when_asked_for() {
        assert_eq!(
            path_stem("icons/close", &PlanOptions::default()),
            "icons-close"
        );
        assert_eq!(
            path_stem(
                "icons/close",
                &PlanOptions {
                    folders_from_names: true,
                    ..PlanOptions::default()
                }
            ),
            "icons/close"
        );
        assert_eq!(path_stem("  ", &PlanOptions::default()), "Layer");
    }

    /// `Frame` reaches past a group to the enclosing artboard; a layer outside one
    /// stays transparent. JPEG resolves to white while the stored value is untouched.
    #[test]
    fn the_background_resolves_up_the_tree_and_jpeg_never_stays_transparent() {
        let mut spec = ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0));
        spec.background = ExportBackground::Frame;
        let (doc, _res, frame, rect) = doc_with_rect(vec![spec.clone()]);
        assert_eq!(
            background_color(&doc, rect, &spec),
            Some(Color::from_rgba8(10, 20, 30, 255)),
            "a layer in a frame takes the frame's background"
        );
        assert_eq!(
            background_color(&doc, frame, &spec),
            Some(Color::from_rgba8(10, 20, 30, 255)),
            "and a frame exporting itself takes its own"
        );

        let transparent = ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0));
        assert_eq!(background_color(&doc, rect, &transparent), None);
        let jpeg = ExportSpec::new(ExportFormat::Jpeg, ExportScale::Times(1.0));
        assert_eq!(
            jpeg.background,
            ExportBackground::Transparent,
            "the stored value is not rewritten"
        );
        assert_eq!(
            background_color(&doc, rect, &jpeg),
            Some(Color::WHITE),
            "but a format with no alpha resolves it to white"
        );
    }

    /// Three specs, three formats, three files in a readable zip.
    #[test]
    fn a_plan_renders_to_bytes_and_packs() {
        let (doc, res, _, rect) = doc_with_rect(vec![
            ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0)),
            ExportSpec::new(ExportFormat::Jpeg, ExportScale::Times(1.0)),
            ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0)),
        ]);
        let files = plan(&doc, &res, &[rect], PlanOptions::default());
        let rendered: Vec<(String, Vec<u8>)> = files
            .iter()
            .map(|f| (f.path.clone(), render(&doc, &res, f).expect("bytes")))
            .collect();
        assert_eq!(&rendered[0].1[1..4], b"PNG");
        assert_eq!(&rendered[1].1[..2], &[0xFF, 0xD8]);
        assert!(rendered[2].1.starts_with(b"<svg"));

        let archive = zip(&rendered).expect("zip");
        let mut r = zip::ZipArchive::new(std::io::Cursor::new(archive)).expect("readable archive");
        assert_eq!(r.len(), 3);
        let names: Vec<String> = r.file_names().map(str::to_string).collect();
        for f in &files {
            assert!(names.contains(&f.path), "{} missing from {names:?}", f.path);
        }
        // The bytes survive the round trip.
        use std::io::Read as _;
        let mut first = r.by_name("Button@2x.png").expect("by name");
        let mut back = Vec::new();
        first.read_to_end(&mut back).unwrap();
        assert_eq!(back, rendered[0].1);
    }
}
