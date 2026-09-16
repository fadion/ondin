//! From a set of layers to a set of named files (§7).
//!
//! This is the one place an [`ExportSpec`] is *interpreted*: `Width(512)` becomes
//! a multiplier against a particular layer's box, `Background::Frame` becomes a
//! colour by looking up the tree, and a layer's name becomes a filename nothing
//! else in the set has. Everything downstream takes numbers.
//!
//! **One interpreter, three callers.** The panel's button, the CLI's `--all` and
//! the zip all plan the same way, so a file exported from the app and the same
//! file exported from a build script are the same bytes under the same name. The
//! alternative — each caller reading the spec its own way — is how a 2× from one
//! door comes out a pixel wider than a 2× from the other.
//!
//! **Nothing here writes to disk.** Planning is pure and rendering returns bytes;
//! `ondin-app` and the CLI own the filesystem, which is what keeps this crate
//! testable without one and is the same division `svg_of` and `png_of` already
//! sit on.

use ondin_core::peniko::Color;
use ondin_core::{
    Document, ExportBackground, ExportFormat, ExportScale, ExportSpec, NodeId, NodeKind, Resolved,
};

/// How a set of layers is laid out under the destination folder.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlanOptions {
    /// Read `/` in a layer's name as a folder separator, so a layer called
    /// `icons/close` writes `icons/close.png`.
    ///
    /// **Off by default, because it changes what a name means.** A designer who
    /// has never heard of the convention and names a layer `before/after` should
    /// get one file called `before-after`, not a folder — so the rule is opt-in,
    /// and the slash is sanitized away when it is off.
    pub folders_from_names: bool,
    /// Replace **every** spec's scale with this one — `ondin export --all
    /// --scale-override 1x`, the fast-preview pass over a set built for retina
    /// (§15 D361).
    ///
    /// It goes here rather than in the caller because a scale is not only a size:
    /// it is half the *filename* through [`ExportScale::marker`], and a caller
    /// that overrode the multiplier after planning would render at 1× under names
    /// still saying `@2x`. One interpreter, which is this module's whole premise.
    ///
    /// **Specs that become identical collapse to one file.** Overriding is the
    /// operation that makes two rows the same row — a layer set up at 1×/2×/3×
    /// forced to 1× is one asset three times — and de-collision would otherwise
    /// turn that into `Close.png`, `Close (2).png`, `Close (3).png`: slower than
    /// the pass being asked for and two files of junk. The dedupe runs **only**
    /// under an override, so the unforced plan is byte-for-byte what it was.
    pub scale_override: Option<ExportScale>,
}

/// One file an export will produce.
///
/// Carries what it took to decide, not just the answer: `pixels` is what the
/// panel puts on the row so the size is visible before anything is written, and
/// `clamped` is the honest flag for a request the rasterizer could not meet.
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
/// **A layer with no specs contributes nothing** rather than a default PNG — an
/// export set is something you build, and a silent extra file is worse than a
/// missing one you can see is missing.
///
/// **The caller filters and orders `subjects`**, as it does for both writers:
/// `build::in_document_order` is the one that keeps a group and its own child
/// from both being planned (§15 D267). Here the consequence is milder than it is
/// there — two overlapping subjects are two files rather than one drawn twice —
/// but the rule is the same and stating it twice is cheaper than a caller
/// guessing.
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
        // Forced first, and *then* deduplicated, because the override is what makes
        // two rows equal — the same list read the other way round would dedupe a
        // set that is still distinct. Per node rather than across the plan: two
        // layers legitimately carry the same spec, and it is their names that keep
        // the files apart.
        let mut specs: Vec<ExportSpec> = n.exports().to_vec();
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
            // **Composed first, sanitized second**, which is the order that makes
            // a `/` mean one thing: the prefix and the layer's name are the two
            // halves of one path, so a folder rule that ran over only one of them
            // would let `hero/` become a directory while a layer called
            // `icons/close` did not — or the reverse.
            let stem = path_stem(&spec.compose(n.name()), opts);
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
/// `None` when the layer has no bounds at all — an empty group, an id naming
/// nothing — which is the same refusal `png_of` makes and for the same reason:
/// there is no box to frame.
///
/// **Public because the panel shows it**: `2x → 96×96` on the row is the cheapest
/// answer to "why is my icon 47 pixels", and it costs arithmetic rather than a
/// render. A trim would make it smaller and a pad would square it, neither of
/// which is knowable without rasterizing — so this is the size *before* those two,
/// and the row says the scale's answer rather than promising the file's.
pub fn raster_size(res: &Resolved, node: NodeId, spec: &ExportSpec) -> (Option<(u32, u32)>, bool) {
    // **`ink_bounds`, because `png::raster_of` frames from `extent`, which does**
    // (§5.3a). This row predicts the file's size, so measuring the *editing* box
    // while the writer frames the *ink* box makes the Export panel promise one
    // number and the file carry another — off by the whole reach of a shadow, and
    // only on the layers anyone would notice it on.
    let Some(b) = res.ink_bounds(node) else {
        return (None, false);
    };
    let (w, h) = (b.width().max(1.0), b.height().max(1.0));
    let f = spec.scale.factor(w, h);
    let (want_w, want_h) = ((w * f).ceil(), (h * f).ceil());
    let vp = crate::png::viewport_at(b, f);
    let clamped = want_w > vp.pixel_size.0 as f64 || want_h > vp.pixel_size.1 as f64;
    (Some(vp.pixel_size), clamped)
}

/// Render one planned file to bytes. `None` when the layer has no bounds to
/// frame — the one refusal both writers already make.
///
/// **The committed document and `Resolved`, always**, which is the caller's job
/// to hand over: `svg_of` and `png_of` are pure functions of the two, so a
/// gesture halfway through a drag cannot leak an uncommitted position into a file
/// (§15 D264). Nothing here can check that, which is why it is said here as well
/// as at the two call sites.
pub fn render(doc: &Document, res: &Resolved, file: &PlannedFile) -> Option<Vec<u8>> {
    let origins = [file.node];
    match file.spec.format {
        ExportFormat::Svg => Some(crate::svg::svg_of(doc, res, &origins).into_bytes()),
        ExportFormat::Png | ExportFormat::Jpeg => {
            let opts = raster_opts(doc, res, file.node, &file.spec);
            let r = crate::png::raster_of(doc, res, &origins, &opts)?;
            Some(match file.spec.format {
                ExportFormat::Jpeg => {
                    crate::jpeg::encode_rgba8(&r.rgba, r.width, r.height, file.spec.quality)
                }
                _ => crate::png::encode_rgba8(&r.rgba, r.width, r.height),
            })
        }
    }
}

/// Resolve a spec against a layer: the multiplier it works out to, and the colour
/// its background asks for.
///
/// Public because the app's preview rasterizes once and then both encodes *and*
/// draws the same buffer — going through [`render`] as well would be a second
/// render of the same thing, and a preview that cost two renders per edit is the
/// one bound the section has.
pub fn raster_opts(
    doc: &Document,
    res: &Resolved,
    node: NodeId,
    spec: &ExportSpec,
) -> crate::png::RasterOpts {
    // The same box `raster_size` measures and `png::raster_of` frames from, or
    // `ExportScale::Width(n)` resolves its factor against a box the file is not
    // framed on and the image comes out the wrong width.
    let b = res.ink_bounds(node);
    let (w, h) = b.map_or((1.0, 1.0), |b| (b.width().max(1.0), b.height().max(1.0)));
    crate::png::RasterOpts {
        scale: spec.scale.factor(w, h),
        background: background_color(doc, node, spec),
        trim: spec.trim,
        pad_square: spec.pad_square,
    }
}

/// What colour goes behind this export, or `None` for transparency.
///
/// **`Frame` answers `None` for a gradient or an image background**, rather than
/// approximating one with a colour. A flat stand-in for a gradient is the kind of
/// silent substitution §7's model-completeness gate exists to forbid, and the
/// honest failure — the layer on transparency, as it would have been anyway — is
/// visible in the file. The panel says which frame it found so an empty answer is
/// not a mystery.
///
/// **JPEG's white is here and not in the encoder**, because it is a decision
/// rather than a format limit: the stored setting stays `Transparent` and what it
/// *means* for a format with no alpha is white. Resolving it here keeps the spec
/// unrewritten — the panel can show a JPEG's effective background without the
/// value on disk having changed behind the user's back.
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
/// The frame *containing* the layer, so a frame exporting itself uses its own
/// ground — which is what selecting a frame and exporting it obviously means, and
/// what `png_of` would otherwise leave out, since the walk starts at the origin
/// and a frame's own fills are painted by its own visit.
///
/// ⚠️ **A frame's ground is a *list* now and this had to decide what one colour
/// means over one** (§15 D400). The rule is the strict one, because the existing
/// rule above it was already strict: **answer only where a single colour is
/// exactly right, and `None` everywhere else.** Concretely, over the visible fills
/// in z-order:
///
/// - **The topmost is an opaque solid** → that colour. Exact: everything under it
///   is covered.
/// - **The only visible fill is a solid**, opaque or not → that colour. Exact for
///   the same reason from the other end: there is nothing under it. This is also
///   the whole of what a v3 frame could hold, so no document that existed before
///   the list did changes its export.
/// - **Anything else** — a gradient or picture on top, a translucent solid over
///   something else, nothing visible at all → `None`. Compositing a stack down to
///   one colour is the silent substitution §7's model-completeness gate forbids,
///   and the honest failure is the layer on transparency, visible in the file.
///
/// A hidden fill is skipped throughout, because it is not drawn.
fn frame_background(doc: &Document, node: NodeId) -> Option<Color> {
    let mut at = Some(node);
    while let Some(id) = at {
        let n = doc.get(id)?;
        if matches!(n.kind(), NodeKind::Artboard { .. }) {
            let visible: Vec<&ondin_core::Brush> = n
                .paint()
                .fills
                .iter()
                .filter(|f| f.visible)
                .map(|f| &f.brush)
                .collect();
            let ondin_core::Brush::Solid(c) = visible.last()? else {
                return None;
            };
            // `to_rgba8().a` is the committed byte, so this asks the same
            // question the written file will.
            return (visible.len() == 1 || c.to_rgba8().a == 255).then_some(*c);
        }
        at = n.parent();
    }
    None
}

/// A layer's name as the leading part of a path — sanitized, and split on `/`
/// only when the caller asked for folders.
///
/// **Windows is the strict one and therefore the rule.** `<>:"\|?*`, the control
/// characters, a trailing dot or space and the device names (`CON`, `NUL`, `COM1`)
/// are all refused by the filesystem or, worse, silently transformed by it — and a
/// file the app said it wrote that is not there is the failure worth the most
/// effort to avoid. A name that sanitizes away to nothing becomes `Layer`, because
/// every file has to be called something.
fn path_stem(name: &str, opts: PlanOptions) -> String {
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

fn sanitize_segment(s: &str) -> String {
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
    // A trailing dot or space is legal to *write* on Windows and is then not
    // there when you look for it, which is the worst of the three outcomes.
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
/// **No two files in one plan share a name**, which the scale marker alone cannot
/// guarantee: it separates two rows *on one layer*, and two different layers can
/// share a name (duplicating one is how), each carrying the same specs by design.
///
/// The other case it catches is a genuinely repeated spec — the same format at the
/// same scale twice, where the marker is the same string. The de-collision is ` (2)` before the
/// extension — Explorer's own spelling, so the result reads as a copy rather than
/// as a mangled name — and it is case-insensitive because Windows and macOS both
/// are: `Icon.png` and `icon.png` are one file on the disk this is writing to, and
/// finding that out at write time means the second one has already replaced the
/// first.
///
/// ⚠️ **Scoped to the plan, and it used to open "nothing is silently
/// overwritten"** (§15 D652, `[S8.2-L5-07]`). That absolute is false and this
/// function cannot make it true: `taken` is a local `Vec<String>` created in
/// [`plan`] and dropped with it, and nothing here reads the filesystem. A file
/// *already on disk* at a planned path is replaced by whichever writer runs next.
/// The scope is the whole of the correction here — **the disclosure belongs to the
/// door, not to the plan**, which is §15 D636's ruling: a *File* or an *Archive*
/// was named in a save dialog a moment earlier and the OS has already warned, and
/// a door that writes with **no dialog at all** owes the user a count instead.
///
/// D636 gave that count to the panel's folder arm and did not reach the CLI, which
/// is a separately implemented walk (D626) and is the other no-dialog door — and
/// the worse of the two, because `main.rs`'s `run_export_all` defaults its output
/// directory to **the folder holding the `.ondin`** when `-o` is absent, which in
/// the library is a project folder the user keeps other things in. So
/// `ondin export brand.ondin --all` on a document with a layer named `logo`
/// replaced a hand-authored `logo.svg` beside it and printed `wrote 1 file`. It
/// now prints `wrote 1 file (1 replaced)`; D636's mechanism, at D636's other door.
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
/// **Deflate, though most of what goes in is already compressed.** A PNG or a JPEG
/// gains a percent or two at best, and paying for that would be pointless on its
/// own — but an export set is often SVG, which is text and halves, and a zip whose
/// compression depended on what was in it would be a rule nobody could predict.
///
/// Folders are implied by the `/` in a name, which is how the format works — no
/// directory entries are written, and every extractor creates the path.
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
                fills: vec![Fill {
                    brush: ondin_core::Brush::Solid(Color::from_rgba8(10, 20, 30, 255)),
                    visible: true,
                }],
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
            Operation::SetExports { id: rect, exports },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, frame, rect)
    }

    /// The scale is what the row promises, so the arithmetic is worth pinning:
    /// 2× of a 100×50 is 200×100, and `Width(512)` is whatever reaches 512 —
    /// *including* on the axis nobody pinned, which is the part a designer is
    /// actually asking about.
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

    /// SVG has no pixels, and a row that showed it a size would be inventing one.
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

    /// Two specs that differ only in scale are separated by the marker the scale
    /// derives; two that are *identical* have nothing to separate them and fall to
    /// the de-collision, which must not be an overwrite.
    ///
    /// **Both halves, because the marker is what replaced a typed suffix** — a
    /// plan that leaned on the user having typed `@2x` is exactly what this rules
    /// out, and a test covering only the identical case would pass against a
    /// naming rule with no marker in it at all.
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

    /// The prefix is a path, and it goes through the same sanitizer the layer's
    /// own name does — so the folder rule governs both halves or neither.
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

    /// The Windows set, which is the strict one — and the trailing dot, which is
    /// the one the filesystem accepts and then quietly drops.
    #[test]
    fn a_name_that_windows_would_refuse_is_repaired() {
        assert_eq!(sanitize_segment("a:b?c"), "a-b-c");
        assert_eq!(sanitize_segment("trailing."), "trailing");
        assert_eq!(sanitize_segment("NUL"), "_NUL");
        assert_eq!(sanitize_segment("con.png"), "_con.png");
        // Not reserved: the device names are whole segments, not prefixes.
        assert_eq!(sanitize_segment("console"), "console");
    }

    /// The slash rule is opt-in, and the off state is not "leave it alone" — a
    /// raw `/` in a path is a folder whether anyone meant it or not.
    #[test]
    fn a_slash_is_a_folder_only_when_asked_for() {
        assert_eq!(
            path_stem("icons/close", PlanOptions::default()),
            "icons-close"
        );
        assert_eq!(
            path_stem(
                "icons/close",
                PlanOptions {
                    folders_from_names: true,
                    ..PlanOptions::default()
                }
            ),
            "icons/close"
        );
        assert_eq!(path_stem("  ", PlanOptions::default()), "Layer");
    }

    /// `Frame` reaches up past a group to the enclosing artboard; a layer outside
    /// one has no frame and stays transparent. And JPEG's substitution is the
    /// separate rule: the *stored* value is untouched, the resolved one is white.
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

    /// The whole path end to end: three specs, three formats, three files that a
    /// zip tool can open.
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
        // And the bytes survive the round trip, which is what "packs" has to mean.
        use std::io::Read as _;
        let mut first = r.by_name("Button@2x.png").expect("by name");
        let mut back = Vec::new();
        first.read_to_end(&mut back).unwrap();
        assert_eq!(back, rendered[0].1);
    }
}
