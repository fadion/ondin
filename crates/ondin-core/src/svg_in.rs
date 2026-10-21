//! SVG import — markup in, a [`Transaction`] out (§7.1).
//!
//! The inverse of `ondin_export::svg`, kept in core rather than beside it: reading is
//! model construction and needs nothing but the model, so it is testable without a
//! graphics context. The two mappings can drift; the round-trip test in
//! `ondin-export/tests/svg_roundtrip.rs` is what binds them.
//!
//! The entry point is a paste: `import` takes a string and a parent, mints its own ids
//! and hands back one transaction, the same shape as every builder in [`crate::build`].
//!
//! ## Grammars
//!
//! `kurbo::BezPath::from_svg` reads `d` and `peniko::color::parse_color` reads CSS
//! colours; the XML (`roxmltree`) and the `transform` list ([`parse_transform`]) are
//! ours.
//!
//! ## What it reads
//!
//! Shapes, groups and transforms; the inherited half of the presentation attributes,
//! with an inline `style` beating them and a `<style>` rule beating a presentation
//! attribute; solid paint and both gradient kinds in both `gradientUnits`, with
//! `gradientTransform`, `spreadMethod` and `xlink:href` inheritance;
//! `fill-rule`; opacity, `display` and `visibility`; `viewBox`; `<clipPath>` and
//! `<mask>` as **mask nodes**; `<use>` and `<symbol>` as deep copies; `<image>` as a
//! rect with an image fill; `<text>` placed by its baseline; and a `<filter>` as an
//! **effect stack** — a lone `<feGaussianBlur>`, a lone `<feDropShadow>`, and the
//! silhouette-offset-blur-flood *shadow chain*, drop or inner, in any number and in
//! the order the markup lays them out (see [`FilterRead`]).
//!
//! A gradient inherits its stops and position through `xlink:href` (how Inkscape writes
//! all of them) and carries a `gradientTransform` that composes inside the units
//! mapping; skipping either yields a drawing with no gradients. See
//! [`gradient_chain`], [`Paint::mapping`] and [`transform_gradient`].
//!
//! ## What it does not read, and says so
//!
//! **[`Import::skipped`]** is what is missing: a `<filter>` primitive with no kind here
//! (an arbitrary `feColorMatrix` among them; see [`FilterRead`]), `<foreignObject>` (a
//! decided non-goal: nothing in the model draws HTML), an `<image>` whose href points
//! at a file the paste never had, a reference that resolves to nothing, and a CSS
//! selector needing sibling order, a pseudo-class or an attribute. **[`Import::approximated`]**
//! is what is there and slightly wrong: a `<mask>` whose luminance and alpha readings
//! differ, `objectBoundingBox` clip units, a stretched image, a substituted font, a
//! `<tspan>` whose own gradient or stroke was dropped, and an `feGaussianBlur` with two
//! unequal deviations, averaged. Nothing is lost silently.
//!
//! An entry on either list is a claim about the model, which can grow; re-derive an old
//! loss rather than trust it. A property read with `attribute` or `has_attribute`
//! rather than [`property`] claims the SVG never puts it in `style`, which Inkscape
//! does for the whole presentation block.
//!
//! The one structural loss is the paragraph: the writer emits one `<text>` per line, so
//! a two-line text layer comes back as two layers in the right places. Rejoining them
//! would mean inventing the line height that places every line after the first.

use crate::id::{IdSource, NodeId};
use crate::image::Brush;
use crate::node::{Fill, FillRule, MaskMode, NodeKind, Stroke, StrokeAlign, StrokeSides};
use crate::op::{Operation, Transaction};
use crate::typography::CharAttr;
use kurbo::{Affine, BezPath, Point, Size};
use peniko::Color;

/// How deeply nested an SVG's elements may be before the walk gives up; a deeper
/// subtree is skipped and reported.
///
/// Far tighter than [`crate::io::MAX_TREE_DEPTH`] on purpose: the walk recurses
/// (`children → node → node_ignoring → children`), and a clipped or masked element
/// becomes a wrapper plus a clip node, so SVG nesting multiplies in the document. A
/// tree past `MAX_TREE_DEPTH` would import and save but refuse to reopen. The XML
/// parser recurses too; [`MAX_XML_DEPTH`] bounds that.
const MAX_SVG_NESTING: usize = 64;

/// How many nodes one import may emit before the rest is skipped and reported.
///
/// [`MAX_SVG_NESTING`] bounds depth, not width: a `<defs>` of groups each
/// instantiating the next twice expands exponentially from a tiny file, and an
/// allocation failure aborts. `use_depth` counts nesting only and `nodes_limit`
/// bounds parsed XML nodes, so neither bounds the output. 50,000 sits above real
/// artwork. It bounds the importer, not the paste: `Document::apply` of that many
/// siblings is quadratic in `op_create`'s `child_names` walk.
const MAX_SVG_NODES: usize = 50_000;

/// How many CSS rules one stylesheet may declare before the rest is skipped and
/// reported.
///
/// `Css::declaration` walks every rule for every element and property, so a huge
/// sheet is cubic in the file. 2,048 leaves headroom over real exports; past it the
/// sheet is reported through [`Css::complex`] and shapes take their presentation
/// attributes instead.
const MAX_CSS_RULES: usize = 2_048;

/// What one `import` produced.
#[derive(Debug)]
pub struct Import {
    /// Every operation, in apply order: the root group first, then its subtree.
    pub tx: Transaction,
    /// The group everything was placed under, so the caller can select it.
    pub root: NodeId,
    /// The size the markup declares — its `viewBox`, else its `width`/`height`.
    /// `None` when it declares neither.
    pub size: Option<Size>,
    /// How many shapes were created. `0` is not an error here (an `<svg>` holding
    /// only a `<defs>` is valid), but a pasting caller will want to refuse it.
    pub shapes: usize,
    /// Element names met and not understood, deduplicated and in the order first
    /// met. The caller owes the user this list.
    pub skipped: Vec<String>,
    /// Features read but not exactly — a `<mask>` whose luminance and alpha readings
    /// differ, or clip units resolved in the wrong space. Distinct from
    /// [`Self::skipped`]: that says a thing is missing, this says it is slightly wrong.
    pub approximated: Vec<String>,
}

/// What the host does with the bytes an `<image>` carries.
///
/// A callback because core can do neither half: the pixel size needs decoding
/// (`ondin-render`), the id is a content hash from an app-only crate, and the bytes
/// must reach the renderer's decode cache. The caller is the same one a drop and a
/// file dialog use (`OndinApp::load_image_bytes`). `None` for bytes that will not
/// decode, which skips the `<image>` rather than failing the import.
pub trait ImageSink {
    fn place(
        &mut self,
        bytes: Vec<u8>,
        format: crate::image::ImageFormat,
    ) -> Option<(crate::image::ImageId, crate::image::ImageEntry)>;
}

/// The four characters XML (and CSS `white-space: normal`) calls whitespace.
///
/// Not `char::is_whitespace`: that is Unicode's wider `White_Space`, which would
/// collapse the no-break, figure and ideographic spaces an author types on purpose.
const XML_SPACE: [char; 4] = [' ', '\t', '\r', '\n'];

/// Append `text` to `out` with `xml:space="default"` collapsing of [`XML_SPACE`],
/// so the newlines and tabs of pretty-printed markup do not reach the drawing.
fn push_collapsed(out: &mut String, text: &str) {
    let mut space = out.ends_with(' ') || out.is_empty();
    for c in text.chars() {
        if XML_SPACE.contains(&c) {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = false;
        }
    }
}

/// The ink a resolved style fills with, its own `fill-opacity` folded in — and
/// `None` for a gradient, which a character span cannot hold.
///
/// Folding the opacity keeps it comparable with the node's own
/// [`crate::typography::TextStyle::color`], which [`Builder::text_node`] builds the same way.
fn solid_fill(s: &Style) -> Option<Color> {
    match &s.fill {
        Some(Paint::Solid(c)) => Some(c.multiply_alpha(s.fill_opacity)),
        _ => None,
    }
}

/// [`solid_fill`] at the `u8` precision a span stores it in.
///
/// `CharAttr::Color` is canonicalized to `to_rgba8` and `Spans::normalize` drops a
/// span equal to the node default, so comparing at `f32` could mint a span that then
/// vanishes.
fn ink(s: &Style) -> Option<[u8; 4]> {
    solid_fill(s).map(|c| c.to_rgba8().to_u8_array())
}

/// The bytes and format inside a `data:` URI, and `None` for anything else.
///
/// Only the base64 form, which is what exporters write. The MIME type decides the
/// format; the bytes are not sniffed.
fn data_uri(href: &str) -> Option<(Vec<u8>, crate::image::ImageFormat)> {
    use base64::Engine as _;
    let rest = href.trim().strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mime = meta.split(';').next()?.trim();
    if !meta.split(';').any(|p| p.trim() == "base64") {
        return None;
    }
    let format = match mime {
        "image/png" => crate::image::ImageFormat::Png,
        "image/jpeg" | "image/jpg" => crate::image::ImageFormat::Jpeg,
        "image/webp" => crate::image::ImageFormat::Webp,
        "image/gif" => crate::image::ImageFormat::Gif,
        _ => return None,
    };
    // Whitespace is legal in an XML attribute and not in base64.
    let clean: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(clean)
        .ok()?;
    Some((bytes, format))
}

/// Why a string could not be read as SVG at all.
///
/// Anything unreadable inside a well-formed `<svg>` is a skip, not an error.
/// [`SvgError::TooDeep`] refuses the text before a parser that would overflow the
/// stack sees it.
#[derive(Debug, PartialEq, Eq)]
pub enum SvgError {
    /// Not well-formed XML.
    NotXml,
    /// Well-formed XML whose root is not `<svg>` — HTML, a plist, a fragment.
    NotSvg,
    /// Elements nested past [`MAX_XML_DEPTH`], refused before the XML parser runs.
    TooDeep,
}

impl std::fmt::Display for SvgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SvgError::NotXml => "not well-formed XML",
            SvgError::NotSvg => "not an SVG document",
            SvgError::TooDeep => "nested too deeply to read",
        })
    }
}

/// How deeply the markup may nest before [`import`] refuses it unparsed.
///
/// `roxmltree` recurses and has no depth option, so deeply nested `<g>` overflowed
/// the 1 MiB main-thread stack (an uncatchable abort) before any builder budget was
/// consulted; [`nests_deeper_than`] reads the depth off the text first. The parser
/// alone survives ~170 levels in debug, so 96 keeps room for the egui frame the paste
/// runs under. It is 1.5x [`MAX_SVG_NESTING`]: a document 65–96 deep still imports
/// with the deeper subtrees skipped and reported.
pub const MAX_XML_DEPTH: usize = 96;

/// Whether `svg`'s elements nest deeper than `limit`, read from the text without
/// parsing it.
///
/// Conservative in the direction that refuses. Comments, CDATA, processing
/// instructions and quoted attribute values are stepped over. A `<!DOCTYPE>`
/// internal subset can declare an entity whose value is markup and `roxmltree`
/// expands references ten deep, so the deepest markup in any quoted doctype string is
/// counted ten times on top of the document's depth. Malformed text answers `false`
/// and is left to the parser to refuse.
fn nests_deeper_than(svg: &str, limit: usize) -> bool {
    /// `roxmltree`'s own entity-reference depth (its `EntityReferenceLoop`).
    const ENTITY_DEPTH: usize = 10;
    let b = svg.as_bytes();
    let find = |from: usize, needle: &[u8]| -> Option<usize> {
        b.get(from..)?
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|i| from + i)
    };
    let mut depth = 0usize;
    let mut entity_depth = 0usize;
    let mut i = 0usize;
    while let Some(off) = b.get(i..).and_then(|r| r.iter().position(|&c| c == b'<')) {
        i += off;
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            let Some(end) = find(i + 4, b"-->") else {
                return false;
            };
            i = end + 3;
        } else if rest.starts_with(b"<![CDATA[") {
            let Some(end) = find(i + 9, b"]]>") else {
                return false;
            };
            i = end + 3;
        } else if rest.starts_with(b"<?") {
            let Some(end) = find(i + 2, b"?>") else {
                return false;
            };
            i = end + 2;
        } else if rest.starts_with(b"<!") {
            // A doctype, possibly with an internal subset in brackets; a quoted
            // string in it may be an entity value holding markup.
            let mut j = i + 2;
            let mut brackets = 0usize;
            loop {
                match b.get(j) {
                    None => return false,
                    Some(b'[') => brackets += 1,
                    Some(b']') => brackets = brackets.saturating_sub(1),
                    Some(b'>') if brackets == 0 => break,
                    Some(&q @ (b'"' | b'\'')) => {
                        let Some(close) =
                            b.get(j + 1..).and_then(|r| r.iter().position(|&c| c == q))
                        else {
                            return false;
                        };
                        let value = &svg[j + 1..j + 1 + close];
                        entity_depth = entity_depth.max(markup_depth(value));
                        j += close + 1;
                    }
                    Some(_) => {}
                }
                j += 1;
            }
            i = j + 1;
        } else {
            let closing = rest.starts_with(b"</");
            // Step to the tag's `>`, over quoted attribute values.
            let mut j = i + 1;
            let mut quote = None;
            loop {
                match (b.get(j), quote) {
                    (None, _) => return false,
                    (Some(&c), Some(q)) if c == q => quote = None,
                    (Some(_), Some(_)) => {}
                    (Some(&c @ (b'"' | b'\'')), None) => quote = Some(c),
                    (Some(b'>'), None) => break,
                    (Some(_), None) => {}
                }
                j += 1;
            }
            if closing {
                depth = depth.saturating_sub(1);
            } else if b[j - 1] != b'/' {
                depth += 1;
                if depth + ENTITY_DEPTH * entity_depth > limit {
                    return true;
                }
            }
            i = j + 1;
        }
    }
    false
}

/// The deepest element nesting in a fragment of markup — an entity value — by
/// the same stepping [`nests_deeper_than`] does, without the doctype arm (an
/// entity value cannot hold one).
fn markup_depth(fragment: &str) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    let b = fragment.as_bytes();
    let mut i = 0;
    while let Some(off) = b.get(i..).and_then(|r| r.iter().position(|&c| c == b'<')) {
        i += off;
        let closing = b.get(i + 1) == Some(&b'/');
        let Some(end) = b.get(i..).and_then(|r| r.iter().position(|&c| c == b'>')) else {
            break;
        };
        let gt = i + end;
        if closing {
            depth = depth.saturating_sub(1);
        } else if b
            .get(i + 1)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_' || *c == b':')
            && b[gt - 1] != b'/'
        {
            depth += 1;
            deepest = deepest.max(depth);
        }
        i = gt + 1;
    }
    deepest
}

impl std::error::Error for SvgError {}

/// A cheap, non-allocating "is this worth parsing at all", asked of every clipboard
/// string: does it start (after any prolog) with `<svg` or `<!DOCTYPE svg`. Not a
/// parse; [`import`] does that and reports why it fails. `app::looks_like_svg` is
/// only the byte-to-`str` adapter, so paste and drop share this one answer.
pub fn looks_like_svg(s: &str) -> bool {
    let head = before_root(s);
    head.starts_with("<svg") || head.starts_with("<!DOCTYPE svg")
}

/// `s` with everything that may legally precede the root element removed.
///
/// Skips an `<?xml …?>` declaration and any leading comments (Illustrator writes a
/// generator comment before the doctype).
///
/// A byte-order mark is still refused: it would reach `roxmltree` as well, so
/// admitting one here only trades the refusal for a parser error.
fn before_root(s: &str) -> &str {
    let s = s.trim_start();
    let mut head = s.strip_prefix("<?xml").map_or(s, |rest| {
        rest.find("?>")
            .map_or(rest, |i| &rest[i + 2..])
            .trim_start()
    });
    // A loop: a generator comment and a licence comment in a row is ordinary.
    while let Some(rest) = head.strip_prefix("<!--") {
        // Unterminated: answering `rest` would let `<!--<svg/>` read as a drawing.
        let Some(i) = rest.find("-->") else {
            return "";
        };
        head = rest[i + 3..].trim_start();
    }
    head
}

/// Read `svg` and build the nodes it describes under `parent`.
///
/// The subtree is wrapped in one `Group` whichever way the markup is shaped: a paste
/// is one undo step and one selection, and the `<svg>` root's transform and `viewBox`
/// need a container to live on.
pub fn import<'s>(
    svg: &str,
    ids: &'s mut IdSource,
    parent: NodeId,
    index: usize,
    // `None` where the caller has nowhere to put pictures; an `<image>` is then skipped.
    images: Option<&'s mut dyn ImageSink>,
) -> Result<Import, SvgError> {
    // `allow_dtd`: roxmltree refuses a `<!DOCTYPE>` by default and SVG 1.1 files open
    // with one. Safe because roxmltree bounds entity expansion and never resolves an
    // external entity; `nodes_limit` is the second belt on untrusted clipboard input.
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 500_000,
    };
    // Depth first: `nodes_limit` bounds how many nodes, never how deep.
    if nests_deeper_than(svg, MAX_XML_DEPTH) {
        return Err(SvgError::TooDeep);
    }
    let doc = roxmltree::Document::parse_with_options(svg, opts).map_err(|_| SvgError::NotXml)?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err(SvgError::NotSvg);
    }

    let size = declared_size(root);
    // Declared above `out`: the CSS rules inside `out.gradients` borrow these strings.
    let sheets = style_sheets(&doc);
    let mut out = Builder {
        ids,
        ops: Vec::new(),
        created: 0,
        child_counts: rustc_hash::FxHashMap::default(),
        shapes: 0,
        skipped: Vec::new(),
        approximated: Vec::new(),
        gradients: Gradients::collect(&doc, &sheets),
        in_clip: false,
        use_depth: 0,
        depth: 0,
        images,
        added: Vec::new(),
    };

    let group = out.ids.mint();
    // Through `push` like every other op so the `ops` invariant has no exemption.
    out.push(Operation::CreateNode {
        id: group,
        parent,
        index,
        kind: NodeKind::Group,
        // The `viewBox` is a transform only; with no `width`/`height` it is the identity.
        transform: Some(view_transform(root)),
        name: element_name(root),
    });

    // The root's own presentation attributes count: Figma writes `<svg fill="none">`,
    // and starting from SVG's initial values would fill every child black.
    let inherited = resolve(root, &Style::initial(), &out.gradients);
    // Reported once for the sheet, not once per element it should have matched.
    if out.gradients.css.complex {
        out.skip("style (complex selector)");
    }
    // A resampled stop list is there and slightly wrong: `approximate`, not `skip`.
    if out.gradients.resampled {
        out.approximate("gradient (too many stops)");
    }
    // A pattern is not there (the shape takes its inherited fill): `skip`.
    if out.gradients.patterns {
        out.skip("pattern (image fill)");
    }
    out.children(root, group, &inherited);
    // After the walk, because an unreadable `fill` is a fact about an element, not the
    // defs; hence the `Cell`. The paint is absent, so `skip`.
    if out.gradients.unreadable_paint.get() {
        out.skip("fill or stroke (unreadable value)");
    }

    let Builder {
        ops,
        shapes,
        skipped,
        approximated,
        ..
    } = out;
    Ok(Import {
        tx: Transaction(ops),
        root: group,
        size,
        shapes,
        skipped,
        approximated,
    })
}

/// The `width`/`height` the markup claims, or its `viewBox`'s.
///
/// `viewBox` second: the caller wants the size the drawing will occupy, which is
/// `width`/`height`, and [`view_transform`] makes the drawing agree.
fn declared_size(root: roxmltree::Node<'_, '_>) -> Option<Size> {
    if let Some(s) = declared_pair(root) {
        return Some(s);
    }
    view_box(root).map(|r| r.size())
}

/// The layer name an element carries: `data-name` if it has one, else `id`.
///
/// `ondin_export::svg::name_attr` writes `data-name`, so this is the other half of
/// the pair; `id` is the fallback for Illustrator and Inkscape files. One function so
/// no element kind misses the attribute.
fn element_name(el: roxmltree::Node<'_, '_>) -> Option<String> {
    // Through `naming::usable`, like the rename fields; `None` makes `op_create`
    // generate a name, as for an element with no `id`.
    el.attribute("data-name")
        .or_else(|| el.attribute("id"))
        .and_then(crate::naming::usable)
}

/// The `width`/`height` pair the root declares, or `None` when it declares nothing
/// positive. Shared by [`declared_size`] and [`view_transform`] so a zero or negative
/// size cannot produce a singular or flipped transform.
fn declared_pair(root: roxmltree::Node<'_, '_>) -> Option<Size> {
    let w = root.attribute("width").and_then(parse_len)?;
    let h = root.attribute("height").and_then(parse_len)?;
    (w > 0.0 && h > 0.0).then(|| Size::new(w, h))
}

fn view_box(root: roxmltree::Node<'_, '_>) -> Option<kurbo::Rect> {
    let v = root.attribute("viewBox")?;
    let n: Vec<f64> = numbers(v);
    match n.as_slice() {
        [x, y, w, h] if *w > 0.0 && *h > 0.0 => Some(kurbo::Rect::new(*x, *y, *x + *w, *y + *h)),
        _ => None,
    }
}

/// The `viewBox` → `width`/`height` mapping, as one affine.
///
/// `preserveAspectRatio` is not read; the default `xMidYMid meet` (fit, centred) is
/// what this computes, so only markup that deliberately distorts itself differs.
fn view_transform(root: roxmltree::Node<'_, '_>) -> Affine {
    let Some(vb) = view_box(root) else {
        return Affine::IDENTITY;
    };
    let Some(size) = declared_pair(root) else {
        // No box to fit into (a non-positive size lands here too): the drawing's own
        // units are the space and the viewBox only gives its origin.
        return Affine::translate((-vb.x0, -vb.y0));
    };
    let (w, h) = (size.width, size.height);
    let s = (w / vb.width()).min(h / vb.height());
    let dx = (w - vb.width() * s) * 0.5;
    let dy = (h - vb.height() * s) * 0.5;
    Affine::translate((dx, dy)) * Affine::scale(s) * Affine::translate((-vb.x0, -vb.y0))
}

/// The inherited half of SVG's presentation attributes.
///
/// `fill`, `stroke`, the stroke's shape properties and `fill-rule` inherit; `opacity`,
/// `transform` and `display` do not (a `Group` node applies them to its subtree), so
/// those are read per element.
#[derive(Clone)]
struct Style {
    fill: Option<Paint>,
    stroke: Option<Paint>,
    stroke_width: f64,
    stroke_cap: kurbo::Cap,
    stroke_join: kurbo::Join,
    dashes: Vec<f64>,
    dash_offset: f64,
    miter_limit: f64,
    fill_rule: FillRule,
    fill_opacity: f32,
    stroke_opacity: f32,
    /// `currentColor`'s value, which is what the `color` property is for.
    current: Color,
    /// The font properties inherit like paint (Inkscape puts `font-family` on a `<g>`,
    /// and a `<tspan>` may set its own size). The family is the raw list, unresolved:
    /// picking from it needs the machine's fonts and reporting a miss is not pure.
    font_family: Option<String>,
    font_size: f64,
    weight: u16,
    italic: bool,
    anchor: Anchor,
}

/// One item of a `<text>`'s flattened content — see [`Builder::text_pieces`].
///
/// Pieces are text nodes, each with the innermost element it came from and that
/// element's resolved style, so `<tspan>x<tspan>y</tspan>z</tspan>` is three pieces.
enum TextPiece<'d, 'input> {
    /// Characters, the element they came from, and its resolved style.
    Run(&'d str, roxmltree::Node<'d, 'input>, Style),
    /// A positioned `<tspan>`, which ends the run so far and starts a node of its
    /// own. Emitted before the pieces inside it.
    Break(roxmltree::Node<'d, 'input>, Style),
}

/// `text-anchor`, which is inherited and therefore lives on [`Style`].
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Anchor {
    #[default]
    Start,
    Middle,
    End,
}

/// A paint before its opacity is folded in — a colour, or a reference to a gradient.
#[derive(Clone)]
enum Paint {
    Solid(Color),
    /// A gradient, the space its coordinates are written in, and its
    /// `gradientTransform`.
    ///
    /// The transform is carried, not baked in: it composes inside the
    /// [`GradientUnits`] mapping, unknown until the painted shape is — see [`Paint::brush`].
    /// The gradient is boxed so every `Solid` does not pay for its inline stops.
    Gradient(Box<peniko::Gradient>, GradientUnits, Affine),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GradientUnits {
    User,
    /// The default, and the one that needs the shape's own box to resolve.
    ObjectBounding,
}

impl Style {
    /// SVG's initial values: an element with no `fill` anywhere above it is black.
    fn initial() -> Self {
        Self {
            fill: Some(Paint::Solid(Color::BLACK)),
            stroke: None,
            stroke_width: 1.0,
            stroke_cap: kurbo::Cap::Butt,
            stroke_join: kurbo::Join::Miter,
            dashes: Vec::new(),
            dash_offset: 0.0,
            miter_limit: 4.0,
            fill_rule: FillRule::NonZero,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            current: Color::BLACK,
            // SVG's `medium` is 16, which is also the model's default.
            font_family: None,
            font_size: 16.0,
            weight: 400,
            italic: false,
            anchor: Anchor::Start,
        }
    }
}

/// One element's presentation property, read from an inline `style="…"`, then a
/// `<style>` rule ([`Css`]), then the attribute: CSS's own order, and what makes an
/// Illustrator export's `class="cls-1"` decide.
fn property<'a>(el: roxmltree::Node<'a, '_>, name: &str, css: &Css<'a>) -> Option<&'a str> {
    // No `?` on the `style` attribute: it would return `None` for the whole property
    // instead of falling through to the sheet and the attribute.
    if let Some(style) = el.attribute("style")
        && let Some(v) = declaration(style, name)
    {
        return Some(v);
    }
    css.declaration(el, name).or_else(|| el.attribute(name))
}

/// One `name: value` out of a `;`-separated declaration block.
///
/// The returned slice borrows from the document, so nothing is copied.
fn declaration<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    block.split(';').find_map(|decl| {
        let (k, v) = decl.split_once(':')?;
        // `!important` is stripped, not parsed: there is no important ordering here,
        // and left on it makes a good value unreadable.
        let v = v.trim();
        let v = v
            .strip_suffix("!important")
            .map_or(v, |head| head.trim_end())
            .trim_end_matches(|c: char| c.is_whitespace());
        (k.trim() == name).then_some(v)
    })
}

/// The document's `<style>` rules: a small cascade engine over compounds of type,
/// class, id and `*`, joined by descendant and child combinators.
///
/// Selectors needing more than an ancestor walk (`+`, `~`, `:pseudo`, `[attr]`) are
/// counted and ignored rather than half-matched, as is a malformed one: repairing
/// `.a > > .b` into `.a > .b` would apply a wider rule than was written.
#[derive(Default)]
struct Css<'a> {
    /// In document order, which is the tiebreak when specificity ties.
    rules: Vec<(Sel<'a>, u32, &'a str)>,
    /// Whether any selector was too complex to read, so the import can say so.
    complex: bool,
}

/// The simple selectors that must all match one element — `rect.a#b`, or `*`.
/// `tag: None` is the universal selector, which adds no specificity.
#[derive(Default)]
struct Compound<'a> {
    tag: Option<&'a str>,
    classes: Vec<&'a str>,
    id: Option<&'a str>,
}

impl Compound<'_> {
    fn matches(&self, el: roxmltree::Node<'_, '_>) -> bool {
        if let Some(t) = self.tag
            && t != el.tag_name().name()
        {
            return false;
        }
        if let Some(i) = self.id
            && el.attribute("id") != Some(i)
        {
            return false;
        }
        if self.classes.is_empty() {
            return true;
        }
        let have: Vec<&str> = el
            .attribute("class")
            .map(|c| c.split_whitespace().collect())
            .unwrap_or_default();
        self.classes.iter().all(|c| have.contains(c))
    }

    /// `(ids, classes, types)`, CSS's three-part specificity for this compound.
    fn specificity(&self) -> (u32, u32, u32) {
        (
            u32::from(self.id.is_some()),
            self.classes.len() as u32,
            u32::from(self.tag.is_some()),
        )
    }
}

/// How a compound is joined to the one on its right.
enum Combinator {
    /// `.a .b` — any ancestor.
    Descendant,
    /// `.a > .b` — the immediate parent.
    Child,
}

/// A selector: the compound the element itself must match, and the chain of
/// ancestors to its left. `chain` runs right to left, so `chain[0]` is immediately
/// left of the subject; the subject is the cheap filter most rules fail on.
struct Sel<'a> {
    subject: Compound<'a>,
    chain: Vec<(Combinator, Compound<'a>)>,
}

impl Sel<'_> {
    fn matches(&self, el: roxmltree::Node<'_, '_>) -> bool {
        self.subject.matches(el) && match_chain(&self.chain, el)
    }

    /// Specificity as one number on a 100/10/1 scale; a selector with more than nine
    /// classes would carry into the next place, which real stylesheets do not reach.
    fn specificity(&self) -> u32 {
        let (mut i, mut c, mut t) = self.subject.specificity();
        for (_, comp) in &self.chain {
            let (ci, cc, ct) = comp.specificity();
            i += ci;
            c += cc;
            t += ct;
        }
        i * 100 + c * 10 + t
    }
}

/// Whether `el`'s ancestors satisfy `chain`, which runs right to left.
///
/// A descendant combinator must backtrack (`.a > .b .c` fails on the nearest `.b`
/// while a `.b` further up has `.a` as its parent), but recursion over ancestors is
/// exponential on a tiny file. An element's ancestors are a path, so this is
/// subsequence matching as a table over `(chain index, ancestor index)`, in
/// `O(chain × depth)`. `g(i, j)` below is "can `chain[i..]` be satisfied using
/// ancestors from the `j`-th upwards", and the answer is `g(0, 0)`.
fn match_chain(chain: &[(Combinator, Compound<'_>)], el: roxmltree::Node<'_, '_>) -> bool {
    if chain.is_empty() {
        return true;
    }
    // Nearest ancestor first, so `path[0]` is the parent.
    let mut path: Vec<roxmltree::Node<'_, '_>> = Vec::new();
    let mut cur = el.parent_element();
    while let Some(p) = cur {
        path.push(p);
        cur = p.parent_element();
    }
    let n = path.len();
    // `row[j]` is `g(i + 1, j)`, seeded with `g(chain.len(), _) = true`: an empty
    // remainder is satisfied wherever it is asked.
    let mut row = vec![true; n + 1];
    for (comb, comp) in chain.iter().rev() {
        let mut next = vec![false; n + 1];
        match comb {
            // The parent and nothing else, so there is one `j` to consider.
            Combinator::Child => {
                for j in 0..n {
                    next[j] = comp.matches(path[j]) && row[j + 1];
                }
            }
            // Any ancestor at or above `j`. Accumulated from the top down, which
            // is what turns the old inner loop into one pass.
            Combinator::Descendant => {
                let mut acc = false;
                for j in (0..n).rev() {
                    acc = acc || (comp.matches(path[j]) && row[j + 1]);
                    next[j] = acc;
                }
            }
        }
        row = next;
    }
    row[0]
}

impl<'a> Css<'a> {
    /// The winning declaration for `name` on `el`, or `None` if no rule matches.
    ///
    /// Specificity is summed across the whole selector, one id beating any number of
    /// classes. The ancestor walk in `Sel::matches` is deliberately last, so most
    /// rules fail the cheap tests first.
    fn declaration(&self, el: roxmltree::Node<'_, '_>, name: &str) -> Option<&'a str> {
        let mut best: Option<(u32, &'a str)> = None;
        for (sel, spec, block) in &self.rules {
            // `>=`: a later rule of equal specificity wins.
            if best.is_none_or(|(s, _)| *spec >= s)
                && let Some(v) = declaration(block, name)
                && sel.matches(el)
            {
                best = Some((*spec, v));
            }
        }
        best.map(|(_, v)| v)
    }
}

/// One compound selector — `rect`, `.a`, `#b`, `*`, or any of those run together —
/// or `None` if it holds anything this reader does not do.
///
/// An empty compound is `None`, not `*`: `.a > > .b` tokenizes to one, and reading it
/// as `*` would silently widen the rule.
fn parse_compound(s: &str) -> Option<Compound<'_>> {
    if s.is_empty() {
        return None;
    }
    let name_ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    };
    let mut out = Compound::default();
    // The leading run, before any `.` or `#`, is the element name.
    let head_len = s.find(['.', '#']).unwrap_or(s.len());
    let (head, mut rest) = s.split_at(head_len);
    match head {
        "" => {}
        "*" => {}
        t if name_ok(t) => out.tag = Some(t),
        _ => return None,
    }
    while !rest.is_empty() {
        let (kind, tail) = rest.split_at(1);
        let len = tail.find(['.', '#']).unwrap_or(tail.len());
        let (name, next) = tail.split_at(len);
        if !name_ok(name) {
            return None;
        }
        match kind {
            "." => out.classes.push(name),
            "#" if out.id.is_none() => out.id = Some(name),
            // Two ids on one compound match nothing; reporting beats pretending.
            _ => return None,
        }
        rest = next;
    }
    Some(out)
}

/// A whole selector: compounds joined by whitespace or `>`. `+`, `~`, `:pseudo` and
/// `[attr]` come back `None` so the import reports them.
fn parse_selector(sel: &str) -> Option<Sel<'_>> {
    if sel.contains(['+', '~', '[', ']', ':', '(', ')', ',']) {
        return None;
    }
    // Whitespace is a descendant combinator unless a `>` is adjacent, so the
    // combinator is decided when the next compound starts.
    let mut parts: Vec<(Combinator, &str)> = Vec::new();
    let mut pending = Combinator::Descendant;
    let mut first: Option<&str> = None;
    for token in sel.split_whitespace().flat_map(|w| {
        // `a>b` has no spaces, so `>` splits within a word too; the empties a
        // leading or trailing `>` produces are refused by `parse_compound`.
        w.split_inclusive('>')
    }) {
        let (text, is_child) = match token.strip_suffix('>') {
            Some(t) => (t, true),
            None => (token, false),
        };
        if !text.is_empty() {
            if first.is_none() {
                first = Some(text);
            } else {
                parts.push((pending, text));
            }
            pending = Combinator::Descendant;
        } else if !is_child && first.is_none() {
            return None;
        }
        if is_child {
            // A leading `>`, or two in a row, is not a selector.
            first?;
            if matches!(pending, Combinator::Child) {
                return None;
            }
            pending = Combinator::Child;
        }
    }
    // A trailing `>` leaves a combinator with no compound after it.
    if matches!(pending, Combinator::Child) {
        return None;
    }
    let subject_text = *parts.last().map(|(_, t)| t).unwrap_or(&first?);
    let subject = parse_compound(subject_text)?;
    // `chain` runs right to left: the compound left of the subject first.
    let mut chain = Vec::new();
    let lefts = first.into_iter().chain(parts.iter().map(|(_, t)| *t));
    let combs = parts.iter().map(|(c, _)| c);
    let texts: Vec<&str> = lefts.collect();
    for (i, comb) in combs.enumerate().rev() {
        let comb = match comb {
            Combinator::Child => Combinator::Child,
            Combinator::Descendant => Combinator::Descendant,
        };
        chain.push((comb, parse_compound(texts[i])?));
    }
    Some(Sel { subject, chain })
}

/// Split a stylesheet into rules, reporting whether anything was too complex to read.
fn parse_css(text: &str) -> (Vec<(Sel<'_>, u32, &str)>, bool) {
    let mut rules = Vec::new();
    let mut complex = false;
    for chunk in text.split('}') {
        let Some((sels, block)) = chunk.split_once('{') else {
            continue;
        };
        // A rule with a CSS comment is dropped and reported: every value is a slice of
        // the document, so stripping comments would need a copy this cannot outlive.
        if sels.contains("/*") || block.contains("/*") || block.contains("*/") {
            complex = true;
            continue;
        }
        for sel in sels.split(',') {
            let sel = sel.trim();
            if sel.is_empty() {
                continue;
            }
            // The sheet's length is bounded; the rules already read still apply.
            if rules.len() >= MAX_CSS_RULES {
                complex = true;
                break;
            }
            match parse_selector(sel) {
                Some(s) => {
                    let spec = s.specificity();
                    rules.push((s, spec, block));
                }
                None => complex = true,
            }
        }
    }
    (rules, complex)
}

struct Builder<'a, 'd, 'input> {
    ids: &'a mut IdSource,
    /// Append only through [`Self::push`], which keeps [`Self::child_counts`] true; a
    /// bare `ops.push` of a `CreateNode` makes [`Self::index_in`] answer one low and
    /// the children arrive in the wrong order.
    ops: Vec<Operation>,
    /// How many nodes this import has emitted, against [`MAX_SVG_NODES`]. Separate
    /// from [`Self::shapes`], which counts only what a user would call a shape.
    created: usize,
    /// How many children each parent has been given so far, so [`Self::index_in`]
    /// is a lookup rather than a quadratic scan of `ops`.
    child_counts: rustc_hash::FxHashMap<NodeId, usize>,
    shapes: usize,
    skipped: Vec<String>,
    approximated: Vec<String>,
    gradients: Gradients<'d, 'input>,
    /// Whether the walk is inside a `<clipPath>`, where the fill rule comes from
    /// `clip-rule`. A flag because it holds for a subtree and threading a parameter
    /// would touch every arm of [`Self::node`].
    in_clip: bool,
    /// How deep the walk is inside `<use>` instances, so a reference cycle stops.
    use_depth: usize,
    /// How deep the walk is inside nested elements; see [`MAX_SVG_NESTING`].
    depth: usize,
    /// Where an `<image>`'s bytes go — see [`ImageSink`].
    images: Option<&'a mut dyn ImageSink>,
    /// Which pictures already have an `AddImage` in this transaction.
    added: Vec<crate::image::ImageId>,
}

impl<'d, 'input> Builder<'_, 'd, 'input> {
    fn skip(&mut self, name: &str) {
        if !self.skipped.iter().any(|s| s == name) {
            self.skipped.push(name.to_string());
        }
    }

    /// Report a feature read but not exactly: unlike [`Self::skip`], it is there and
    /// slightly wrong.
    fn approximate(&mut self, name: &str) {
        if !self.approximated.iter().any(|s| s == name) {
            self.approximated.push(name.to_string());
        }
    }

    /// Walk `el`'s element children into `parent`, in document order (paint order,
    /// which is already this app's bottom-to-top child order).
    ///
    /// The one choke point of the element recursion, so the depth bound lives here;
    /// past it the subtree is skipped and reported.
    fn children(&mut self, el: roxmltree::Node<'d, 'input>, parent: NodeId, inherited: &Style) {
        if self.depth >= MAX_SVG_NESTING {
            self.skip("deeply nested elements");
            return;
        }
        self.depth += 1;
        for child in el.children().filter(roxmltree::Node::is_element) {
            self.node(child, parent, inherited);
        }
        self.depth -= 1;
    }

    /// One element wrapped in a group with its clip beneath it.
    ///
    /// Our mask node clips the run of siblings above it, so a wrapper holding
    /// `[clip, content]` is exactly SVG's `clip-path`.
    ///
    /// The transform goes on the clip, not the wrapper: `userSpaceOnUse` resolves in the
    /// referencing element's space, which includes its own `transform`, so the wrapper
    /// stays at the identity and both children carry `el`'s transform.
    ///
    /// `clip-path` is [`MaskMode::Shape`] and `<mask>` is [`MaskMode::Alpha`]. SVG's mask
    /// is luminance, which agrees only for opaque white; [`luminance_equivalent`]
    /// decides whether that is reported.
    #[allow(clippy::too_many_arguments)]
    fn masked(
        &mut self,
        el: roxmltree::Node<'d, 'input>,
        def: roxmltree::Node<'d, 'input>,
        mode: MaskMode,
        attr: &str,
        parent: NodeId,
        inherited: &Style,
        ignore: &[&str],
    ) -> Option<NodeId> {
        // A container clips itself rather than getting a wrapper (Figma's outermost
        // `<g>` carries `clip-path`); its own first child clips the run above it and
        // needs no transform. Not when the other clip attribute is still to be
        // consumed: two masks in one parent are two runs, so one would do nothing.
        let other = if attr == "clip-path" {
            "mask"
        } else {
            "clip-path"
        };
        let hoist = matches!(el.tag_name().name(), "g" | "a" | "svg")
            && !(el.has_attribute(other) && !ignore.contains(&other));

        let mut ig: Vec<&str> = ignore.to_vec();
        ig.push(attr);

        let index = self.index_in(parent);
        let wrapper = if hoist {
            // The element builds first; `CreateNode`'s index inserts, so the clip at
            // index 0 goes beneath its existing children.
            self.node_ignoring(el, parent, inherited, &ig)?
        } else {
            let wrapper = self.ids.mint();
            self.push(Operation::CreateNode {
                id: wrapper,
                parent,
                index,
                kind: NodeKind::Group,
                transform: None,
                name: element_name(el),
            });
            wrapper
        };

        // `objectBoundingBox` units are read as user space and reported: resolving
        // them needs the referencing element's bounds, which do not exist yet.
        let units = if attr == "clip-path" {
            "clipPathUnits"
        } else {
            "maskContentUnits"
        };
        if def.attribute(units) == Some("objectBoundingBox") {
            self.approximate(attr);
        }

        let clip = self.ids.mint();
        self.push(Operation::CreateNode {
            id: clip,
            parent: wrapper,
            index: 0,
            kind: NodeKind::Group,
            // Hoisted, the clip is already in the element's space; wrapped, it carries
            // the element's transform.
            transform: (!hoist).then(|| element_transform(el)),
            // Named, so it is not "Group 4" in the layer tree.
            name: Some(if attr == "clip-path" { "Clip" } else { "Mask" }.to_string()),
        });
        self.push(Operation::SetMask {
            id: clip,
            mask: true,
        });
        if mode != MaskMode::default() {
            self.push(Operation::SetMaskMode { id: clip, mode });
        }

        // The clip's contents inherit from the def, not the element it clips (a
        // `<mask>` conventionally carries `fill="white"` itself).
        let seed = resolve(def, &Style::initial(), &self.gradients);
        let was = self.in_clip;
        self.in_clip = mode == MaskMode::Shape;
        self.children(def, clip, &seed);
        self.in_clip = was;

        if mode == MaskMode::Alpha && !luminance_equivalent(def, &self.gradients.css) {
            self.approximate("mask");
        }

        if !hoist {
            self.node_ignoring(el, wrapper, inherited, &ig);
        }
        Some(wrapper)
    }

    /// The one way an operation joins the transaction, so [`Self::child_counts`]
    /// cannot fall out of step with [`Self::ops`].
    fn push(&mut self, op: Operation) {
        if let Operation::CreateNode { parent, .. } = &op {
            *self.child_counts.entry(*parent).or_insert(0) += 1;
            self.created += 1;
        }
        self.ops.push(op);
    }

    /// Whether the node budget is spent — see [`MAX_SVG_NODES`]. Asked at the top of
    /// [`Self::node_ignoring`] rather than at each push, so the last element built is
    /// whole.
    fn out_of_budget(&mut self) -> bool {
        if self.created >= MAX_SVG_NODES {
            self.skip("too many elements");
            return true;
        }
        false
    }

    /// The next child index under `parent`, counted from the ops built so far.
    ///
    /// Read before the `CreateNode` it indexes, never after: [`Self::push`] bumps the
    /// count.
    fn index_in(&self, parent: NodeId) -> usize {
        self.child_counts.get(&parent).copied().unwrap_or(0)
    }

    fn node(&mut self, el: roxmltree::Node<'d, 'input>, parent: NodeId, inherited: &Style) {
        self.node_ignoring(el, parent, inherited, &[]);
    }

    /// [`Self::node`], with the clip attributes a wrapper has already consumed.
    ///
    /// `ignore` stops the recursion: [`Self::masked`] builds the element again inside
    /// its wrapper and it still carries the attribute that sent it there.
    fn node_ignoring(
        &mut self,
        el: roxmltree::Node<'d, 'input>,
        parent: NodeId,
        inherited: &Style,
        ignore: &[&str],
    ) -> Option<NodeId> {
        if self.out_of_budget() {
            return None;
        }
        let mut style = resolve(el, inherited, &self.gradients);
        // Inside a `<clipPath>` the rule is `clip-rule`, not `fill-rule`. Applied here
        // rather than in `resolve` because it depends on where the walk is; the
        // overridden style descends, so a `<g>`'s `clip-rule` inherits.
        if self.in_clip
            && let Some(v) = property(el, "clip-rule", &self.gradients.css)
        {
            style.fill_rule = if v.trim() == "evenodd" {
                FillRule::EvenOdd
            } else {
                FillRule::NonZero
            };
        }
        // A clip or a mask becomes a mask node in a wrapper. `clip-path` goes first:
        // an element with both is clipped, then masked, one wrapper each. Both are
        // read as properties, since `style="clip-path:url(#c)"` is legal.
        for (attr, mode) in [("clip-path", MaskMode::Shape), ("mask", MaskMode::Alpha)] {
            if ignore.contains(&attr) {
                continue;
            }
            // `none` is the initial value; reporting it as a dropped clip would be noise.
            let Some(value) =
                property(el, attr, &self.gradients.css).filter(|v| v.trim() != "none")
            else {
                continue;
            };
            if let Some(def) = self.gradients.clip(value) {
                return self.masked(el, def, mode, attr, parent, inherited, ignore);
            }
            // SVG does not render an element with an unresolvable `clip-path`; drawing
            // it unclipped is friendlier, so it is reported rather than obeyed.
            self.skip(attr);
        }
        let index = self.index_in(parent);

        match el.tag_name().name() {
            // Containers. `<a>` is a group; its href is not held.
            "g" | "a" | "svg" => {
                let id = self.ids.mint();
                self.push(Operation::CreateNode {
                    id,
                    parent,
                    index,
                    kind: NodeKind::Group,
                    transform: Some(element_transform(el)),
                    name: element_name(el),
                });
                self.chrome(el, id, style.current, None);
                self.children(el, id, &style);
                Some(id)
            }
            // Definitions are read by lookup, never walked: `<defs>` and `<symbol>`
            // (drawn only through a `<use>`) must not paste as shapes, and `<style>` is
            // read up front by `Gradients::collect`. `clipPath` and `mask` are
            // definitions wherever they sit, already registered by id; reaching the
            // unknown-element arm would report a used clip as skipped.
            "defs" | "title" | "desc" | "metadata" | "symbol" | "style" | "clipPath" | "mask" => {
                None
            }
            // `<use>` is a deep copy: a group with the use's transform and `x`/`y`
            // around the same walk that built the original. `width`/`height` on a
            // `<symbol>` or `<svg>` target would set a viewport and are ignored, but
            // reported.
            "use" => {
                let href = el
                    .attribute("href")
                    .or_else(|| el.attribute(("http://www.w3.org/1999/xlink", "href")));
                let Some(target) = href.and_then(|h| self.gradients.element(h)) else {
                    self.skip("use (unresolved)");
                    return None;
                };
                // A `<use>` can reference something that contains it; a depth cap is
                // the cheap cycle check, and nothing real nests instances 16 deep.
                if self.use_depth >= 16 {
                    self.skip("use (recursive)");
                    return None;
                }
                if (el.has_attribute("width") || el.has_attribute("height"))
                    && matches!(target.tag_name().name(), "symbol" | "svg")
                {
                    self.approximate("use (sized)");
                }
                let x = el.attribute("x").and_then(parse_len).unwrap_or(0.0);
                let y = el.attribute("y").and_then(parse_len).unwrap_or(0.0);
                let id = self.ids.mint();
                self.push(Operation::CreateNode {
                    id,
                    parent,
                    index,
                    kind: NodeKind::Group,
                    transform: Some(element_transform(el) * Affine::translate((x, y))),
                    name: element_name(el),
                });
                self.chrome(el, id, style.current, None);
                self.use_depth += 1;
                // A `<symbol>` contributes its contents; anything else, itself.
                if matches!(target.tag_name().name(), "symbol" | "svg") {
                    let inner = resolve(target, &style, &self.gradients);
                    self.children(target, id, &inner);
                } else {
                    self.node(target, id, &style);
                }
                self.use_depth -= 1;
                Some(id)
            }
            "rect" => {
                let (Some(w), Some(h)) = (
                    el.attribute("width").and_then(parse_len),
                    el.attribute("height").and_then(parse_len),
                ) else {
                    return None;
                };
                if w <= 0.0 || h <= 0.0 {
                    return None;
                }
                let x = el.attribute("x").and_then(parse_len).unwrap_or(0.0);
                let y = el.attribute("y").and_then(parse_len).unwrap_or(0.0);
                // `rx`/`ry` default to each other, and each clamps to half its own side.
                let rx = el.attribute("rx").and_then(parse_len);
                let ry = el.attribute("ry").and_then(parse_len);
                let (rx, ry) = match (rx, ry) {
                    (Some(a), Some(b)) => (a, b),
                    (Some(a), None) | (None, Some(a)) => (a, a),
                    (None, None) => (0.0, 0.0),
                };
                let rx = rx.max(0.0).min(w * 0.5);
                let ry = ry.max(0.0).min(h * 0.5);
                let rect = kurbo::Rect::from_origin_size((x, y), Size::new(w, h));
                Some(self.rounded_rect(el, parent, index, rect, (rx, ry), &style))
            }
            "circle" | "ellipse" => {
                let (rx, ry) = if el.tag_name().name() == "circle" {
                    let r = el.attribute("r").and_then(parse_len).unwrap_or(0.0);
                    (r, r)
                } else {
                    (
                        el.attribute("rx").and_then(parse_len).unwrap_or(0.0),
                        el.attribute("ry").and_then(parse_len).unwrap_or(0.0),
                    )
                };
                if rx <= 0.0 || ry <= 0.0 {
                    return None;
                }
                let cx = el.attribute("cx").and_then(parse_len).unwrap_or(0.0);
                let cy = el.attribute("cy").and_then(parse_len).unwrap_or(0.0);
                Some(self.shape(
                    el,
                    parent,
                    index,
                    NodeKind::Ellipse {
                        size: Size::new(rx * 2.0, ry * 2.0),
                    },
                    // Our ellipse is a box at the origin; SVG's is a centre and radii.
                    Affine::translate((cx - rx, cy - ry)),
                    &style,
                ))
            }
            "line" => {
                let x1 = el.attribute("x1").and_then(parse_len).unwrap_or(0.0);
                let y1 = el.attribute("y1").and_then(parse_len).unwrap_or(0.0);
                let x2 = el.attribute("x2").and_then(parse_len).unwrap_or(0.0);
                let y2 = el.attribute("y2").and_then(parse_len).unwrap_or(0.0);
                Some(self.shape(
                    el,
                    parent,
                    index,
                    NodeKind::Line {
                        end: Point::new(x2 - x1, y2 - y1),
                    },
                    Affine::translate((x1, y1)),
                    &style,
                ))
            }
            // A point list is a `Path`, not a `Polygon`: `NodeKind::Polygon` is a regular
            // n-gon, and reading every `<polygon>` as one would regularize an arbitrary
            // outline. The round-trip test asserts geometry, not kind, because of this.
            "polygon" | "polyline" => {
                let pts = numbers(el.attribute("points").unwrap_or_default());
                if pts.len() < 4 {
                    return None;
                }
                let mut path = BezPath::new();
                path.move_to(Point::new(pts[0], pts[1]));
                for p in pts[2..].as_chunks::<2>().0 {
                    path.line_to(Point::new(p[0], p[1]));
                }
                if el.tag_name().name() == "polygon" {
                    path.close_path();
                }
                Some(self.shape(
                    el,
                    parent,
                    index,
                    NodeKind::Path {
                        path,
                        corner_radii: Vec::new(),
                    },
                    Affine::IDENTITY,
                    &style,
                ))
            }
            // One text node per positioned run — see [`Builder::text`].
            "text" => {
                let made = self.text(el, parent, &style);
                made.last().copied()
            }
            // A picture is a `Rect` with an image fill; there is no `NodeKind::Image`.
            "image" => {
                let (Some(w), Some(h)) = (
                    el.attribute("width").and_then(parse_len),
                    el.attribute("height").and_then(parse_len),
                ) else {
                    self.skip("image (no size)");
                    return None;
                };
                let href = el
                    .attribute("href")
                    .or_else(|| el.attribute(("http://www.w3.org/1999/xlink", "href")))
                    .unwrap_or_default();
                let Some((bytes, format)) = data_uri(href) else {
                    // An external reference is a skip, not a fetch: core does no I/O.
                    self.skip("image (external)");
                    return None;
                };
                let Some(sink) = self.images.as_deref_mut() else {
                    self.skip("image");
                    return None;
                };
                let Some((image, entry)) = sink.place(bytes, format) else {
                    self.skip("image (undecodable)");
                    return None;
                };
                // Once per distinct picture: the id is a content hash, and a second
                // `AddImage` would invert into removing an entry still in use.
                if !self.added.contains(&image) {
                    self.added.push(image.clone());
                    self.push(Operation::AddImage {
                        id: image.clone(),
                        entry,
                    });
                }
                // SVG's default (`meet`) is contain; `slice` asks for cover, and `none`
                // asks for a stretch this model cannot express, so it is reported.
                let par = el
                    .attribute("preserveAspectRatio")
                    .unwrap_or("xMidYMid meet");
                let fit = if par.contains("slice") {
                    crate::image::ImageFit::Fill
                } else {
                    if par.trim().starts_with("none") {
                        self.approximate("image (stretched)");
                    }
                    crate::image::ImageFit::Fit
                };
                let x = el.attribute("x").and_then(parse_len).unwrap_or(0.0);
                let y = el.attribute("y").and_then(parse_len).unwrap_or(0.0);
                let mut brush = crate::image::image_brush(image.clone());
                if let crate::image::Brush::Image(b) = &mut brush {
                    b.image.fit = fit;
                }
                let id = self.shape(
                    el,
                    parent,
                    index,
                    NodeKind::Rect {
                        size: Size::new(w, h),
                        corner_radii: kurbo::RoundedRectRadii::default(),
                    },
                    Affine::translate((x, y)),
                    // Painted below; `shape` takes fills from the style.
                    &Style {
                        fill: None,
                        stroke: None,
                        ..style.clone()
                    },
                );
                self.push(Operation::SetFills {
                    id,
                    // A node just made, so the list is built whole.
                    fills: crate::item::keyed_by_position([Fill {
                        brush: brush.into(),
                        visible: true,
                    }]),
                });
                Some(id)
            }
            "path" => {
                let d = el.attribute("d")?;
                let Ok(path) = BezPath::from_svg(d) else {
                    self.skip("path (bad d)");
                    return None;
                };
                // kurbo accepts `1e999`, and a non-finite coordinate makes the saved
                // document unopenable and `Document::apply` refuse the transaction, so
                // skip the one path. `numbers()` is the same filter for attributes.
                if !crate::node::path_is_finite(&path) {
                    self.skip("path (a coordinate is not finite)");
                    return None;
                }
                if path.is_empty() {
                    return None;
                }
                Some(self.shape(
                    el,
                    parent,
                    index,
                    NodeKind::Path {
                        path,
                        corner_radii: Vec::new(),
                    },
                    Affine::IDENTITY,
                    &style,
                ))
            }
            other => {
                self.skip(other);
                None
            }
        }
    }

    /// A `<text>` element, as one text node per positioned run.
    ///
    /// A `<tspan>` carrying its own `x` or `y` starts a new node (Illustrator: one per
    /// line); one without is a style run in the current node (this app's writer). A
    /// multi-line paragraph of ours comes back as several layers, a decided non-goal to
    /// rejoin: the only signal is indistinguishable from a stack of separate labels.
    ///
    /// A `<tspan>`'s colour and font scope (size, weight, italic, family) become
    /// character spans over the bytes it contributed; they are collected against
    /// `pending`, and [`Builder::text_node`] shifts them for its trim. A `<tspan>`
    /// gradient fill or one-sided stroke is reported. Any new `Style` field must be
    /// compared below or it is lost silently.
    fn text(
        &mut self,
        el: roxmltree::Node<'d, 'input>,
        parent: NodeId,
        inherited: &Style,
    ) -> Vec<NodeId> {
        let mut made = Vec::new();
        // The element's own anchor is the fallback for a run that states none.
        let mut pending = String::new();
        let mut at = (
            property(el, "x", &self.gradients.css)
                .and_then(parse_len)
                .unwrap_or(0.0),
            property(el, "y", &self.gradients.css)
                .and_then(parse_len)
                .unwrap_or(0.0),
        );
        // `<textPath>` holds the runs and a curve places the type, so `x`/`y` mean
        // nothing. Only the first is read (a text node holds one rail); the rest are
        // reported.
        let rail_el = el.children().find(|c| c.has_tag_name("textPath"));
        if el.children().filter(|c| c.has_tag_name("textPath")).count() > 1 {
            self.skip("text (second textPath on one element)");
        }
        // The style the runs inherit; Illustrator's `<textPath>` carries its own `fill`
        // and `font-*`.
        let resolved = match rail_el {
            Some(tp) => resolve(tp, inherited, &self.gradients),
            None => inherited.clone(),
        };
        // `startOffset` is kept as a percentage only: the field is a fraction so it
        // survives the rail being scaled, and a length would need the arc length
        // `PathWarp` owns, so a length is reported and dropped.
        let rail_offset = rail_el
            .and_then(|tp| tp.attribute("startOffset"))
            .map(str::trim)
            .and_then(|v| match v.strip_suffix('%') {
                Some(p) => p.trim().parse::<f64>().ok().map(|n| n / 100.0),
                None => {
                    // `0` is `0%` in every unit there is, so it costs no report.
                    let zero = v.parse::<f64>().is_ok_and(|n| n == 0.0);
                    (!zero).then(|| {
                        self.approximate("textPath (startOffset in user units)");
                        0.0
                    })
                }
            })
            .unwrap_or(0.0);
        let rail = rail_el.and_then(|tp| self.rail_of(tp));
        // The element whose children are the runs.
        let body = rail_el.unwrap_or(el);
        let inherited = &resolved;
        let mut run_style = inherited.clone();
        let outer = element_transform(el);
        // The element a run came from, which is not always the `<text>`: a positioned
        // `<tspan>` supplies its run's id, opacity and font.
        let mut run_el = el;
        // The spans the finished node owes, as `(start, end)` into `pending`, cleared
        // with it. One entry per attribute, the shape `Spans::set` takes.
        let mut spans: Vec<(usize, usize, crate::typography::CharAttr)> = Vec::new();

        // Flattened first, at any depth; see `text_pieces`.
        let mut pieces = Vec::new();
        self.text_pieces(body, inherited, &mut pieces);

        for piece in pieces {
            let (text, child, s) = match piece {
                TextPiece::Break(child, s) => {
                    // The run so far ends here, at the position it was given.
                    if let Some(id) = self.text_node(
                        run_el,
                        el,
                        parent,
                        &pending,
                        at,
                        &run_style,
                        outer,
                        &spans,
                        rail.as_ref(),
                        rail_offset,
                    ) {
                        made.push(id);
                    }
                    pending.clear();
                    spans.clear();
                    at = (
                        child.attribute("x").and_then(parse_len).unwrap_or(at.0),
                        child.attribute("y").and_then(parse_len).unwrap_or(at.1),
                    );
                    run_style = s;
                    run_el = child;
                    continue;
                }
                TextPiece::Run(text, child, s) => (text, child, s),
            };
            let start = pending.len();
            push_collapsed(&mut pending, text);
            // Text from the element the run is needs no span. Beyond the self-
            // comparisons, the gradient arm would double-report a positioned
            // `<tspan fill="url(#g)">` here and from `text_node`.
            if child == run_el {
                continue;
            }
            let end = pending.len();
            match solid_fill(&s) {
                Some(c) if ink(&s) != ink(&run_style) => {
                    spans.push((start, end, CharAttr::Color(Some(c))));
                }
                // A gradient stated on the `<tspan>` itself, which a `CharAttr` cannot
                // hold; one the run merely inherits is its own paint.
                None if property(child, "fill", &self.gradients.css).is_some() => {
                    self.approximate("text (per-run style)");
                }
                _ => {}
            }
            if s.stroke.is_some() != run_style.stroke.is_some() {
                self.approximate("text (per-run style)");
            }
            // The font scope. The family list is a preference order, resolved and
            // reported by [`Builder::pick_family`]. A value equal to the node's own is
            // pushed anyway and dropped by `Spans::normalize`, so `"Foo, Arial"` against
            // `"Arial"` cannot disagree with the model.
            if s.font_size != run_style.font_size {
                spans.push((start, end, CharAttr::Size(s.font_size)));
            }
            if s.weight != run_style.weight {
                spans.push((start, end, CharAttr::Weight(s.weight)));
            }
            if s.italic != run_style.italic {
                spans.push((start, end, CharAttr::Italic(s.italic)));
            }
            if s.font_family != run_style.font_family
                && let Some(list) = s.font_family.clone()
                && let Some(f) = self.pick_family(&list)
            {
                spans.push((start, end, CharAttr::Family(f)));
            }
        }
        if let Some(id) = self.text_node(
            run_el,
            el,
            parent,
            &pending,
            at,
            &run_style,
            outer,
            &spans,
            rail.as_ref(),
            rail_offset,
        ) {
            made.push(id);
        }
        made
    }

    /// A `<text>`'s content flattened into runs, at whatever depth the file states
    /// them.
    ///
    /// One piece per text node, carrying the innermost element it came from and that
    /// element's resolved [`Style`], so depth does not matter and interleaving works:
    /// `<tspan>x<tspan>y</tspan>z</tspan>` yields three pieces and `z` reads the outer
    /// tspan's style. A `Break` is emitted before the pieces inside it.
    fn text_pieces(
        &self,
        node: roxmltree::Node<'d, 'input>,
        inherited: &Style,
        out: &mut Vec<TextPiece<'d, 'input>>,
    ) {
        for child in node.children() {
            if child.is_text() {
                out.push(TextPiece::Run(
                    child.text().unwrap_or_default(),
                    node,
                    inherited.clone(),
                ));
                continue;
            }
            if !child.has_tag_name("tspan") {
                continue;
            }
            let s = resolve(child, inherited, &self.gradients);
            if child.has_attribute("x") || child.has_attribute("y") {
                out.push(TextPiece::Break(child, s.clone()));
            }
            self.text_pieces(child, &s, out);
        }
    }

    /// The curve a `<textPath>` names, in the referencing element's own user space; the
    /// inverse of `svg::collect_defs`, which puts the rail in `<defs>` as a `<path>`.
    ///
    /// The target's own `transform` is ignored, per SVG (unlike `<use>`), but reported.
    /// `None`, with a report, for a missing `href`, an absent target, no `d` or an
    /// unparseable one; it must not come back as straight text.
    fn rail_of(&mut self, tp: roxmltree::Node<'d, 'input>) -> Option<BezPath> {
        let Some(href) = tp
            .attribute("href")
            .or_else(|| tp.attribute(("http://www.w3.org/1999/xlink", "href")))
        else {
            self.skip("textPath (no href)");
            return None;
        };
        let Some(target) = href
            .trim()
            .strip_prefix('#')
            .and_then(|id| self.gradients.ids.get(id))
            .copied()
        else {
            self.skip("textPath (href resolves to nothing)");
            return None;
        };
        let Some(path) = target
            .attribute("d")
            .and_then(|d| BezPath::from_svg(d).ok())
            // Finiteness too, as in the `"path"` arm: `apply` refuses an infinite one.
            .filter(|p: &BezPath| !p.is_empty() && crate::node::path_is_finite(p))
        else {
            self.skip("textPath (target has no usable path)");
            return None;
        };
        if target.has_attribute("transform") {
            self.approximate("textPath (transform on the referenced path)");
        }
        Some(path)
    }

    /// The first family in a CSS `font-family` list that this machine actually has,
    /// reporting a substitution when it has none of them.
    ///
    /// A `font-family` is a preference order, so the head is not taken regardless.
    /// Shared by [`Builder::text_node`] and the `<tspan>` runs so they substitute
    /// alike. `None` means nothing was available and the substitution is already
    /// reported (deduplicated by name).
    fn pick_family(&mut self, list: &str) -> Option<String> {
        let found = list
            .split(',')
            .map(|f| f.trim().trim_matches(|c| c == '"' || c == '\''))
            .find(|f| crate::text::is_family_available(f))
            .map(str::to_string);
        if found.is_none() {
            self.approximate("text (font substituted)");
        }
        found
    }

    /// One text node holding `content`, with its first baseline at `at`.
    ///
    /// SVG places text by baseline and this model by box, so the text is laid out
    /// headlessly (`text::layout`) to get the shaped ascent between the two.
    /// `text-anchor` is folded into the placement: `middle` shifts the box half its width.
    ///
    /// `runs` are the per-run ranges [`Builder::text`] collected, indexed into the
    /// untrimmed `content`; this function's trim is the only thing that knows what it
    /// removed, so it shifts them. The front trim is structurally zero
    /// ([`push_collapsed`] never emits leading space) and guards a change to
    /// collapsing; the back trim and the `min` run.
    // `clippy::duplicated_attributes` is silent on an item inside an `impl`, so a
    // repeat of the attribute below is caught by `tests/duplicated_attributes.rs`.
    #[allow(clippy::too_many_arguments)]
    fn text_node(
        &mut self,
        el: roxmltree::Node<'d, 'input>,
        text_el: roxmltree::Node<'d, 'input>,
        parent: NodeId,
        content: &str,
        at: (f64, f64),
        style: &Style,
        outer: Affine,
        runs: &[(usize, usize, CharAttr)],
        rail: Option<&BezPath>,
        rail_offset: f64,
    ) -> Option<NodeId> {
        // The same [`XML_SPACE`] set [`push_collapsed`] collapses; they must not drift,
        // because `lead` re-bases every span's byte range onto the trimmed string.
        let lead = content.len() - content.trim_start_matches(XML_SPACE).len();
        let content = content.trim_matches(XML_SPACE);
        if content.is_empty() {
            return None;
        }
        // Font properties come from the resolved `style`, not `el`, since they inherit.
        // Through the setters, not a struct literal: `TextStyle::set` clamps size and
        // weight and a span goes through the same clamp, so a raw default could never
        // equal an out-of-range span and `normalize` would keep it.
        let mut ts = crate::typography::TextStyle::default()
            .with(CharAttr::Size(style.font_size))
            .with(CharAttr::Weight(style.weight))
            .with(CharAttr::Italic(style.italic));
        if let Some(list) = &style.font_family
            && let Some(f) = self.pick_family(list)
        {
            ts.set(CharAttr::Family(f));
        }
        // Through the setter so `fill-opacity` is quantized to the 1/255 the hex field
        // can express (`typography::canonical_color`); an `f32` 0.5 alpha could never
        // coalesce with a default. The invisible-span guard is `ink` in `text`, not this.
        if let Some(c) = solid_fill(style) {
            ts.set(crate::typography::CharAttr::Color(Some(c)));
        } else if style.fill.is_some() {
            // A gradient on the text itself: `TextStyle::color` is a colour, so the text
            // takes the default ink and this report. Here because this is the only place
            // that sees a whole node's fill, and a positioned `<tspan>` arrives as its
            // own node.
            self.approximate("text (gradient fill)");
        }

        let mut spans = crate::typography::CharSpans::default();
        for (start, end, attr) in runs {
            let start = start.saturating_sub(lead).min(content.len());
            let end = end.saturating_sub(lead).min(content.len());
            if start < end {
                spans.set(start..end, attr.clone(), &ts);
            }
        }
        // A span covering every byte is a default: hoist it onto the node style, else
        // this app's own output (one `<tspan>` per style run, font on the tspan) would
        // carry a full-coverage span over a node saying otherwise. `set` then `clear`
        // over the range, per kind, leaves other attributes' spans alone.
        let whole: Vec<CharAttr> = spans
            .as_slice()
            .iter()
            .filter(|s| s.start == 0 && s.end == content.len())
            .map(|s| s.attr.clone())
            .collect();
        for attr in whole {
            let kind = attr.kind();
            ts.set(attr);
            spans.clear(0..content.len(), kind, &ts);
        }
        let para_spans = crate::typography::ParaSpans::default();
        let mut paragraph = crate::typography::ParagraphStyle::default();
        // On a rail the anchor becomes `align` rather than a shift: railed text has no
        // `x` to shift from, and the anchor is where along the curve the type sits.
        if rail.is_some() {
            paragraph.align = match style.anchor {
                Anchor::Middle => crate::typography::TextAlign::Center,
                Anchor::End => crate::typography::TextAlign::End,
                Anchor::Start => crate::typography::TextAlign::Start,
            };
        }
        let block = crate::node::BlockStyle::default();
        let sizing = crate::node::TextSizing::Auto;
        let layout = crate::text::layout(crate::node::TextRef {
            content,
            style: &ts,
            spans: &spans,
            para_spans: &para_spans,
            paragraph: &paragraph,
            block: &block,
            sizing: &sizing,
            on_path: rail,
            // Always `false`: our writer writes a flip as a reversed rail, which draws
            // identically, and only SVG 2's `side="right"` would say otherwise.
            on_path_flip: false,
            on_path_offset: rail_offset,
        });
        // The first glyph's `y` is the baseline below the layout origin.
        let ascent = layout
            .runs
            .first()
            .and_then(|r| r.glyphs.first())
            // 0.8 of the size is the fallback when the text shapes to no glyphs.
            .map_or(ts.font_size * 0.8, |g| f64::from(g.y));
        let shift = match style.anchor {
            Anchor::Middle => layout.size.width * 0.5,
            Anchor::End => layout.size.width,
            Anchor::Start => 0.0,
        };

        let id = self.ids.mint();
        self.push(Operation::CreateNode {
            id,
            parent,
            index: self.index_in(parent),
            kind: NodeKind::Text {
                content: content.to_string(),
                style: Box::new(ts),
                spans,
                para_spans,
                paragraph,
                block,
                sizing,
                on_path: rail.cloned().map(Box::new),
                on_path_flip: false,
                on_path_offset: rail_offset,
            },
            // The `<text>`'s transform (`outer`), not `el`'s: `el` may be a `<tspan>`,
            // which carries none. A rail places the node itself, so the baseline
            // arithmetic would move the type off the curve by an ascent.
            transform: Some(if rail.is_some() {
                outer
            } else {
                outer * Affine::translate((at.0 - shift - layout.origin.x, at.1 - ascent))
            }),
            name: element_name(el),
        });
        self.shapes += 1;
        // The `<text>`'s own chrome as well as this run's — see `chrome`'s `over`.
        self.chrome(el, id, style.current, Some(text_el));
        Some(id)
    }

    /// A `<rect>` with its two corner radii already clamped, as whichever of the
    /// model's three shapes holds it *exactly*.
    ///
    /// SVG's rounded rect has an ellipse at each corner and ours a circle, so one
    /// attribute pair spans three kinds:
    ///
    /// - `rx == ry` — a [`NodeKind::Rect`] with that radius.
    /// - both radii at half their side — the shape is an [`NodeKind::Ellipse`].
    /// - anything else — a [`NodeKind::Path`], the only exact form.
    fn rounded_rect(
        &mut self,
        el: roxmltree::Node<'_, 'input>,
        parent: NodeId,
        index: usize,
        // The box in the element's own user space (`x`/`y` and `width`/`height`).
        rect: kurbo::Rect,
        (rx, ry): (f64, f64),
        style: &Style,
    ) -> NodeId {
        let (x, y) = (rect.x0, rect.y0);
        let size = rect.size();
        let (w, h) = (size.width, size.height);
        if (rx - ry).abs() < 1e-9 {
            return self.shape(
                el,
                parent,
                index,
                NodeKind::Rect {
                    size,
                    corner_radii: kurbo::RoundedRectRadii::from_single_radius(rx),
                },
                Affine::translate((x, y)),
                style,
            );
        }
        if rx >= w * 0.5 - 1e-9 && ry >= h * 0.5 - 1e-9 {
            // Our ellipse is a box at the origin, and the rect's box is already it.
            return self.shape(
                el,
                parent,
                index,
                NodeKind::Ellipse { size },
                Affine::translate((x, y)),
                style,
            );
        }
        // The quarter-ellipse's cubic constant.
        const K: f64 = 0.552_284_749_830_793_4;
        let (cx, cy) = (rx * K, ry * K);
        let mut path = BezPath::new();
        path.move_to((rx, 0.0));
        path.line_to((w - rx, 0.0));
        path.curve_to((w - rx + cx, 0.0), (w, ry - cy), (w, ry));
        path.line_to((w, h - ry));
        path.curve_to((w, h - ry + cy), (w - rx + cx, h), (w - rx, h));
        path.line_to((rx, h));
        path.curve_to((rx - cx, h), (0.0, h - ry + cy), (0.0, h - ry));
        path.line_to((0.0, ry));
        path.curve_to((0.0, ry - cy), (rx - cx, 0.0), (rx, 0.0));
        path.close_path();
        self.shape(
            el,
            parent,
            index,
            NodeKind::Path {
                path,
                corner_radii: Vec::new(),
            },
            Affine::translate((x, y)),
            style,
        )
    }

    /// Create one shape node and paint it, and hand back its id.
    fn shape(
        &mut self,
        el: roxmltree::Node<'_, 'input>,
        parent: NodeId,
        index: usize,
        kind: NodeKind,
        local: Affine,
        style: &Style,
    ) -> NodeId {
        let id = self.ids.mint();
        // The element's transform, then the shape's placement inside it: `rotate(45)`
        // on a `<rect x="10">` turns about the user origin, not the corner.
        let transform = element_transform(el) * local;
        let bounds = crate::geometry::local_path(&kind)
            .map(|p| kurbo::Shape::bounding_box(&p))
            .unwrap_or_default();
        self.push(Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(transform),
            name: element_name(el),
        });
        self.shapes += 1;

        // Paint: fills, strokes, then the fill rule.

        let fills: Vec<Fill> = style
            .fill
            .as_ref()
            .map(|p| Fill {
                brush: p.brush(bounds, local, style.fill_opacity).into(),
                visible: true,
            })
            .into_iter()
            .collect();
        // Lists for a node just made are built whole, as every list below is.
        if !fills.is_empty() {
            self.push(Operation::SetFills {
                id,
                fills: crate::item::keyed_by_position(fills),
            });
        }
        let strokes: Vec<Stroke> = style
            .stroke
            .as_ref()
            .filter(|_| style.stroke_width > 0.0)
            .map(|p| Stroke {
                brush: p.brush(bounds, local, style.stroke_opacity).into(),
                width: style.stroke_width,
                width_var: None,
                join: style.stroke_join,
                cap: style.stroke_cap,
                dashes: style.dashes.clone(),
                dash_offset: style.dash_offset,
                // `dash_fit` is ours: the markup already holds the fitted numbers, and
                // `true` would re-fit them.
                miter_limit: style.miter_limit,
                dash_fit: false,
                // SVG strokes are always centred.
                align: StrokeAlign::Center,
                sides: StrokeSides::All,
                visible: true,
            })
            .into_iter()
            .collect();
        if !strokes.is_empty() {
            self.push(Operation::SetStrokes {
                id,
                strokes: crate::item::keyed_by_position(strokes),
            });
        }
        if style.fill_rule == FillRule::EvenOdd {
            self.push(Operation::SetFillRule {
                id,
                rule: FillRule::EvenOdd,
            });
        }
        self.chrome(el, id, style.current, None);
        id
    }

    /// The two properties every element carries and neither inherits: its own
    /// opacity, and whether it is drawn at all.
    ///
    /// `display="none"` becomes a hidden layer rather than nothing.
    ///
    /// `over` is the `<text>` a positioned `<tspan>` hangs under, for `text_node`'s
    /// runs after the first: the `<tspan>` carries none of these properties, so they
    /// compose with it (opacity multiplies, either `display` or `visibility` hides)
    /// rather than being replaced by it.
    fn chrome(
        &mut self,
        el: roxmltree::Node<'_, 'input>,
        id: NodeId,
        current: Color,
        over: Option<roxmltree::Node<'_, 'input>>,
    ) {
        // Both read before either is written: the reads borrow `self.gradients` and
        // the writes `self.ops`.
        let css = &self.gradients.css;
        let prop = |name: &str| property(el, name, css);
        let outer = |name: &str| {
            over.filter(|o| *o != el)
                .and_then(|o| property(o, name, css))
        };
        let hides = |v: Option<&str>, name: &str| match name {
            "display" => v == Some("none"),
            _ => v == Some("hidden"),
        };
        // A product (50% inside 50% is 25%), with `None` when stated on neither.
        let opacity = match (
            prop("opacity").and_then(parse_len),
            outer("opacity").and_then(parse_len),
        ) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(1.0) * b.unwrap_or(1.0)),
        };
        let hidden = hides(prop("display"), "display")
            || hides(prop("visibility"), "visibility")
            || hides(outer("display"), "display")
            || hides(outer("visibility"), "visibility");
        // `filter` is read as a property (Inkscape writes it into `style`). The run's
        // own wins over the outer element's: filters do not compose, the model holds
        // one effect stack per node.
        let filter = prop("filter").or_else(|| outer("filter")).map(|v| {
            let read = self
                .gradients
                .filter(v)
                .and_then(|def| effects_of(def, css, current));
            (v, read)
        });
        if let Some(o) = opacity
            && o < 1.0
        {
            self.push(Operation::SetOpacity {
                id,
                opacity: o.clamp(0.0, 1.0) as f32,
            });
        }
        if hidden {
            self.push(Operation::SetVisible { id, visible: false });
        }
        match filter {
            None => {}
            // Effects composite on a container as on a shape, hence `chrome`, not `shape`.
            Some((_, Some(read))) => {
                if read.uneven {
                    self.approximate("uneven feGaussianBlur (averaged)");
                }
                // An empty stack is a filter resolved to "unchanged" (a bare
                // `<feOffset/>`); write nothing.
                if !read.effects.is_empty() {
                    self.push(Operation::SetEffects {
                        id,
                        effects: crate::item::keyed_by_position(read.effects),
                    });
                }
            }
            // A graph this cannot invert, a reference to nothing, or
            // `objectBoundingBox` primitive units.
            Some(_) => self.skip("filter"),
        }
    }
}

impl Paint {
    /// The affine that carries this paint's gradient from the coordinates it was
    /// written in to the shape's own space. Identity for a solid.
    ///
    /// Shared so the report and the brush cannot disagree about the composition
    /// (see [`Paint::brush`]).
    fn mapping(&self, bounds: kurbo::Rect, local: Affine) -> Affine {
        match self {
            Paint::Solid(_) => Affine::IDENTITY,
            Paint::Gradient(_, units, xform) => {
                let onto = match units {
                    GradientUnits::ObjectBounding => {
                        Affine::translate((bounds.x0, bounds.y0))
                            * Affine::scale_non_uniform(bounds.width(), bounds.height())
                    }
                    GradientUnits::User => local.inverse(),
                };
                onto * *xform
            }
        }
    }

    /// Whether this is a radial gradient whose mapping is not a similarity, so its
    /// ellipse has to ride on [`crate::image::GradientBrush::transform`] rather than
    /// be baked into the two circles.
    ///
    /// Tests the ratio of the mapping's singular values, not whether the matrix looks
    /// diagonal (a rotation is representable; see [`transform_gradient`]). The
    /// threshold is numerical noise, not a visibility judgement; `1e-6` rather than
    /// `0.0` is only insurance, since a similarity's ratio is exactly `1.0`.
    fn squashed_radial(&self, bounds: kurbo::Rect, local: Affine) -> bool {
        if !matches!(
            self,
            Paint::Gradient(g, _, _) if matches!(g.kind, peniko::GradientKind::Radial(_))
        ) {
            return false;
        }
        let [a, b, c, d, _, _] = self.mapping(bounds, local).as_coeffs();
        let sum = a * a + b * b + c * c + d * d;
        let diff = ((a * a + b * b - c * c - d * d).powi(2) + 4.0 * (a * c + b * d).powi(2)).sqrt();
        let (hi, lo) = (((sum + diff) * 0.5).sqrt(), ((sum - diff) * 0.5).sqrt());
        lo > 1e-9 && hi / lo > 1.000_001
    }

    /// Fold an opacity into the paint, and resolve a gradient against the shape it is
    /// on.
    ///
    /// Both `gradientUnits` need a mapping. `local` is the shape's placement inside its
    /// element's user space (a rect's `x`/`y`); our brush is written in the shape's own
    /// space, while a `userSpaceOnUse` gradient is written in the space that placement
    /// was measured in.
    fn brush(&self, bounds: kurbo::Rect, local: Affine, alpha: f32) -> Brush {
        match self {
            Paint::Solid(c) => Brush::Solid(c.multiply_alpha(alpha)),
            Paint::Gradient(g, _, _) => {
                let mut g = (**g).clone();
                // [`Paint::mapping`]: the unit square onto the shape's box for
                // `objectBoundingBox`, the inverse placement for `userSpaceOnUse`.
                let m = self.mapping(bounds, local);
                // Baked where exact, carried where not: linear points take any affine,
                // radial circles only a similarity, so only a squash survives as a
                // transform and the geometry stays in the shape's own space.
                let transform = if self.squashed_radial(bounds, local) {
                    m
                } else {
                    g.kind = transform_gradient(g.kind, m);
                    Affine::IDENTITY
                };
                if alpha < 1.0 {
                    for s in g.stops.iter_mut() {
                        s.color = s.color.multiply_alpha(alpha);
                    }
                }
                Brush::Gradient(crate::image::GradientBrush {
                    gradient: g,
                    transform,
                    // Already baked into the stops above; carrying it would apply it twice.
                    opacity: 1.0,
                })
            }
        }
    }
}

fn transform_gradient(kind: peniko::GradientKind, t: Affine) -> peniko::GradientKind {
    let p = |v: peniko::kurbo::Point| {
        let q = t * Point::new(v.x, v.y);
        peniko::kurbo::Point::new(q.x, q.y)
    };
    // A radius has no direction, so it scales by `sqrt(|determinant|)`, the geometric
    // mean of the axes. Not the diagonal: that is the scale only without rotation, and
    // `matrix(0, -1.33, -1.33, 0, …)` (Inkscape's rotated highlight) has `a == d == 0`
    // and would collapse the radius. A radial reaching here is a similarity, so it is
    // exact; a squash is routed to the brush's transform by [`Paint::brush`].
    let s = t.determinant().abs().sqrt() as f32;
    match kind {
        peniko::GradientKind::Linear(l) => {
            // Mapping the two endpoints is not mapping the gradient: its iso-lines are
            // perpendicular to `start → end` in the written space, and only a similarity
            // keeps that, so a non-square `objectBoundingBox` tilts the bands. The exact
            // answer is still linear; transform the covector: the parameter at `q` is
            // `(q − T·start) · u` with `u = Tₗᵢₙ⁻ᵀ·d / |d|²`, which peniko reads back as
            // `u / |u|²` laid off from the mapped start.
            let d = l.end - l.start;
            let len2 = d.hypot2();
            let c = t.as_coeffs();
            let inv = Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
                .inverse()
                .as_coeffs();
            // The inverse transpose: the off-diagonal entries swap.
            let u =
                kurbo::Vec2::new(inv[0] * d.x + inv[1] * d.y, inv[2] * d.x + inv[3] * d.y) / len2;
            let start = p(l.start);
            // A degenerate gradient (zero length) or a singular transform has no
            // direction to correct; the endpoint mapping is as good as anything.
            let end = if len2 > 0.0 && u.hypot2() > 0.0 && crate::build::affine_is_invertible(t) {
                let e = start + u / u.hypot2();
                peniko::kurbo::Point::new(e.x, e.y)
            } else {
                p(l.end)
            };
            peniko::GradientKind::Linear(peniko::LinearGradientPosition { start, end })
        }
        peniko::GradientKind::Radial(r) => {
            peniko::GradientKind::Radial(peniko::RadialGradientPosition {
                start_center: p(r.start_center),
                start_radius: r.start_radius * s,
                end_center: p(r.end_center),
                end_radius: r.end_radius * s,
            })
        }
        peniko::GradientKind::Sweep(w) => {
            peniko::GradientKind::Sweep(peniko::SweepGradientPosition {
                center: p(w.center),
                ..w
            })
        }
    }
}

/// A gradient and everything it inherits from, nearest first.
///
/// SVG's gradient inheritance is per attribute: a gradient with no `x1` takes the
/// referenced one's, one with no `<stop>` children takes its stops, and so on. Inkscape
/// writes every gradient this way (a stop-holding element plus a positioned one per
/// use, joined by `xlink:href`). A broken link costs only the attributes behind it,
/// as in a browser. The depth cap of eight stops a cycle.
fn gradient_chain<'a, 'input>(
    el: roxmltree::Node<'a, 'input>,
    ids: &rustc_hash::FxHashMap<&'a str, roxmltree::Node<'a, 'input>>,
) -> Vec<roxmltree::Node<'a, 'input>> {
    let mut out = vec![el];
    for _ in 0..8 {
        let cur = *out.last().expect("seeded with `el`");
        let Some(href) = cur
            .attribute("href")
            .or_else(|| cur.attribute(("http://www.w3.org/1999/xlink", "href")))
        else {
            break;
        };
        let Some(next) = href
            .trim()
            .strip_prefix('#')
            .and_then(|id| ids.get(id))
            .copied()
        else {
            break;
        };
        let name = next.tag_name().name();
        if name != "linearGradient" && name != "radialGradient" {
            break;
        }
        if out.iter().any(|n| n.id() == next.id()) {
            break;
        }
        out.push(next);
    }
    out
}

/// Every gradient in the document, by id.
///
/// Collected in one pass, because a `fill` may reference a gradient defined after it
/// and `xlink:href` inheritance can need a second lookup.
struct Gradients<'a, 'input> {
    // Keyed on `&'a str`, the document borrow: an id read off a node cannot outlive
    // the tree it was read from.
    by_id: rustc_hash::FxHashMap<&'a str, (peniko::Gradient, GradientUnits, Affine)>,
    /// `<clipPath>` and `<mask>` elements by id, kept as nodes: their contents are
    /// ordinary markup, built by the same walk as a shape.
    clips: rustc_hash::FxHashMap<&'a str, roxmltree::Node<'a, 'input>>,
    /// Every element carrying an id, which `<use href="#x">` needs. Kept separate from
    /// the two maps above so a lookup cannot return a gradient where a clip belongs.
    ids: rustc_hash::FxHashMap<&'a str, roxmltree::Node<'a, 'input>>,
    /// The document's `<style>` rules — see [`Css`].
    css: Css<'a>,
    /// Whether the document holds a `<pattern>`, a paint server this reader does not
    /// read. This app's own writer emits every image fill as one, so without this
    /// report a re-imported picture silently becomes a black rectangle (the inherited
    /// initial fill). A flag, reported once for the file.
    patterns: bool,
    /// Whether any `fill` or `stroke` carried a value this reader could not parse.
    ///
    /// A `Cell` because `resolve` sees this by shared reference and sets it while
    /// walking elements; the other flags are facts about the defs, settled in `collect`.
    unreadable_paint: std::cell::Cell<bool>,
    /// Whether any gradient's stop list was resampled down to [`MAX_GRADIENT_STOPS`].
    resampled: bool,
}

/// The most stops this reader keeps from one `<linearGradient>`/`<radialGradient>`.
///
/// Bounds a gradient's payload, which neither `nodes_limit` nor [`MAX_SVG_NODES`]
/// sees: every shape naming a gradient gets its own copy of the list, which ends up
/// in the `Document` and the saved JSON (6,000 stops on 6,000 rects held 864 MB).
/// Real gradients are single digits to dozens, so a file hitting 256 is generated and
/// resampling it is honest.
const MAX_GRADIENT_STOPS: usize = 256;

/// Every `<style>` element's full text content, keyed by [`roxmltree::Node::id`].
///
/// Owned and built by the caller, because the rules borrow it: a sheet interrupted by
/// an XML comment is two text nodes with no single `&str` to point at, and `Css`
/// outlives `collect`, so the caller declares this above the `Builder`.
fn style_sheets(doc: &roxmltree::Document<'_>) -> rustc_hash::FxHashMap<roxmltree::NodeId, String> {
    doc.descendants()
        .filter(roxmltree::Node::is_element)
        .filter(|el| el.tag_name().name() == "style")
        .map(|el| {
            (
                el.id(),
                el.children()
                    .filter(roxmltree::Node::is_text)
                    .filter_map(|c| c.text())
                    .collect(),
            )
        })
        .collect()
}

impl<'a, 'input> Gradients<'a, 'input> {
    fn collect(
        doc: &'a roxmltree::Document<'input>,
        sheets: &'a rustc_hash::FxHashMap<roxmltree::NodeId, String>,
    ) -> Self {
        let mut by_id = rustc_hash::FxHashMap::default();
        let mut clips = rustc_hash::FxHashMap::default();
        let mut ids = rustc_hash::FxHashMap::default();
        let mut css = Css::default();
        let mut resampled = false;
        let mut patterns = false;
        for el in doc.descendants().filter(roxmltree::Node::is_element) {
            let name = el.tag_name().name();
            patterns |= name == "pattern";
            if name == "style" {
                // Every text child (see `style_sheets`), not `text()`: an XML comment
                // inside `<style>` (the legacy `<style><!-- … --></style>` idiom) would
                // end the sheet and send every classed shape to the initial black.
                if let Some(sheet) = sheets.get(&el.id()) {
                    let (rules, complex) = parse_css(sheet);
                    css.rules.extend(rules);
                    css.complex |= complex;
                }
            }
            let Some(id) = el.attribute("id") else {
                continue;
            };
            if name == "clipPath" || name == "mask" {
                clips.entry(id).or_insert(el);
            }
            // First wins for all three id maps (`clips`, `ids`, `by_id`), as in a
            // browser, so one document never has two answers for an id.
            ids.entry(id).or_insert(el);
        }
        for el in doc.descendants().filter(roxmltree::Node::is_element) {
            let name = el.tag_name().name();
            if name != "linearGradient" && name != "radialGradient" {
                continue;
            }
            let Some(id) = el.attribute("id") else {
                continue;
            };
            // Everything below reads the chain, not the element; see [`gradient_chain`].
            let chain = gradient_chain(el, &ids);
            let mut stops: Vec<peniko::ColorStop> = chain
                .iter()
                .find_map(|n| {
                    let s: Vec<peniko::ColorStop> = n
                        .children()
                        .filter(|c| c.has_tag_name("stop"))
                        .filter_map(|c| stop_of(c, &css))
                        .collect();
                    (!s.is_empty()).then_some(s)
                })
                .unwrap_or_default();
            if stops.is_empty() {
                continue;
            }
            // SVG corrects a descending offset up, so the model holds what the file
            // means. The render boundary does the same for hand-edited files; neither
            // covers the other.
            crate::image::make_stops_monotonic(&mut stops);
            // Capped at `MAX_GRADIENT_STOPS`, resampled evenly rather than truncated
            // (which squeezes the ramp into the front) or refused (which loses the paint).
            if stops.len() > MAX_GRADIENT_STOPS {
                let n = stops.len();
                stops = (0..MAX_GRADIENT_STOPS)
                    .map(|i| stops[i * (n - 1) / (MAX_GRADIENT_STOPS - 1)])
                    .collect();
                resampled = true;
            }
            let attr = |n: &str| chain.iter().find_map(|g| g.attribute(n));
            let units = match attr("gradientUnits") {
                Some("userSpaceOnUse") => GradientUnits::User,
                _ => GradientUnits::ObjectBounding,
            };
            let xform = attr("gradientTransform")
                .map(parse_transform)
                .unwrap_or(Affine::IDENTITY);
            let num = |n: &str, d: f64| attr(n).and_then(parse_len).unwrap_or(d);
            let kind = if name == "linearGradient" {
                peniko::GradientKind::Linear(peniko::LinearGradientPosition {
                    start: peniko::kurbo::Point::new(num("x1", 0.0), num("y1", 0.0)),
                    end: peniko::kurbo::Point::new(num("x2", 1.0), num("y2", 0.0)),
                })
            } else {
                let (cx, cy) = (num("cx", 0.5), num("cy", 0.5));
                let r = num("r", 0.5);
                peniko::GradientKind::Radial(peniko::RadialGradientPosition {
                    // `fx`/`fy` (the focal point) default to the centre.
                    start_center: peniko::kurbo::Point::new(num("fx", cx), num("fy", cy)),
                    start_radius: 0.0,
                    end_center: peniko::kurbo::Point::new(cx, cy),
                    end_radius: r as f32,
                })
            };
            let mut g = peniko::Gradient::new_linear((0.0, 0.0), (0.0, 0.0));
            g.kind = kind;
            // `spreadMethod` is `extend`: a reflected ramp is how brushed-metal sheens
            // are written, and padding it leaves the outer half flat.
            g.extend = match attr("spreadMethod") {
                Some("reflect") => peniko::Extend::Reflect,
                Some("repeat") => peniko::Extend::Repeat,
                _ => peniko::Extend::Pad,
            };
            g.stops = stops.as_slice().into();
            // `or_insert`: first wins, as for `ids` above.
            by_id.entry(id).or_insert((g, units, xform));
        }
        Self {
            by_id,
            clips,
            ids,
            css,
            resampled,
            patterns,
            unreadable_paint: std::cell::Cell::new(false),
        }
    }

    /// Resolve a `<use>`'s `href`, a bare `#id` rather than a `url(#id)`, so
    /// [`reference()`] is not used. The `()` disambiguates from Rust's `reference`
    /// primitive type.
    fn element(&self, href: &str) -> Option<roxmltree::Node<'a, 'input>> {
        self.ids.get(href.trim().strip_prefix('#')?).copied()
    }

    /// Resolve a `fill="url(#id)"` value.
    fn get(&self, value: &str) -> Option<(peniko::Gradient, GradientUnits, Affine)> {
        self.by_id.get(reference(value)?).cloned()
    }

    /// Resolve a `clip-path="url(#id)"` or `mask="url(#id)"` value.
    fn clip(&self, value: &str) -> Option<roxmltree::Node<'a, 'input>> {
        self.clips.get(reference(value)?).copied()
    }

    /// Resolve a `filter="url(#id)"` value to the `<filter>` it names.
    ///
    /// Off [`Self::ids`] with a tag check, which stops `filter="url(#some-gradient)"`
    /// from resolving.
    fn filter(&self, value: &str) -> Option<roxmltree::Node<'a, 'input>> {
        let el = *self.ids.get(reference(value)?)?;
        el.has_tag_name("filter").then_some(el)
    }
}

/// The filter shapes this model can hold, read out of a `<filter>` element.
///
/// A `<filter>` is an image-processing graph and an effect stack is four kinds, so
/// most arrangements are still a skip. Read: a lone `<feGaussianBlur>` (Inkscape's
/// Blur slider), a lone `<feDropShadow>`, and the shadow chain (silhouette, optional
/// inversion and dilation, offset, blur, flood cut to the silhouette) that this app's
/// writer emits (`ondin_export::svg::shadow_graph`) and older files write by hand.
///
/// [`crate::effect::EffectKind::Filters`] is written but cannot be read back: its
/// 20-value `feColorMatrix` does not determine the brightness and contrast that made
/// it, and guessing would invent values. A filter is all or nothing: one unrecognised
/// primitive refuses the element, since a half-read filter is wrong silently.
struct FilterRead {
    /// In document order, which is stack order, so nothing is sorted.
    effects: Vec<crate::effect::Effect>,
    /// Some `stdDeviation` named two unequal numbers. The model's blur is isotropic,
    /// so the mean is used and the caller reports it.
    uneven: bool,
}

/// The filter region is not modelled and deliberately not reported.
///
/// Every `<filter>` clips to a region (default `-10% -10% 120% 120%`) and
/// [`crate::effect::EffectKind::LayerBlur`] has none, but regions are nearly always
/// generated to fit the blur (Inkscape fits 2.4 σ per axis, so fractions are not
/// comparable across axes and a naive comparison gives false positives).
/// [`crate::effect::BLUR_CUTOFF`] reaches 3 σ, adding about 1.6% of the kernel's
/// tail. Judging an authored region needs the shape's bounds, which `chrome` lacks.
const _FILTER_REGION_IS_NOT_MODELLED: () = ();

/// A `<filter>` as an effect stack, or `None` for anything this cannot hold.
///
/// A linear walk in document order: every producer of the idioms above emits the
/// primitives in the order they compose, so a result-and-`in` graph needs no
/// topological sort. A file that names its results out of order is refused, not
/// misread.
///
/// `current` is the referencing element's `color`, for `flood_color`'s `currentColor`
/// (a `<filter>` in `<defs>` has no inherited paint).
fn effects_of<'a>(
    def: roxmltree::Node<'a, '_>,
    css: &Css<'a>,
    current: Color,
) -> Option<FilterRead> {
    // `primitiveUnits="objectBoundingBox"` makes `stdDeviation` a fraction; refusing is
    // the only honest answer without the bounds, and it is rare.
    if def.attribute("primitiveUnits") == Some("objectBoundingBox") {
        return None;
    }
    let prims: Vec<_> = def.children().filter(roxmltree::Node::is_element).collect();
    if prims.is_empty() {
        return None;
    }
    let mut out = FilterRead {
        effects: Vec::new(),
        uneven: false,
    };
    let mut i = 0;
    while i < prims.len() {
        let p = prims[i];
        if starts_shadow(p) {
            let (shadow, inner, next) = shadow_chain(&prims, i, css, current, &mut out.uneven)?;
            out.effects.push(crate::effect::Effect::new(if inner {
                crate::effect::EffectKind::InnerShadow(shadow)
            } else {
                crate::effect::EffectKind::DropShadow(shadow)
            }));
            i = next;
        } else if p.has_tag_name("feDropShadow") {
            // The SVG primitive has no spread, so the model's is zero.
            let (dev, uneven) = deviation_of(p.attribute("stdDeviation").unwrap_or("2"))?;
            out.uneven |= uneven;
            out.effects.push(crate::effect::Effect::new(
                crate::effect::EffectKind::DropShadow(crate::effect::Shadow {
                    offset: kurbo::Vec2::new(
                        p.attribute("dx").and_then(parse_len).unwrap_or(2.0),
                        p.attribute("dy").and_then(parse_len).unwrap_or(2.0),
                    ),
                    blur: dev / crate::effect::BLUR_DEVIATION,
                    spread: 0.0,
                    color: flood_color(p, css, current)?,
                }),
            ));
            i += 1;
        } else if p.has_tag_name("feGaussianBlur") {
            // A blur belonging to a shadow was consumed by `shadow_chain`, so this one
            // blurs the drawing itself.
            let (dev, uneven) = deviation_of(p.attribute("stdDeviation")?)?;
            out.uneven |= uneven;
            // A zero deviation disables the primitive: the identity, not a skip.
            if dev > 0.0 {
                out.effects.push(crate::effect::Effect::new(
                    crate::effect::EffectKind::LayerBlur {
                        radius: dev / crate::effect::BLUR_DEVIATION,
                    },
                ));
            }
            i += 1;
        } else if p.has_tag_name("feOffset")
            && p.attribute("dx").is_none()
            && p.attribute("dy").is_none()
        {
            // The identity primitive, read rather than refused: a filter with no
            // primitives renders nothing, so the writer emits a bare `<feOffset/>` for
            // an all-neutral stack (`ondin_export::svg::effect_def`).
            i += 1;
        } else if p.has_tag_name("feMerge") {
            // Assembly, not an effect: its inputs were read already.
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

/// Whether `p` begins a shadow chain rather than acting on the layer itself.
///
/// The silhouette is either a `feColorMatrix` keeping only alpha (this app's writer,
/// since `SourceAlpha` would ignore earlier stack changes) or `SourceAlpha` itself
/// (hand-authored files).
fn starts_shadow(p: roxmltree::Node<'_, '_>) -> bool {
    p.attribute("in") == Some("SourceAlpha") || is_alpha_matrix(p)
}

/// The 20-value `feColorMatrix` that keeps alpha and discards colour.
fn is_alpha_matrix(p: roxmltree::Node<'_, '_>) -> bool {
    if !p.has_tag_name("feColorMatrix") || p.attribute("type").unwrap_or("matrix") != "matrix" {
        return false;
    }
    let v = numbers(p.attribute("values").unwrap_or_default());
    // Row four takes the source's alpha, the rest are zero: an opaque black stencil.
    v.len() == 20
        && v.iter()
            .enumerate()
            .all(|(n, c)| *c == if n == 18 { 1.0 } else { 0.0 })
}

/// One shadow, read from `prims[start..]`. Returns it, whether it is an inner
/// shadow, and the index after the last primitive it consumed.
///
/// The offset and the blur are accepted in either order: they give the same picture,
/// and this app's writer offsets first while hand-written files blur first.
fn shadow_chain<'a>(
    prims: &[roxmltree::Node<'a, '_>],
    start: usize,
    css: &Css<'a>,
    current: Color,
    uneven: &mut bool,
) -> Option<(crate::effect::Shadow, bool, usize)> {
    let mut i = start;
    // The silhouette, when it is a primitive of its own.
    if is_alpha_matrix(prims[i]) {
        i += 1;
    }
    let at = |i: usize| prims.get(i).copied();

    // Everything outside the layer, which an inner shadow casts from.
    let mut inner = false;
    if let Some(p) = at(i)
        && p.has_tag_name("feComponentTransfer")
    {
        let func = p.children().find(|c| c.has_tag_name("feFuncA"))?;
        if func.attribute("type") != Some("table")
            || numbers(func.attribute("tableValues").unwrap_or_default()) != [1.0, 0.0]
        {
            return None;
        }
        inner = true;
        i += 1;
    }

    // The dilation's sign depends on the shadow: the writer picks the operator as
    // `(spread > 0) != inner`, so `dilate` is not simply "positive".
    let mut spread = 0.0;
    if let Some(p) = at(i)
        && p.has_tag_name("feMorphology")
    {
        let r = numbers(p.attribute("radius").unwrap_or_default());
        let r = match r.as_slice() {
            [a] => *a,
            [x, y] if (x - y).abs() < 1e-9 => *x,
            _ => return None,
        };
        // `erode` is the spec's default for a `feMorphology` with no operator.
        let dilate = p.attribute("operator").unwrap_or("erode") == "dilate";
        spread = if dilate != inner { r } else { -r };
        i += 1;
    }

    let (mut offset, mut blur) = (kurbo::Vec2::ZERO, 0.0);
    let (mut seen_offset, mut seen_blur) = (false, false);
    while let Some(p) = at(i) {
        if p.has_tag_name("feOffset") && !seen_offset {
            offset = kurbo::Vec2::new(
                p.attribute("dx").and_then(parse_len).unwrap_or(0.0),
                p.attribute("dy").and_then(parse_len).unwrap_or(0.0),
            );
            seen_offset = true;
        } else if p.has_tag_name("feGaussianBlur") && !seen_blur {
            let (dev, un) = deviation_of(p.attribute("stdDeviation")?)?;
            *uneven |= un;
            blur = dev / crate::effect::BLUR_DEVIATION;
            seen_blur = true;
        } else {
            break;
        }
        i += 1;
    }
    // A silhouette neither moved nor softened is a recolour, not a shadow; refuse it.
    // Tested on the values, not on which primitives were present: a zero `dx`/`dy`
    // and zero `stdDeviation` set both flags yet change nothing. `spread` is
    // deliberately not in the test, so a spread-only halo is still refused.
    if offset == kurbo::Vec2::ZERO && blur == 0.0 {
        return None;
    }

    // The flood fills the region and the composite cuts it to the silhouette.
    let flood = at(i).filter(|p| p.has_tag_name("feFlood"))?;
    let color = flood_color(flood, css, current)?;
    i += 1;
    let cut = at(i).filter(|p| p.has_tag_name("feComposite"))?;
    if cut.attribute("operator") != Some("in") {
        return None;
    }
    i += 1;
    // A second composite confines an inner shadow to the layer's alpha.
    if inner {
        let keep = at(i).filter(|p| p.has_tag_name("feComposite"))?;
        if keep.attribute("operator") != Some("in") {
            return None;
        }
        i += 1;
    }
    Some((
        crate::effect::Shadow {
            offset,
            blur,
            spread,
            color,
        },
        inner,
        i,
    ))
}

/// A `stdDeviation` as one number, and whether it named two unequal ones.
///
/// Zero is a value, not a refusal: a hard-edged offset shadow (`stdDeviation="0"`) is
/// an ordinary idiom the model holds exactly. Negative is refused, per SVG, and
/// `is_finite` guards junk files.
fn deviation_of(v: &str) -> Option<(f64, bool)> {
    let (dev, uneven) = match numbers(v).as_slice() {
        [s] => (*s, false),
        [x, y] => ((x + y) * 0.5, (x - y).abs() > 1e-9),
        _ => return None,
    };
    // Clamped so the radius it becomes stays within `effect::MAX_BLUR`, which
    // `Document::apply` would otherwise refuse the whole import for.
    let cap = crate::effect::MAX_BLUR * crate::effect::BLUR_DEVIATION;
    (dev.is_finite() && dev >= 0.0).then_some((dev.min(cap), uneven))
}

/// `flood-color` and `flood-opacity` as one colour.
///
/// Both are read through [`property`], since Inkscape writes filter primitives through
/// `style=""` and the shadow would otherwise default to opaque black.
fn flood_color<'a>(p: roxmltree::Node<'a, '_>, css: &Css<'a>, current: Color) -> Option<Color> {
    let base = match property(p, "flood-color", css) {
        // `currentColor` resolves against the referencing element's `color`;
        // `parse_paint_color` does not know the keyword.
        Some("currentColor") => current,
        Some(v) => parse_paint_color(v)?,
        None => Color::BLACK,
    };
    let alpha = property(p, "flood-opacity", css)
        .and_then(parse_len)
        .unwrap_or(1.0);
    Some(base.with_alpha(alpha.clamp(0.0, 1.0) as f32))
}

/// Whether reading `def`'s ink as alpha gives the same answer as luminance, which is
/// what SVG's `<mask>` means.
///
/// They agree on opaque white (the common mask) and are opposites for black.
/// Conservative: any paint not an opaque near-white colour (gradient, grey, partial
/// alpha, `currentColor`) answers `false` and the mask is reported approximated.
fn luminance_equivalent(def: roxmltree::Node<'_, '_>, css: &Css<'_>) -> bool {
    def.descendants()
        .filter(roxmltree::Node::is_element)
        .filter(|el| !matches!(el.tag_name().name(), "mask" | "clipPath" | "defs"))
        .all(|el| {
            let prop = |name: &str| property(el, name, css);
            let Some(v) = prop("fill") else {
                // No fill is SVG's initial black, where the readings disagree most.
                return false;
            };
            match parse_paint_color(v) {
                Some(c) => {
                    let [r, g, b, a] = c.components;
                    // Unlinearised on purpose: this asks "is it white".
                    a >= 0.999 && 0.2126 * r + 0.7152 * g + 0.0722 * b >= 0.99
                }
                None => v.trim() == "none",
            }
        })
}

/// The id inside a `url(#id)`, and `None` for anything else.
///
/// The paren closing `url(` is the first one: Inkscape writes a fallback after the
/// reference (`url(#g) rgb(0, 0, 0)`), and reading to the last `)` yields a bogus id.
fn reference(value: &str) -> Option<&str> {
    let rest = value.trim().strip_prefix("url(")?;
    let inner = rest[..rest.find(')')?].trim();
    inner
        .trim_matches(|c| c == '"' || c == '\'')
        .strip_prefix('#')
}

/// One `<stop>` as a colour stop, or `None` where it is not one.
///
/// The offset rejects NaN (which `f64::clamp` propagates, and `serde_json` writes as
/// `null`, so the document would never reopen) but not infinities: `1e999` means "at
/// the end" and clamps to 1.0, so `!is_nan()`, not `is_finite()`. A NaN offset drops
/// to 0.0 like a missing one rather than losing the stop.
fn stop_of(el: roxmltree::Node<'_, '_>, css: &Css<'_>) -> Option<peniko::ColorStop> {
    let prop = |name: &str| property(el, name, css);
    let offset = prop("offset")
        .and_then(|v| {
            v.strip_suffix('%')
                .and_then(|p| p.trim().parse::<f64>().ok().map(|n| n / 100.0))
                .or_else(|| v.trim().parse::<f64>().ok())
        })
        .filter(|f| !f.is_nan())
        .unwrap_or(0.0);
    let color = prop("stop-color")
        .and_then(parse_paint_color)
        .unwrap_or(Color::BLACK);
    // Clamped like `fill-opacity`: `multiply_alpha` does no range work of its own.
    let alpha = prop("stop-opacity")
        .and_then(parse_len)
        .unwrap_or(1.0)
        .clamp(0.0, 1.0) as f32;
    Some(peniko::ColorStop {
        offset: offset.clamp(0.0, 1.0) as f32,
        color: color.multiply_alpha(alpha).into(),
    })
}

/// One element's resolved style: the inherited properties, overridden by whatever
/// this element states.
fn resolve(el: roxmltree::Node<'_, '_>, inherited: &Style, gradients: &Gradients<'_, '_>) -> Style {
    let prop = |name: &str| property(el, name, &gradients.css);
    let mut s = inherited.clone();
    if let Some(v) = prop("color").and_then(parse_paint_color) {
        s.current = v;
    }
    // The parse is guarded, not assigned: `None` means unpainted, so a value peniko
    // refuses (`var(--brand)`) must leave the inherited paint standing, as CSS drops
    // an invalid declaration. Only `none`, `transparent` and a `url(#…)` with a dead
    // reference mean unpainted; the last must not fall back to the inherited paint or
    // an unresolvable image reference comes back opaque black.
    let unpainted = |v: &str| {
        let v = v.trim();
        matches!(v, "none" | "transparent") || v.starts_with("url(")
    };
    if let Some(v) = prop("fill") {
        match parse_paint(v, s.current, gradients) {
            Some(p) => s.fill = Some(p),
            None if unpainted(v) => s.fill = None,
            None => gradients.unreadable_paint.set(true),
        }
    }
    if let Some(v) = prop("stroke") {
        match parse_paint(v, s.current, gradients) {
            Some(p) => s.stroke = Some(p),
            None if unpainted(v) => s.stroke = None,
            None => gradients.unreadable_paint.set(true),
        }
    }
    if let Some(v) = prop("stroke-width").and_then(parse_len) {
        s.stroke_width = v.max(0.0);
    }
    if let Some(v) = prop("stroke-linecap") {
        s.stroke_cap = match v {
            "round" => kurbo::Cap::Round,
            "square" => kurbo::Cap::Square,
            _ => kurbo::Cap::Butt,
        };
    }
    if let Some(v) = prop("stroke-linejoin") {
        s.stroke_join = match v {
            "round" => kurbo::Join::Round,
            "bevel" => kurbo::Join::Bevel,
            _ => kurbo::Join::Miter,
        };
    }
    if let Some(v) = prop("stroke-dasharray") {
        // `none` is the way a child turns an inherited dash pattern off.
        s.dashes = if v.trim() == "none" {
            Vec::new()
        } else {
            numbers(v).into_iter().filter(|n| *n >= 0.0).collect()
        };
    }
    if let Some(v) = prop("stroke-dashoffset").and_then(parse_len) {
        s.dash_offset = v;
    }
    if let Some(v) = prop("stroke-miterlimit").and_then(parse_len) {
        s.miter_limit = v.max(1.0);
    }
    if let Some(v) = prop("fill-rule") {
        s.fill_rule = if v.trim() == "evenodd" {
            FillRule::EvenOdd
        } else {
            FillRule::NonZero
        };
    }
    if let Some(v) = prop("fill-opacity").and_then(parse_len) {
        s.fill_opacity = v.clamp(0.0, 1.0) as f32;
    }
    if let Some(v) = prop("stroke-opacity").and_then(parse_len) {
        s.stroke_opacity = v.clamp(0.0, 1.0) as f32;
    }
    // The font half, which inherits exactly as the paint half does.
    if let Some(v) = prop("font-family") {
        s.font_family = Some(v.to_string());
    }
    if let Some(v) = prop("font-size").and_then(parse_len)
        && v > 0.0
    {
        s.font_size = v;
    }
    if let Some(w) = prop("font-weight").and_then(|v| match v.trim() {
        "normal" => Some(400),
        "bold" => Some(700),
        // `bolder`/`lighter` are relative and rare in drawing-tool files.
        n => n.parse::<u16>().ok(),
    }) {
        s.weight = w;
    }
    if let Some(v) = prop("font-style") {
        s.italic = matches!(v.trim(), "italic" | "oblique");
    }
    if let Some(v) = prop("text-anchor") {
        s.anchor = match v.trim() {
            "middle" => Anchor::Middle,
            "end" => Anchor::End,
            _ => Anchor::Start,
        };
    }
    s
}

/// A `fill`/`stroke` value: `none`, a colour, `currentColor`, or a gradient
/// reference.
fn parse_paint(v: &str, current: Color, gradients: &Gradients<'_, '_>) -> Option<Paint> {
    let v = v.trim();
    if v == "none" || v == "transparent" {
        return None;
    }
    if v == "currentColor" {
        return Some(Paint::Solid(current));
    }
    if v.starts_with("url(") {
        // A reference that does not resolve uses the fallback colour, else is `none`,
        // not black.
        if let Some((g, units, xform)) = gradients.get(v) {
            return Some(Paint::Gradient(Box::new(g), units, xform));
        }
        // The fallback starts after the first `)`, as in [`reference`].
        let fallback = v
            .strip_prefix("url(")
            .and_then(|rest| rest.find(')').map(|i| rest[i + 1..].trim()))?;
        return parse_paint_color(fallback).map(Paint::Solid);
    }
    parse_paint_color(v).map(Paint::Solid)
}

/// The whole CSS colour grammar, via `peniko::color`.
fn parse_paint_color(v: &str) -> Option<Color> {
    peniko::color::parse_color(v.trim())
        .ok()
        .map(|c| c.to_alpha_color::<peniko::color::Srgb>())
}

/// A length, in the one unit the model has.
///
/// Only `px` (or none) is read; reading `10mm` as `10` would be wrong by nearly 4x,
/// so any other unit is refused and the attribute falls back to its default.
fn parse_len(v: &str) -> Option<f64> {
    let v = v.trim();
    let n = v.strip_suffix("px").unwrap_or(v);
    n.trim().parse::<f64>().ok().filter(|f| f.is_finite())
}

/// The numbers in a whitespace- or comma-separated list.
///
/// Non-finite values are dropped, as in [`parse_len`]: `1e999` is infinity, which
/// `serde_json` writes as `null`, and `Document::apply` refuses it (`OpError::NonFinite`),
/// failing the whole import. Dropped, not clamped, since no finite substitute is right.
fn numbers(v: &str) -> Vec<f64> {
    v.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f64>().ok())
        .filter(|f| f.is_finite())
        .collect()
}

fn element_transform(el: roxmltree::Node<'_, '_>) -> Affine {
    el.attribute("transform")
        .map(parse_transform)
        .unwrap_or(Affine::IDENTITY)
}

/// An SVG `transform` list, as one affine.
///
/// Composes left to right on the right (`translate(10) rotate(45)` rotates first, in
/// the translated space); angles are degrees, unlike kurbo's radians.
fn parse_transform(v: &str) -> Affine {
    let mut out = Affine::IDENTITY;
    let mut rest = v;
    while let Some(open) = rest.find('(') {
        let name = rest[..open]
            .trim_start_matches([',', ' ', '\t', '\n', '\r'])
            .trim();
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let args = numbers(&rest[open + 1..open + close]);
        rest = &rest[open + close + 1..];
        let t = match (name, args.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => Affine::new([*a, *b, *c, *d, *e, *f]),
            ("translate", [x]) => Affine::translate((*x, 0.0)),
            ("translate", [x, y]) => Affine::translate((*x, *y)),
            ("scale", [s]) => Affine::scale(*s),
            ("scale", [x, y]) => Affine::scale_non_uniform(*x, *y),
            ("rotate", [a]) => Affine::rotate(a.to_radians()),
            // Rotation about a point.
            ("rotate", [a, cx, cy]) => {
                Affine::translate((*cx, *cy))
                    * Affine::rotate(a.to_radians())
                    * Affine::translate((-*cx, -*cy))
            }
            ("skewX", [a]) => Affine::new([1.0, 0.0, a.to_radians().tan(), 1.0, 0.0, 0.0]),
            ("skewY", [a]) => Affine::new([1.0, a.to_radians().tan(), 0.0, 1.0, 0.0, 0.0]),
            _ => continue,
        };
        // A singular transform is dropped, not carried: `build::local_for_world`
        // inverts the parent's world, and a singular one yields `NaN` coefficients
        // that the next autosave writes as an unopenable file.
        // `build::affine_is_invertible` is the shared, threshold-free predicate.
        if !crate::build::affine_is_invertible(t) {
            continue;
        }
        out *= t;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::effect::EffectKind;

    /// A document with one import applied, and the nodes it made.
    fn imported(svg: &str) -> (Document, Import) {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let out = import(svg, &mut ids, root, 0, None).expect("valid SVG");
        doc.apply(&out.tx).expect("the transaction applies");
        (doc, out)
    }

    /// An `id` that is not a usable name falls through to the generated name; the
    /// fourth rect is the control that an ordinary `id` survives.
    ///
    /// flip: `element_name` back to `.map(str::to_string)` → fails at the first rect.
    #[test]
    fn an_unusable_id_falls_through_to_the_generated_name() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <rect id="" width="10" height="10"/>
                  <rect id="   " width="10" height="10"/>
                  <rect data-name="a&#10;b" width="10" height="10"/>
                  <rect id="Roof" width="10" height="10"/>
                </svg>"##,
        );
        let names: Vec<String> = shapes(&doc, &out)
            .iter()
            .map(|id| doc.get(*id).unwrap().name().to_string())
            .collect();
        assert_eq!(names.len(), 4, "the fixture is four rects");

        for (i, n) in names.iter().take(2).enumerate() {
            let stem = n.split(' ').next().unwrap_or_default();
            assert!(
                crate::naming::is_generated(stem),
                "rect {i} kept an unusable id: {n:?}"
            );
        }
        assert_eq!(names[2], "a b", "the line break should have become a space");
        assert_eq!(names[3], "Roof", "an ordinary id is untouched");
    }

    /// Every node under the import's root group, in document order.
    fn shapes(doc: &Document, out: &Import) -> Vec<NodeId> {
        fn walk(doc: &Document, id: NodeId, into: &mut Vec<NodeId>) {
            for c in doc
                .get(id)
                .map(|n| n.children().to_vec())
                .unwrap_or_default()
            {
                into.push(c);
                walk(doc, c, into);
            }
        }
        let mut v = Vec::new();
        walk(doc, out.root, &mut v);
        v
    }

    fn world_box(doc: &Document, id: NodeId) -> kurbo::Rect {
        let res = crate::Resolved::rebuild(doc);
        res.world_bounds(id).expect("a live node has bounds")
    }

    /// A transform list composes left to right and its angles are degrees.
    ///
    /// flip: `out *= t` to `out = t * out` → fails at the translate-then-rotate point.
    #[test]
    fn a_transform_list_composes_left_to_right_in_degrees() {
        let t = parse_transform("translate(10,0) rotate(90)");
        let p = t * Point::new(1.0, 0.0);
        assert!(
            (p - Point::new(10.0, 1.0)).hypot() < 1e-9,
            "rotate happens inside the translate: {p:?}"
        );
        // A rotation about a point fixes that point.
        let about = parse_transform("rotate(180 5 5)");
        let q = about * Point::new(5.0, 5.0);
        assert!((q - Point::new(5.0, 5.0)).hypot() < 1e-9, "{q:?}");
        assert!(((about * Point::new(6.0, 5.0)) - Point::new(4.0, 5.0)).hypot() < 1e-9);
    }

    /// The primitives land where the markup says; boxes are off the origin so a
    /// forgotten translate would show.
    #[test]
    fn the_primitives_land_where_the_markup_says() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <rect x="10" y="20" width="30" height="40"/>
                 <circle cx="100" cy="50" r="25"/>
                 <ellipse cx="200" cy="60" rx="40" ry="10"/>
               </svg>"#,
        );
        let ids = shapes(&doc, &out);
        assert_eq!(ids.len(), 3, "three shapes, and no group per shape");
        assert_eq!(out.shapes, 3);
        let boxes: Vec<kurbo::Rect> = ids.iter().map(|id| world_box(&doc, *id)).collect();
        assert_eq!(
            boxes[0],
            kurbo::Rect::new(10.0, 20.0, 40.0, 60.0),
            "the rect"
        );
        assert_eq!(
            boxes[1],
            kurbo::Rect::new(75.0, 25.0, 125.0, 75.0),
            "the circle"
        );
        assert_eq!(
            boxes[2],
            kurbo::Rect::new(160.0, 50.0, 240.0, 70.0),
            "the ellipse"
        );
    }

    /// A `viewBox` scales the drawing onto the declared size, which is what `size` reports.
    #[test]
    fn a_view_box_scales_the_drawing_onto_the_declared_size() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 48 48">
                 <rect x="0" y="0" width="48" height="48"/>
               </svg>"#,
        );
        assert_eq!(out.size, Some(Size::new(24.0, 24.0)));
        let id = shapes(&doc, &out)[0];
        assert_eq!(world_box(&doc, id), kurbo::Rect::new(0.0, 0.0, 24.0, 24.0));
    }

    /// A non-positive `width`/`height` is no declaration to either reader, so the
    /// drawing pastes at its own units instead of collapsing or flipping.
    ///
    /// flip: `declared_pair`'s `w > 0.0 && h > 0.0` dropped → fails at the size assertion for `24×0`.
    #[test]
    fn a_declared_size_that_is_not_a_size_leaves_the_drawing_at_its_own_units() {
        let at = |w: &str, h: &str| {
            let (doc, out) = imported(&format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 48 48">
                     <rect x="0" y="0" width="48" height="48"/>
                   </svg>"#
            ));
            let id = shapes(&doc, &out)[0];
            (out.size, world_box(&doc, id))
        };

        for (w, h, why) in [
            ("24", "0", "a zero height is no declaration"),
            ("-24", "-24", "and neither is a negative pair"),
            ("0", "0", "nor two zeroes"),
        ] {
            let (size, box_) = at(w, h);
            assert_eq!(
                size,
                Some(Size::new(48.0, 48.0)),
                "{why}: the size falls through to the viewBox"
            );
            assert_eq!(
                box_,
                kurbo::Rect::new(0.0, 0.0, 48.0, 48.0),
                "{why}: and so does the geometry, rather than collapsing or flipping"
            );
        }

        assert_eq!(
            at("24", "24"),
            (
                Some(Size::new(24.0, 24.0)),
                kurbo::Rect::new(0.0, 0.0, 24.0, 24.0)
            ),
            "control: a positive pair is still fitted onto"
        );
    }

    /// Paint inherits down groups, and an inline `style` beats a presentation attribute.
    #[test]
    fn paint_inherits_and_an_inline_style_wins() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <g fill="#ff0000">
                   <rect width="10" height="10" fill="#0000ff" style="fill:#00ff00"/>
                   <rect width="10" height="10"/>
                 </g>
               </svg>"##,
        );
        let ids = shapes(&doc, &out);
        // Four channels, not a packed integer: `Rgba8::to_u32` packs little-endian.
        let fill = |id: NodeId| match doc.get(id).unwrap().paint().fills[0].brush.literal() {
            Some(Brush::Solid(c)) => c.to_rgba8().to_u8_array(),
            other => panic!("a solid fill, got {other:?}"),
        };
        // `ids[0]` is the group, which takes no paint of its own.
        assert_eq!(fill(ids[1]), [0, 255, 0, 255], "style beats the attribute");
        assert_eq!(
            fill(ids[2]),
            [255, 0, 0, 255],
            "and the group's red inherits"
        );
    }

    /// `fill="none"` is unpainted, while the default with no `fill` is black.
    #[test]
    fn none_is_unpainted_and_the_default_is_black() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <rect width="10" height="10" fill="none" stroke="black"/>
                 <rect width="10" height="10"/>
               </svg>"#,
        );
        let ids = shapes(&doc, &out);
        assert!(
            doc.get(ids[0]).unwrap().paint().fills.is_empty(),
            "no fill at all, rather than a transparent one"
        );
        assert_eq!(doc.get(ids[0]).unwrap().paint().strokes.len(), 1);
        assert_eq!(
            doc.get(ids[1]).unwrap().paint().fills.len(),
            1,
            "and an unpainted rect is black, which is SVG's initial value"
        );
    }

    /// `fill-rule="evenodd"` is carried, so a compound path keeps its holes.
    #[test]
    fn the_even_odd_rule_is_carried() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <path d="M0 0h100v100h-100z M30 30h40v40h-40z" fill-rule="evenodd"/>
               </svg>"#,
        );
        let id = shapes(&doc, &out)[0];
        assert_eq!(doc.get(id).unwrap().fill_rule(), FillRule::EvenOdd);
        let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
            panic!("a Path")
        };
        assert!(!FillRule::EvenOdd.contains(path, Point::new(50.0, 50.0)));
        assert!(FillRule::EvenOdd.contains(path, Point::new(10.0, 50.0)));
    }

    /// What it cannot read is reported once per kind, not dropped.
    #[test]
    fn unsupported_elements_are_counted_once_each() {
        let (_doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <rect width="10" height="10"/>
                 <image href="a.png" width="10" height="10"/>
                 <image href="b.png" width="10" height="10"/>
                 <foreignObject width="10" height="10"/>
               </svg>"#,
        );
        assert_eq!(
            out.skipped,
            vec!["image (external)", "foreignObject"],
            "one entry per *kind* of loss, however many times it occurs — and an \
             image says why it was skipped, a file this paste never had"
        );
        assert_eq!(out.shapes, 1, "and what it could read still arrived");
    }

    /// A sink that accepts anything; the id is the byte length, so one payload twice
    /// is the same picture.
    struct FakeImages(usize);

    impl ImageSink for FakeImages {
        fn place(
            &mut self,
            bytes: Vec<u8>,
            format: crate::image::ImageFormat,
        ) -> Option<(crate::image::ImageId, crate::image::ImageEntry)> {
            self.0 += 1;
            let id = crate::image::ImageId(format!("test:{}", bytes.len()));
            Some((
                id,
                crate::image::ImageEntry {
                    source: crate::image::ImageSource::Embedded(bytes.into()),
                    format,
                    width: 10,
                    height: 5,
                },
            ))
        }
    }

    fn imported_with(svg: &str, sink: &mut FakeImages) -> (Document, Import) {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let out = import(svg, &mut ids, root, 0, Some(sink)).expect("valid SVG");
        doc.apply(&out.tx).expect("the transaction applies");
        (doc, out)
    }

    /// An `<image>` becomes a `Rect` with an image fill, fitted `Fit` because SVG's
    /// default `meet` is contain, not the field's own default.
    #[test]
    fn an_image_becomes_a_rect_with_an_image_fill() {
        let mut sink = FakeImages(0);
        let (doc, out) = imported_with(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <image x="5" y="7" width="40" height="20"
                         href="data:image/png;base64,AAECAwQ="/>
                </svg>"##,
            &mut sink,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert_eq!(sink.0, 1, "the bytes reached the host once");
        let id = shapes(&doc, &out)[0];
        assert_eq!(
            world_box(&doc, id),
            kurbo::Rect::new(5.0, 7.0, 45.0, 27.0),
            "at its own x and y, at its own size"
        );
        let Some(Brush::Image(b)) = doc.get(id).unwrap().paint().fills[0].brush.literal() else {
            panic!("an image fill")
        };
        assert_eq!(
            b.image.fit,
            crate::image::ImageFit::Fit,
            "SVG's default is contain, which is not this field's default"
        );
    }

    /// The same picture twice is one table entry and two fills.
    #[test]
    fn one_picture_used_twice_is_added_to_the_table_once() {
        let mut sink = FakeImages(0);
        let (_doc, out) = imported_with(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <image width="10" height="10" href="data:image/png;base64,AAECAwQ="/>
                  <image x="20" width="10" height="10" href="data:image/png;base64,AAECAwQ="/>
                </svg>"##,
            &mut sink,
        );
        assert_eq!(out.shapes, 2, "two layers");
        let adds = out
            .tx
            .0
            .iter()
            .filter(|op| matches!(op, Operation::AddImage { .. }))
            .count();
        assert_eq!(adds, 1, "and one entry in the table");
    }

    /// Without a sink an `<image>` is skipped and counted.
    #[test]
    fn an_image_with_nowhere_to_go_is_skipped() {
        let (_doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <image width="10" height="10" href="data:image/png;base64,AAECAwQ="/>
                </svg>"##,
        );
        assert_eq!(out.skipped, vec!["image"]);
        assert_eq!(out.shapes, 0);
    }

    /// A `<defs>` is read by lookup and never walked.
    #[test]
    fn defs_contribute_no_shapes() {
        let (_doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs><rect id="tpl" width="99" height="99"/></defs>
                 <rect width="10" height="10"/>
               </svg>"#,
        );
        assert_eq!(out.shapes, 1, "the template is not a shape");
    }

    /// `stop-opacity` is clamped into `0..=1`; both ends are asserted so a bare `min` fails.
    #[test]
    fn a_stop_opacity_outside_zero_to_one_is_clamped_on_import() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs>
                   <linearGradient id="g">
                     <stop offset="0" stop-color="red" stop-opacity="5"/>
                     <stop offset="0.5" stop-color="lime" stop-opacity="-1"/>
                     <stop offset="1" stop-color="blue" stop-opacity="0.25"/>
                   </linearGradient>
                 </defs>
                 <rect width="50" height="20" fill="url(#g)"/>
               </svg>"#,
        );
        let id = shapes(&doc, &out)[0];
        let Some(Brush::Gradient(g)) = doc.get(id).unwrap().paint().fills[0].brush.literal() else {
            panic!("a gradient fill")
        };
        let alphas: Vec<f32> = g
            .gradient
            .stops
            .iter()
            .map(|s| s.color.to_alpha_color::<peniko::color::Srgb>().components[3])
            .collect();
        assert_eq!(
            alphas.len(),
            3,
            "the fixture must reach the state: three stops"
        );
        assert_eq!(alphas[0], 1.0, "5 clamps to the ceiling");
        assert_eq!(alphas[1], 0.0, "-1 clamps to the floor");
        assert!(
            (alphas[2] - 0.25).abs() < 1e-6,
            "and an in-range value is untouched, got {}",
            alphas[2]
        );
    }

    /// A bounding-box gradient (SVG's default units) resolves against the shape it is on.
    #[test]
    fn a_bounding_box_gradient_is_mapped_onto_the_shape() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs>
                   <linearGradient id="g"><stop offset="0" stop-color="red"/>
                     <stop offset="1" stop-color="blue"/></linearGradient>
                 </defs>
                 <rect x="100" y="0" width="50" height="20" fill="url(#g)"/>
               </svg>"#,
        );
        let id = shapes(&doc, &out)[0];
        let Some(Brush::Gradient(g)) = doc.get(id).unwrap().paint().fills[0].brush.literal() else {
            panic!("a gradient fill")
        };
        let peniko::GradientKind::Linear(l) = g.gradient.kind else {
            panic!("linear")
        };
        // The rect's `x` is on its transform, so the box is 0..50, not 100..150.
        assert!((l.start.x - 0.0).abs() < 1e-9, "{:?}", l.start);
        assert!((l.end.x - 50.0).abs() < 1e-9, "{:?}", l.end);
        assert_eq!(g.gradient.stops.len(), 2);
        assert_eq!(
            g.transform,
            Affine::IDENTITY,
            "a linear mapping is baked into the two points, so nothing rides along"
        );
    }

    /// A duplicate id is first-wins in every id map; `#h` inherits from `#g` through
    /// `ids`, so the `href` path and the fill path must agree (Chrome paints red).
    ///
    /// flip: `by_id.entry(id).or_insert(…)` back to `by_id.insert(id, …)` → fails at the `seen` assertion.
    #[test]
    fn a_duplicate_gradient_id_resolves_to_the_first_definition() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
                 <defs>
                   <linearGradient id="g"><stop offset="0" stop-color="red"/>
                     <stop offset="1" stop-color="red"/></linearGradient>
                   <linearGradient id="g"><stop offset="0" stop-color="blue"/>
                     <stop offset="1" stop-color="blue"/></linearGradient>
                   <linearGradient id="h" xlink:href="#g"/>
                 </defs>
                 <rect width="50" height="20" fill="url(#g)"/>
                 <rect width="50" height="20" fill="url(#h)"/>
               </svg>"##,
        );
        let ids = shapes(&doc, &out);
        assert_eq!(ids.len(), 2, "the fixture is two rects");

        // Both read before either is asserted, so a failure shows the pair.
        let seen: Vec<&str> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let Some(Brush::Gradient(g)) =
                    doc.get(*id).unwrap().paint().fills[0].brush.literal()
                else {
                    panic!("rect {i}: a gradient fill")
                };
                let c = g.gradient.stops[0].color;
                match (c.components[0], c.components[2]) {
                    (1.0, 0.0) => "red",
                    (0.0, 1.0) => "blue",
                    _ => panic!("rect {i}: neither fixture colour: {c:?}"),
                }
            })
            .collect();
        assert_eq!(
            seen,
            ["red", "red"],
            "url(#g) and url(#h) must resolve to the same first definition"
        );
    }

    /// The linear gradient on the first shape under the import's root.
    fn first_linear(
        doc: &Document,
        out: &Import,
    ) -> (peniko::Gradient, peniko::LinearGradientPosition) {
        let id = shapes(doc, out)[0];
        let Some(Brush::Gradient(g)) = doc.get(id).unwrap().paint().fills[0].brush.literal() else {
            panic!("a gradient fill")
        };
        let peniko::GradientKind::Linear(l) = g.gradient.kind else {
            panic!("linear")
        };
        (g.gradient.clone(), l)
    }

    /// A gradient inherits stops and position through `xlink:href`; the stops sit
    /// behind the href and the position in front, so inheriting only one fails.
    #[test]
    fn a_gradient_inherits_stops_and_position_through_xlink_href() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
                 <defs>
                   <linearGradient id="base" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="40" y2="0">
                     <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
                   </linearGradient>
                   <linearGradient id="g" xlink:href="#base"/>
                 </defs>
                 <rect width="50" height="20" fill="url(#g)"/>
               </svg>"##,
        );
        let (g, l) = first_linear(&doc, &out);
        assert_eq!(g.stops.len(), 2, "the stops came from behind the href");
        assert!(
            (l.end.x - 40.0).abs() < 1e-9,
            "and so did x2, rather than defaulting to 1: {l:?}"
        );
    }

    /// A gradient `href` cycle ends rather than looping.
    #[test]
    fn a_gradient_href_cycle_stops_rather_than_recursing_forever() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
                 <defs>
                   <linearGradient id="a" xlink:href="#b"/>
                   <linearGradient id="b" xlink:href="#a"><stop offset="0" stop-color="red"/>
                     <stop offset="1" stop-color="blue"/></linearGradient>
                 </defs>
                 <rect width="50" height="20" fill="url(#a)"/>
               </svg>"##,
        );
        assert_eq!(first_linear(&doc, &out).0.stops.len(), 2);
    }

    /// A `userSpaceOnUse` gradient has the shape's placement mapped back out of it.
    #[test]
    fn a_user_space_gradient_lands_in_the_shapes_own_space() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs>
                   <linearGradient id="g" gradientUnits="userSpaceOnUse" x1="100" y1="0" x2="150" y2="0">
                     <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
                   </linearGradient>
                 </defs>
                 <rect x="100" y="0" width="50" height="20" fill="url(#g)"/>
               </svg>"##,
        );
        let (_, l) = first_linear(&doc, &out);
        assert!((l.start.x - 0.0).abs() < 1e-9, "{:?}", l.start);
        assert!((l.end.x - 50.0).abs() < 1e-9, "{:?}", l.end);
    }

    /// `gradientTransform` composes inside the `gradientUnits` mapping: `scale(2,1)`
    /// over `x2="25"` reaches 50, where a dropped transform lands at 25.
    #[test]
    fn a_gradient_transform_applies_inside_the_units_mapping() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs>
                   <linearGradient id="g" gradientUnits="userSpaceOnUse"
                                   gradientTransform="scale(2,1)" x1="0" y1="0" x2="25" y2="0">
                     <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
                   </linearGradient>
                 </defs>
                 <rect width="50" height="20" fill="url(#g)"/>
               </svg>"##,
        );
        let (_, l) = first_linear(&doc, &out);
        assert!((l.end.x - 50.0).abs() < 1e-9, "{:?}", l.end);
    }

    /// A non-uniform transform keeps a linear gradient's bands square to its own space:
    /// squashing y by 1/4 under a 45° gradient tilts the vector to 76°, not the naive
    /// 14° of the mapped endpoints.
    ///
    /// flip: the covector replaced with `p(l.end)` → fails at the `l.end` assertion (the only test that bites; the axis-aligned ones stay green).
    #[test]
    fn a_squashed_transform_keeps_a_linear_gradients_bands_square_to_its_own_space() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs>
                   <linearGradient id="g" gradientUnits="userSpaceOnUse"
                                   gradientTransform="scale(1,0.25)" x1="0" y1="0" x2="40" y2="40">
                     <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
                   </linearGradient>
                 </defs>
                 <rect width="40" height="40" fill="url(#g)"/>
               </svg>"##,
        );
        let (_, l) = first_linear(&doc, &out);
        // u = Tₗᵢₙ⁻ᵀ·d/|d|² = (1, 4)·40/3200 = (0.0125, 0.05); |u|² = 0.0026563
        assert!((l.start.to_vec2()).hypot() < 1e-9, "{:?}", l.start);
        assert!(
            (l.end.x - 4.705_882_35).abs() < 1e-6 && (l.end.y - 18.823_529_4).abs() < 1e-6,
            "the bands stay at 45° in gradient space, so the vector tilts: {:?}",
            l.end
        );
    }

    /// A radius scales by `sqrt(|det|)`, so a quarter-turn rotation keeps it.
    ///
    /// flip: `(|a| + |d|) / 2` → radius 0, fails at the `end_radius` assertion.
    #[test]
    fn a_rotated_gradient_transform_keeps_a_radial_gradients_radius() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs>
                   <radialGradient id="g" gradientUnits="userSpaceOnUse"
                                   gradientTransform="matrix(0, 2, -2, 0, 0, 0)"
                                   cx="0" cy="0" r="10">
                     <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
                   </radialGradient>
                 </defs>
                 <rect width="50" height="50" fill="url(#g)"/>
               </svg>"##,
        );
        let id = shapes(&doc, &out)[0];
        let Some(Brush::Gradient(g)) = doc.get(id).unwrap().paint().fills[0].brush.literal() else {
            panic!("a gradient fill")
        };
        let peniko::GradientKind::Radial(r) = g.gradient.kind else {
            panic!("radial")
        };
        assert!(
            (r.end_radius - 20.0).abs() < 1e-4,
            "a quarter turn at 2× is still 2×: {}",
            r.end_radius
        );
        assert!(
            out.approximated.is_empty(),
            "and a rotation is exact, so nothing is approximated: {:?}",
            out.approximated
        );
    }

    /// A squashed radial gradient is exact: the circle is unmapped (radius 10) and the
    /// squash rides on the brush's transform.
    #[test]
    fn a_squashed_radial_gradient_is_exact_rather_than_drawn_round() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs>
                   <radialGradient id="g" gradientUnits="userSpaceOnUse"
                                   gradientTransform="scale(1, 6)" cx="0" cy="0" r="10">
                     <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
                   </radialGradient>
                 </defs>
                 <rect width="50" height="50" fill="url(#g)"/>
               </svg>"##,
        );
        assert!(
            out.approximated.is_empty(),
            "nothing is approximated any more: {:?}",
            out.approximated
        );
        let id = shapes(&doc, &out)[0];
        let Some(Brush::Gradient(g)) = doc.get(id).unwrap().paint().fills[0].brush.literal() else {
            panic!("a gradient fill")
        };
        let peniko::GradientKind::Radial(r) = g.gradient.kind else {
            panic!("radial")
        };
        assert!(
            (r.end_radius - 10.0).abs() < 1e-4,
            "the circle is the file's own, unmapped: {}",
            r.end_radius
        );
        let [a, b, c, d, _, _] = g.transform.as_coeffs();
        assert!(
            (a - 1.0).abs() < 1e-9 && b.abs() < 1e-9 && c.abs() < 1e-9 && (d - 6.0).abs() < 1e-9,
            "and the squash is on the brush: {:?}",
            g.transform.as_coeffs()
        );
    }

    /// A 2% squash is carried on the brush transform and a similarity is baked.
    ///
    /// flip: threshold `1.000_001` back to `1.05` → fails at the first assertion; dropping it to `1.0` does not bite the second (the ratio of a similarity is exactly 1.0).
    #[test]
    fn a_squash_under_the_old_deadband_is_carried_and_a_similarity_is_not() {
        let of = |xform: &str| {
            let (doc, out) = imported(&format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg">
                     <defs>
                       <radialGradient id="g" gradientUnits="userSpaceOnUse"
                                       gradientTransform="{xform}" cx="0" cy="0" r="10">
                         <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
                       </radialGradient>
                     </defs>
                     <rect width="50" height="50" fill="url(#g)"/>
                   </svg>"##
            ));
            let id = shapes(&doc, &out)[0];
            let Some(Brush::Gradient(g)) = doc.get(id).unwrap().paint().fills[0].brush.literal()
            else {
                panic!("a gradient fill")
            };
            g.transform
        };
        assert_ne!(
            of("scale(1, 1.02)"),
            Affine::IDENTITY,
            "a squash too small to see is still a squash"
        );
        // A similarity: the two circles hold it exactly.
        assert_eq!(
            of("rotate(37) scale(3)"),
            Affine::IDENTITY,
            "a similarity is baked, or every rotated gradient grows a field it does \
             not need and rewrites its document on the next save"
        );
    }

    /// `spreadMethod` becomes the gradient's `extend`.
    #[test]
    fn spread_method_becomes_the_gradients_extend() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs>
                   <linearGradient id="g" spreadMethod="reflect">
                     <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
                   </linearGradient>
                 </defs>
                 <rect width="50" height="20" fill="url(#g)"/>
               </svg>"##,
        );
        assert_eq!(first_linear(&doc, &out).0.extend, peniko::Extend::Reflect);
    }

    /// A paint's `url()` closes at its first paren: the first rect resolves despite a
    /// fallback, the second falls back, the third has a paren-free fallback.
    #[test]
    fn a_url_paint_reads_past_its_fallback_colour() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs>
                   <linearGradient id="g"><stop offset="0" stop-color="red"/>
                     <stop offset="1" stop-color="blue"/></linearGradient>
                 </defs>
                 <rect width="50" height="20" style="fill: url(#g) rgb(0, 0, 0);"/>
                 <rect width="50" height="20" style="fill: url(#gone) rgb(0, 128, 0);"/>
                 <rect width="50" height="20" fill="url(#gone) #00ff00"/>
               </svg>"##,
        );
        let ids = shapes(&doc, &out);
        let brush = |i: usize| doc.get(ids[i]).unwrap().paint().fills[0].brush.clone();
        assert!(
            matches!(brush(0).literal(), Some(Brush::Gradient(_))),
            "the reference resolves despite the fallback beside it"
        );
        assert_eq!(
            brush(1),
            Brush::Solid(Color::from_rgb8(0, 128, 0)).into(),
            "and an unresolvable one falls back to the colour, parens and all"
        );
        assert_eq!(brush(2), Brush::Solid(Color::from_rgb8(0, 255, 0)).into());
    }

    /// An unequal `rx`/`ry` picks a rect, an ellipse or a path; the kind is asserted
    /// because a stadium and an ellipse share a bounding box.
    #[test]
    fn an_unequal_rx_and_ry_pick_the_shape_that_is_exact() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <rect width="15" height="130" rx="7.5" ry="65"/>
                 <rect width="40" height="130" rx="2.5" ry="65"/>
                 <rect width="40" height="130" rx="6" ry="6"/>
               </svg>"##,
        );
        let ids = shapes(&doc, &out);
        let kind = |i: usize| doc.get(ids[i]).unwrap().kind().clone();
        assert!(
            matches!(kind(0), NodeKind::Ellipse { size } if size == Size::new(15.0, 130.0)),
            "both radii reach half their side: {:?}",
            kind(0)
        );
        assert!(
            matches!(kind(1), NodeKind::Path { .. }),
            "only one does, so nothing else is exact: {:?}",
            kind(1)
        );
        assert!(
            matches!(kind(2), NodeKind::Rect { .. }),
            "and an equal pair is still a rect: {:?}",
            kind(2)
        );
        assert_eq!(
            world_box(&doc, ids[1]),
            kurbo::Rect::new(0.0, 0.0, 40.0, 130.0)
        );
    }

    /// The first node's effect stack, named apart from the free `effects_of`, which
    /// reads the markup.
    fn stack_of(doc: &Document, out: &Import) -> Vec<crate::effect::Effect> {
        crate::item::values(doc.get(shapes(doc, out)[0]).unwrap().effects())
    }

    /// A lone `<feGaussianBlur>` is a `LayerBlur` (`stdDeviation="3"` is radius 6), in
    /// the attribute and both `style` spellings.
    #[test]
    fn a_lone_gaussian_blur_becomes_a_layer_blur_in_either_spelling() {
        for paint in [
            r#"filter="url(#f)""#,
            r#"style="filter:url(#f)""#,
            r#"style="fill:red;filter:url(#f);stroke:none""#,
        ] {
            let (doc, out) = imported(&format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg">
                     <defs><filter id="f"><feGaussianBlur stdDeviation="3"/></filter></defs>
                     <rect width="50" height="20" {paint}/>
                   </svg>"##
            ));
            let fx = stack_of(&doc, &out);
            assert!(
                matches!(
                    fx.as_slice(),
                    [crate::effect::Effect {
                        kind: EffectKind::LayerBlur { radius },
                        visible: true,
                        vars: [None, None, None, None],
                    }] if (radius - 6.0).abs() < 1e-9
                ),
                "{paint} → {fx:?}"
            );
            assert!(out.skipped.is_empty(), "{paint} → {:?}", out.skipped);
        }
    }

    /// An absurd finite `stdDeviation` imports as a blur clamped to
    /// `effect::MAX_BLUR`, not a refused import. Flip: `deviation_of` without its
    /// `min` → the import's `apply` refuses with `BlurTooLarge` and this panics.
    #[test]
    fn an_absurd_deviation_imports_clamped_to_the_blur_cap() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs><filter id="f"><feGaussianBlur stdDeviation="1e30"/></filter></defs>
                 <rect width="50" height="20" filter="url(#f)"/>
               </svg>"##,
        );
        let fx = stack_of(&doc, &out);
        assert!(
            matches!(
                fx.as_slice(),
                [crate::effect::Effect { kind: EffectKind::LayerBlur { radius }, .. }]
                    if *radius == crate::effect::MAX_BLUR
            ),
            "{fx:?}"
        );
    }

    /// `clip-path` is read as a property too; `clip-path: none` is not reported as a
    /// lost clip.
    #[test]
    fn a_clip_spelled_in_a_style_is_read_like_one_spelled_as_an_attribute() {
        let clipped = |paint: &str| {
            let (doc, out) = imported(&format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg">
                     <defs><clipPath id="c"><rect width="40" height="40"/></clipPath></defs>
                     <rect width="100" height="100" {paint}/>
                   </svg>"##
            ));
            let ids = shapes(&doc, &out);
            let masks = ids
                .iter()
                .filter(|id| doc.get(**id).unwrap().mask())
                .count();
            (masks, out.skipped.clone())
        };
        assert_eq!(clipped(r#"clip-path="url(#c)""#).0, 1, "the attribute");
        assert_eq!(clipped(r#"style="clip-path:url(#c)""#).0, 1, "the style");
        assert_eq!(
            clipped(r#"style="clip-path:none""#),
            (0, Vec::new()),
            "and `none` is not a clip that went missing"
        );
    }

    /// A blur on a `<g>` lands on the group, not its children.
    #[test]
    fn a_blur_on_a_group_lands_on_the_group() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs><filter id="f"><feGaussianBlur stdDeviation="2"/></filter></defs>
                 <g filter="url(#f)"><rect width="10" height="10"/><rect width="10" height="10"/></g>
               </svg>"##,
        );
        let group = doc.get(out.root).unwrap().children()[0];
        assert_eq!(doc.get(group).unwrap().effects().len(), 1, "on the group");
        for child in doc.get(group).unwrap().children() {
            assert!(
                doc.get(*child).unwrap().effects().is_empty(),
                "and not pushed down onto the children"
            );
        }
    }

    /// A filter this cannot hold is skipped whole and reported, never half read (an
    /// unknown primitive after a readable one, a dead reference, `objectBoundingBox`
    /// primitive units).
    ///
    /// flip: `effects_of`'s final `return None` to `i += 1` → fails at the first def's emptiness assertion.
    #[test]
    fn a_filter_this_cannot_hold_is_skipped_rather_than_half_read() {
        for def in [
            r#"<filter id="f"><feGaussianBlur stdDeviation="2"/><feOffset dx="3"/></filter>"#,
            r#"<filter id="f"><feColorMatrix in="SourceGraphic" type="matrix"
                 values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 1 0"/></filter>"#,
            r#"<filter id="f">
                 <feColorMatrix in="SourceGraphic" type="matrix"
                                values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0" result="a"/>
                 <feOffset in="a" dx="2" dy="2" result="o"/>
                 <feFlood flood-color="black" result="f"/>
                 <feComposite in="f" in2="o" operator="in" result="c"/>
                 <feTurbulence baseFrequency="0.1"/>
               </filter>"#,
            r#"<filter id="f"><feTurbulence baseFrequency="0.1"/></filter>"#,
            r#"<filter id="f"/>"#,
            r#"<filter id="f" primitiveUnits="objectBoundingBox"><feGaussianBlur stdDeviation="0.1"/></filter>"#,
            r#"<linearGradient id="f"><stop offset="0" stop-color="red"/></linearGradient>"#,
        ] {
            let (doc, out) = imported(&format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg">
                     <defs>{def}</defs>
                     <rect width="50" height="20" filter="url(#f)"/>
                   </svg>"##
            ));
            assert!(stack_of(&doc, &out).is_empty(), "{def}");
            assert_eq!(out.skipped, vec!["filter"], "{def}");
        }
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <rect width="50" height="20" filter="url(#gone)"/>
               </svg>"##,
        );
        assert!(stack_of(&doc, &out).is_empty());
        assert_eq!(out.skipped, vec!["filter"]);
    }

    /// An unequal `stdDeviation` pair is averaged and reported; an equal pair is silent.
    #[test]
    fn an_uneven_gaussian_blur_is_averaged_and_reported() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs><filter id="f"><feGaussianBlur stdDeviation="2 6"/></filter></defs>
                 <rect width="50" height="20" filter="url(#f)"/>
               </svg>"##,
        );
        assert!(
            matches!(stack_of(&doc, &out)[0].kind, EffectKind::LayerBlur { radius } if (radius - 8.0).abs() < 1e-9),
            "the mean of 2 and 6 is 4, and the radius is twice that"
        );
        assert_eq!(out.approximated, vec!["uneven feGaussianBlur (averaged)"]);

        let (_doc, even) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs><filter id="f"><feGaussianBlur stdDeviation="2 2"/></filter></defs>
                 <rect width="50" height="20" filter="url(#f)"/>
               </svg>"##,
        );
        assert!(even.approximated.is_empty(), "{:?}", even.approximated);
    }

    /// One `<filter>` around a `<rect>`, for the shadow tests below.
    fn filtered(def: &str) -> (Document, Import) {
        imported(&format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs><filter id="f">{def}</filter></defs>
                 <rect width="50" height="20" filter="url(#f)"/>
               </svg>"##
        ))
    }

    /// The stack's `n`th shadow, drop or inner, with which it is.
    fn shadow(fx: &[crate::effect::Effect], n: usize) -> (crate::effect::Shadow, bool) {
        match &fx[n].kind {
            EffectKind::DropShadow(s) => (s.clone(), false),
            EffectKind::InnerShadow(s) => (s.clone(), true),
            other => panic!("a shadow, not {other:?}"),
        }
    }

    /// A shadow chain (silhouette, offset, blur, flood cut back) is a `DropShadow`;
    /// the alpha comes from `flood-opacity`, not the `feFlood`.
    ///
    /// flip: `with_alpha` dropped in `flood_color` → fails at the alpha assertion (and the `feDropShadow`, `style` and round-trip tests).
    #[test]
    fn a_shadow_chain_becomes_a_drop_shadow_with_its_offset_blur_and_colour() {
        let (doc, out) = filtered(
            r#"<feColorMatrix in="SourceGraphic" type="matrix"
                              values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0" result="a"/>
               <feOffset in="a" dx="4" dy="6" result="o"/>
               <feGaussianBlur in="o" stdDeviation="4" result="b"/>
               <feFlood flood-color="rebeccapurple" flood-opacity="0.5" result="fl"/>
               <feComposite in="fl" in2="b" operator="in" result="c"/>
               <feMerge><feMergeNode in="c"/><feMergeNode in="SourceGraphic"/></feMerge>"#,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let fx = stack_of(&doc, &out);
        assert_eq!(fx.len(), 1, "{fx:?}");
        let (s, inner) = shadow(&fx, 0);
        assert!(
            !inner,
            "the silhouette was not inverted, so it casts outward"
        );
        assert_eq!(s.offset, kurbo::Vec2::new(4.0, 6.0));
        assert!(
            (s.blur - 8.0).abs() < 1e-9,
            "deviation 4 is a radius of 8: {}",
            s.blur
        );
        assert_eq!(s.spread, 0.0, "no feMorphology means no spread");
        assert!(
            (s.color.components[3] - 0.5).abs() < 1e-6,
            "and `flood-opacity` is the alpha, not the `feFlood`'s own: {:?}",
            s.color
        );
    }

    /// An inverted silhouette is an inner shadow and the dilation's sign flips with it;
    /// all four combinations are asserted.
    ///
    /// flip: `if dilate != inner` to `if dilate` → fails at the `true dilate` case (`3.0` against `-3.0`).
    #[test]
    fn an_inverted_silhouette_is_an_inner_shadow_and_the_spread_keeps_its_sign() {
        let chain = |invert: bool, op: &str| {
            let inversion = if invert {
                r#"<feComponentTransfer in="a" result="t">
                     <feFuncA type="table" tableValues="1 0"/>
                   </feComponentTransfer>"#
            } else {
                ""
            };
            let src = if invert { "t" } else { "a" };
            let confine = if invert {
                r#"<feComposite in="c" in2="a" operator="in" result="k"/>"#
            } else {
                ""
            };
            filtered(&format!(
                r#"<feColorMatrix in="SourceGraphic" type="matrix"
                                  values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0" result="a"/>
                   {inversion}
                   <feMorphology in="{src}" operator="{op}" radius="3" result="m"/>
                   <feOffset in="m" dx="1" dy="1" result="o"/>
                   <feFlood flood-color="black" flood-opacity="1" result="fl"/>
                   <feComposite in="fl" in2="o" operator="in" result="c"/>
                   {confine}"#
            ))
        };
        for (invert, op, want) in [
            (false, "dilate", 3.0),
            (false, "erode", -3.0),
            (true, "dilate", -3.0),
            (true, "erode", 3.0),
        ] {
            let (doc, out) = chain(invert, op);
            assert!(out.skipped.is_empty(), "{invert} {op} → {:?}", out.skipped);
            let fx = stack_of(&doc, &out);
            let (s, inner) = shadow(&fx, 0);
            assert_eq!(inner, invert, "{invert} {op}: which shadow it is");
            assert_eq!(s.spread, want, "{invert} {op}: the spread's sign");
        }
    }

    /// `<feDropShadow>` is one primitive; its spec defaults (offset 2, deviation 2,
    /// opaque black) are asserted so an omitted attribute is not a zero.
    #[test]
    fn a_lone_fe_drop_shadow_is_one_primitive_saying_the_whole_thing() {
        let (doc, out) = filtered(
            r#"<feDropShadow dx="3" dy="5" stdDeviation="2"
                             flood-color="red" flood-opacity="0.25"/>"#,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let (s, inner) = shadow(&stack_of(&doc, &out), 0);
        assert!(!inner);
        assert_eq!(s.offset, kurbo::Vec2::new(3.0, 5.0));
        assert!((s.blur - 4.0).abs() < 1e-9, "{}", s.blur);
        assert_eq!(s.spread, 0.0);
        assert_eq!(s.color.components[0], 1.0, "red: {:?}", s.color);
        assert!((s.color.components[3] - 0.25).abs() < 1e-6, "{:?}", s.color);

        let (doc, out) = filtered(r#"<feDropShadow/>"#);
        let (s, _) = shadow(&stack_of(&doc, &out), 0);
        assert_eq!(
            s.offset,
            kurbo::Vec2::new(2.0, 2.0),
            "the spec's defaults, not zero"
        );
        assert!((s.blur - 4.0).abs() < 1e-9, "{}", s.blur);
        assert_eq!(s.color, Color::BLACK);
    }

    /// A `stdDeviation` of zero is a hard-edged shadow or a disabled blur (nothing
    /// skipped), and a negative one is still refused.
    ///
    /// flip: `>= 0.0` back to `> 0.0` in `deviation_of` → fails at the first `skipped.is_empty()`.
    #[test]
    fn a_zero_deviation_is_a_hard_edge_rather_than_an_unreadable_filter() {
        let (doc, out) = filtered(r#"<feDropShadow dx="1" dy="1" stdDeviation="0"/>"#);
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let fx = stack_of(&doc, &out);
        assert_eq!(fx.len(), 1, "a hard-edged shadow is a shadow");
        let (s, inner) = shadow(&fx, 0);
        assert!(!inner);
        assert_eq!(s.offset, kurbo::Vec2::new(1.0, 1.0));
        assert_eq!(s.blur, 0.0, "and its blur is the zero the file stated");

        let (doc, out) = filtered(r#"<feDropShadow dx="1" dy="1" stdDeviation="2"/>"#);
        assert_eq!(shadow(&stack_of(&doc, &out), 0).0.blur, 4.0);

        // A lone zero blur is the identity: no effect and no complaint.
        let (doc, out) = filtered(r#"<feGaussianBlur stdDeviation="0"/>"#);
        assert!(
            out.skipped.is_empty(),
            "a disabled primitive is not a skipped filter: {:?}",
            out.skipped
        );
        assert!(stack_of(&doc, &out).is_empty(), "and it adds nothing");
        let (doc, out) = filtered(r#"<feGaussianBlur stdDeviation="2"/>"#);
        assert_eq!(stack_of(&doc, &out).len(), 1);

        let (_, out) = filtered(r#"<feGaussianBlur stdDeviation="-2"/>"#);
        assert_eq!(out.skipped, vec!["filter".to_string()]);
    }

    /// A chain that neither moves nor softens is a recolour and is refused, even with
    /// both primitives present; two controls prove chains are still read.
    ///
    /// flip: the guard back to `!seen_offset && !seen_blur` → fails at "a chain that neither moves nor softens is not a shadow".
    #[test]
    fn a_chain_that_neither_moves_nor_softens_is_still_refused() {
        let chain = |dx: &str, dev: &str| {
            filtered(&format!(
                r#"<feColorMatrix in="SourceGraphic" type="matrix"
                                  values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0" result="a"/>
                   <feOffset in="a" dx="{dx}" dy="0" result="o"/>
                   <feGaussianBlur in="o" stdDeviation="{dev}" result="b"/>
                   <feFlood flood-color="black" result="fl"/>
                   <feComposite in="fl" in2="b" operator="in" result="c"/>"#
            ))
        };
        let (doc, out) = chain("0", "0");
        assert!(
            stack_of(&doc, &out).is_empty(),
            "a chain that neither moves nor softens is not a shadow"
        );
        assert_eq!(
            out.skipped,
            vec!["filter".to_string()],
            "and refusing it is a loss worth reporting"
        );

        // Control one: it moves.
        let (doc, out) = chain("4", "0");
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let (s, inner) = shadow(&stack_of(&doc, &out), 0);
        assert!(!inner);
        assert_eq!(s.offset, kurbo::Vec2::new(4.0, 0.0));
        assert_eq!(s.blur, 0.0, "hard-edged, read out of a hand-written chain");

        // Control two: it softens without moving.
        let (doc, out) = chain("0", "2");
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let (s, _) = shadow(&stack_of(&doc, &out), 0);
        assert_eq!(s.offset, kurbo::Vec2::ZERO);
        assert_eq!(s.blur, 4.0);
    }

    /// `flood-color="currentColor"` resolves against the referencing element's `color`
    /// (asserted green, not black).
    ///
    /// flip: the `Some("currentColor")` arm removed → fails at the `skipped` assertion with `["filter"]`.
    #[test]
    fn a_current_color_flood_resolves_against_the_referencing_elements_color() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <defs><filter id="f">
                   <feDropShadow dx="1" dy="1" stdDeviation="2" flood-color="currentColor"/>
                 </filter></defs>
                 <rect width="50" height="20" color="#00ff00" filter="url(#f)"/>
               </svg>"##,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let fx = stack_of(&doc, &out);
        assert_eq!(fx.len(), 1, "a currentColor flood is readable");
        let (s, _) = shadow(&fx, 0);
        assert_eq!(
            s.color.components[..3],
            [0.0, 1.0, 0.0],
            "the element's own colour, not black: {:?}",
            s.color
        );
    }

    /// The hand-written idiom blurs before it offsets (`SourceAlpha` marks the chain)
    /// and reads the same as offset-first.
    ///
    /// flip: requiring the offset first (`&& seen_offset` on the blur arm) → fails at `out.skipped`.
    #[test]
    fn the_hand_written_idiom_blurs_before_it_offsets_and_reads_the_same() {
        let (doc, out) = filtered(
            r#"<feGaussianBlur in="SourceAlpha" stdDeviation="3" result="b"/>
               <feOffset in="b" dx="2" dy="2" result="o"/>
               <feFlood flood-color="black" flood-opacity="0.4" result="fl"/>
               <feComposite in="fl" in2="o" operator="in" result="c"/>
               <feMerge><feMergeNode in="c"/><feMergeNode in="SourceGraphic"/></feMerge>"#,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let (s, inner) = shadow(&stack_of(&doc, &out), 0);
        assert!(!inner);
        assert_eq!(s.offset, kurbo::Vec2::new(2.0, 2.0));
        assert!((s.blur - 6.0).abs() < 1e-9, "{}", s.blur);
    }

    /// A bare `<feOffset/>` (the writer's "unchanged") is an empty stack, not a skip.
    #[test]
    fn a_filter_that_resolves_to_unchanged_is_an_empty_stack_rather_than_a_skip() {
        let (doc, out) = filtered(r#"<feOffset/>"#);
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert!(
            stack_of(&doc, &out).is_empty(),
            "{:?}",
            stack_of(&doc, &out)
        );
    }

    /// Document order is stack order; the trailing `feMerge` lists them differently
    /// and reading it would swap the pair.
    #[test]
    fn a_stack_of_several_effects_comes_back_in_the_order_it_was_written() {
        let (doc, out) = filtered(
            r#"<feGaussianBlur in="SourceGraphic" stdDeviation="1" result="g"/>
               <feColorMatrix in="g" type="matrix"
                              values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0" result="a0"/>
               <feComponentTransfer in="a0" result="t0">
                 <feFuncA type="table" tableValues="1 0"/>
               </feComponentTransfer>
               <feOffset in="t0" dx="1" dy="1" result="o0"/>
               <feFlood flood-color="black" result="f0"/>
               <feComposite in="f0" in2="o0" operator="in" result="c0"/>
               <feComposite in="c0" in2="a0" operator="in" result="k0"/>
               <feColorMatrix in="g" type="matrix"
                              values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0" result="a1"/>
               <feOffset in="a1" dx="9" dy="9" result="o1"/>
               <feFlood flood-color="black" result="f1"/>
               <feComposite in="f1" in2="o1" operator="in" result="c1"/>
               <feMerge>
                 <feMergeNode in="c1"/><feMergeNode in="g"/><feMergeNode in="k0"/>
               </feMerge>"#,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let fx = stack_of(&doc, &out);
        assert_eq!(fx.len(), 3, "{fx:?}");
        assert!(
            matches!(fx[0].kind, EffectKind::LayerBlur { .. }),
            "stage one first: {:?}",
            fx[0].kind
        );
        assert!(shadow(&fx, 1).1, "then the inner shadow, as written");
        assert!(!shadow(&fx, 2).1, "then the drop shadow, as written");
        assert_eq!(shadow(&fx, 2).0.offset, kurbo::Vec2::new(9.0, 9.0));
    }

    /// A flood's colour and opacity are read through `style=""` too.
    #[test]
    fn a_floods_colour_is_read_through_style_like_every_other_presentation_attribute() {
        let (doc, out) = filtered(
            r#"<feDropShadow dx="1" dy="1" stdDeviation="1"
                             style="flood-color:red;flood-opacity:0.3"/>"#,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let (s, _) = shadow(&stack_of(&doc, &out), 0);
        assert_eq!(s.color.components[0], 1.0, "{:?}", s.color);
        assert!((s.color.components[3] - 0.3).abs() < 1e-6, "{:?}", s.color);
    }

    /// `display="none"` becomes a hidden layer rather than nothing.
    #[test]
    fn a_hidden_element_arrives_hidden_rather_than_missing() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <rect width="10" height="10" display="none"/>
               </svg>"#,
        );
        let id = shapes(&doc, &out)[0];
        assert!(!doc.get(id).unwrap().visible());
        assert_eq!(out.shapes, 1);
    }

    /// Bad XML and a non-`<svg>` root are the only refusals.
    #[test]
    fn only_bad_xml_and_a_non_svg_root_are_refused() {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        assert_eq!(
            import("<svg><rect", &mut ids, root, 0, None).unwrap_err(),
            SvgError::NotXml
        );
        assert_eq!(
            import("<html><body/></html>", &mut ids, root, 0, None).unwrap_err(),
            SvgError::NotSvg
        );
        assert!(
            import(
                r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
                &mut ids,
                root,
                0,
                None
            )
            .is_ok()
        );
    }

    /// `looks_like_svg` refuses ordinary text and admits markup behind an XML declaration.
    #[test]
    fn the_clipboard_gate_admits_declarations_and_refuses_prose() {
        assert!(looks_like_svg(r#"<svg viewBox="0 0 1 1"/>"#));
        assert!(looks_like_svg(
            "<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
        ));
        assert!(looks_like_svg("   \n <svg/>"));
        assert!(!looks_like_svg("an <svg> in a sentence"));
        assert!(!looks_like_svg("<html><svg/></html>"));
        assert!(!looks_like_svg(""));
    }

    /// A doctype or generator comment before the root element still passes the gate.
    ///
    /// flip: dropping the `<!--` strip loop in `looks_like_svg` → fails at the second assertion.
    #[test]
    fn a_doctype_or_a_generator_comment_is_still_a_drawing() {
        assert!(looks_like_svg(
            r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">
<svg/>"#
        ));
        assert!(looks_like_svg(
            "<!-- Generator: Adobe Illustrator -->\n<svg/>"
        ));
        assert!(looks_like_svg(
            "<?xml version=\"1.0\"?>\n<!-- Generator -->\n<!-- and a licence -->\n<svg/>"
        ));
        assert!(!looks_like_svg("<!--<svg/> and never closed"));
        assert!(!looks_like_svg("<!-- <svg/> -->"));
    }

    /// A `<polygon>` imports as a `Path`: `NodeKind::Polygon` is a regular n-gon, and an arbitrary point list is not.
    #[test]
    fn a_point_list_is_a_path_and_not_a_regular_polygon() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <polygon points="0,0 10,0 3,7"/>
               </svg>"#,
        );
        let id = shapes(&doc, &out)[0];
        assert!(matches!(doc.get(id).unwrap().kind(), NodeKind::Path { .. }));
        assert_eq!(world_box(&doc, id), kurbo::Rect::new(0.0, 0.0, 10.0, 7.0));
    }

    /// The root `<svg>`'s own `fill="none"` is inherited by children that state no paint.
    #[test]
    fn the_root_element_is_styled_too() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg" fill="none">
                 <rect width="10" height="10" stroke="black"/>
               </svg>"#,
        );
        let id = shapes(&doc, &out)[0];
        assert!(
            doc.get(id).unwrap().paint().fills.is_empty(),
            "the root's fill=\"none\" inherits; SVG's initial black must not win"
        );
        assert_eq!(doc.get(id).unwrap().paint().strokes.len(), 1);
    }

    /// A skipped `filter` attribute is reported like a skipped element, and its shape still arrives.
    #[test]
    fn a_skipped_attribute_is_reported_like_a_skipped_element() {
        let (_doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <g filter="url(#f)"><rect width="10" height="10"/></g>
                </svg>"##,
        );
        assert_eq!(out.skipped, vec!["filter"]);
        assert_eq!(out.shapes, 1, "and the shape under it still arrived");
    }

    /// A `clip-path` becomes a wrapper group holding `[mask, content]`; a mask clips the siblings above it.
    #[test]
    fn a_clip_path_becomes_a_mask_node_beneath_its_content() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <rect width="100" height="100" clip-path="url(#c)"/>
                  <defs><clipPath id="c"><rect width="40" height="40"/></clipPath></defs>
                </svg>"##,
        );
        assert!(
            out.skipped.is_empty(),
            "read, not skipped: {:?}",
            out.skipped
        );
        let kids = doc.get(out.root).unwrap().children().to_vec();
        assert_eq!(kids.len(), 1, "one wrapper");
        let wrapper = kids[0];
        let inside = doc.get(wrapper).unwrap().children().to_vec();
        assert_eq!(inside.len(), 2, "the clip and the thing it clips");

        let clip = doc.get(inside[0]).unwrap();
        assert!(clip.mask(), "the *first* child is the mask");
        assert_eq!(clip.mask_mode(), MaskMode::Shape, "a clip is hard-edged");
        assert_eq!(clip.name(), "Clip");
        assert!(
            !doc.get(inside[1]).unwrap().mask(),
            "and the content above it is not"
        );
        // The clip's own geometry is the 40-square, not the 100-square it clips.
        assert_eq!(
            world_box(&doc, inside[0]),
            kurbo::Rect::new(0.0, 0.0, 40.0, 40.0)
        );
    }

    /// A duplicate `<clipPath>` id resolves to the first definition (the same map serves `<mask>`).
    ///
    /// The sizes are 40 and 90, not 100, so an ignored clip cannot read as the first one.
    /// flip: `Gradients::collect`'s `clips.entry(id).or_insert(el)` → `clips.insert(id, el)` → fails at the box assertion.
    #[test]
    fn a_duplicate_clip_path_id_resolves_to_the_first_definition() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <rect width="100" height="100" clip-path="url(#c)"/>
                  <defs>
                    <clipPath id="c"><rect width="40" height="40"/></clipPath>
                    <clipPath id="c"><rect width="90" height="90"/></clipPath>
                  </defs>
                </svg>"##,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let wrapper = doc.get(out.root).unwrap().children()[0];
        let inside = doc.get(wrapper).unwrap().children().to_vec();
        assert!(
            doc.get(inside[0]).unwrap().mask(),
            "the first child is the mask"
        );
        assert_eq!(
            world_box(&doc, inside[0]),
            kurbo::Rect::new(0.0, 0.0, 40.0, 40.0),
            "the second <clipPath id=\"c\"> won"
        );
    }

    /// The clip moves with the element it clips: a shape's clip carries the transform itself,
    /// a container's inherits the group's, and both routes land on the same boxes.
    ///
    /// flip: moving the transform from the clip to the wrapper → fails at the content assertion (it is applied twice).
    #[test]
    fn the_clip_carries_the_referencing_elements_transform() {
        let clip_and_content = |markup: &str| {
            let (doc, out) = imported(markup);
            let holder = doc.get(out.root).unwrap().children()[0];
            let inside = doc.get(holder).unwrap().children().to_vec();
            (
                world_box(&doc, inside[0]),
                world_box(&doc, inside[1]),
                doc.get(inside[0]).unwrap().mask(),
            )
        };
        // A container, which clips itself: the clip carries no transform of its own.
        let hoisted = clip_and_content(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <g transform="translate(50,0)" clip-path="url(#c)">
                    <rect width="100" height="100"/>
                  </g>
                  <defs><clipPath id="c"><rect width="40" height="40"/></clipPath></defs>
                </svg>"##,
        );
        // A shape, which takes a wrapper: the clip carries the transform itself.
        let wrapped = clip_and_content(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <rect transform="translate(50,0)" width="100" height="100" clip-path="url(#c)"/>
                  <defs><clipPath id="c"><rect width="40" height="40"/></clipPath></defs>
                </svg>"##,
        );
        for (what, (clip, content, is_mask)) in [("hoisted", hoisted), ("wrapped", wrapped)] {
            assert!(is_mask, "{what}: the first child is the mask");
            assert_eq!(
                clip,
                kurbo::Rect::new(50.0, 0.0, 90.0, 40.0),
                "{what}: the clip moved with what it clips"
            );
            assert_eq!(
                content,
                kurbo::Rect::new(50.0, 0.0, 150.0, 100.0),
                "{what}: and so did the content, which is the point of them agreeing"
            );
        }
    }

    /// A clipped container clips itself; only a clipped shape gets a wrapper (an outer `<g clip-path>` would otherwise add a level to every paste).
    #[test]
    fn a_clipped_container_clips_itself_rather_than_gaining_a_wrapper() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <g clip-path="url(#c)">
                    <rect width="100" height="100"/>
                    <rect width="10" height="10"/>
                  </g>
                  <defs><clipPath id="c"><rect width="40" height="40"/></clipPath></defs>
                </svg>"##,
        );
        let kids = doc.get(out.root).unwrap().children().to_vec();
        assert_eq!(kids.len(), 1, "the group, and no wrapper over it");
        let inside = doc.get(kids[0]).unwrap().children().to_vec();
        assert_eq!(inside.len(), 3, "the clip and the group's own two shapes");
        assert!(doc.get(inside[0]).unwrap().mask(), "the clip is first");
        assert!(
            !doc.get(inside[1]).unwrap().mask() && !doc.get(inside[2]).unwrap().mask(),
            "and both shapes are above it, so both are clipped"
        );
        assert_eq!(
            doc.get(inside[0]).unwrap().transform(),
            Affine::IDENTITY,
            "and it carries no transform, being in the group's space already"
        );
    }

    /// An element with both a clip and a mask nests them (wrapper plus hoist); two masks in one parent would be two runs and one would do nothing.
    ///
    /// flip: dropping the "other attribute still pending" half of the hoist test → fails at the first count.
    #[test]
    fn both_a_clip_and_a_mask_nest_rather_than_sharing_a_parent() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <g clip-path="url(#c)" mask="url(#m)">
                    <rect width="100" height="100"/>
                  </g>
                  <defs>
                    <clipPath id="c"><rect width="40" height="40"/></clipPath>
                    <mask id="m"><rect width="20" height="20" fill="white"/></mask>
                  </defs>
                </svg>"##,
        );
        let wrapper = doc.get(out.root).unwrap().children()[0];
        let outer = doc.get(wrapper).unwrap().children().to_vec();
        assert_eq!(outer.len(), 2, "the clip and the group it clips");
        assert!(doc.get(outer[0]).unwrap().mask());
        assert_eq!(doc.get(outer[0]).unwrap().mask_mode(), MaskMode::Shape);

        let inner = doc.get(outer[1]).unwrap().children().to_vec();
        assert_eq!(inner.len(), 2, "the mask and the rect, one level down");
        assert!(doc.get(inner[0]).unwrap().mask());
        assert_eq!(
            doc.get(inner[0]).unwrap().mask_mode(),
            MaskMode::Alpha,
            "the second one hoisted into the group rather than joining the first"
        );
    }

    /// A `<mask>` imports as an alpha mask (SVG's is luminance); opaque white agrees exactly, anything else is reported as approximated.
    #[test]
    fn a_mask_is_an_alpha_mask_and_says_when_that_is_a_guess() {
        let white = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <rect width="100" height="100" mask="url(#m)"/>
                  <defs><mask id="m"><rect width="40" height="40" fill="white"/></mask></defs>
                </svg>"##,
        );
        let wrapper = white.0.get(white.1.root).unwrap().children()[0];
        let clip = white.0.get(wrapper).unwrap().children()[0];
        assert!(white.0.get(clip).unwrap().mask());
        assert_eq!(white.0.get(clip).unwrap().mask_mode(), MaskMode::Alpha);
        assert!(
            white.1.approximated.is_empty(),
            "opaque white reads the same either way: {:?}",
            white.1.approximated
        );

        let grey = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <rect width="100" height="100" mask="url(#m)"/>
                  <defs><mask id="m"><rect width="40" height="40" fill="#808080"/></mask></defs>
                </svg>"##,
        );
        assert_eq!(
            grey.1.approximated,
            vec!["mask"],
            "a grey mask means something different under luminance"
        );
        assert!(
            grey.1.skipped.is_empty(),
            "and it is not *skipped* — it is there"
        );
    }

    /// A clip reference that resolves to nothing draws unclipped and is reported (SVG would render nothing).
    #[test]
    fn an_unresolvable_clip_reference_still_draws_and_is_reported() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <rect width="100" height="100" clip-path="url(#missing)"/>
                </svg>"##,
        );
        assert_eq!(out.skipped, vec!["clip-path"]);
        assert_eq!(out.shapes, 1);
        assert!(
            !doc.get(doc.get(out.root).unwrap().children()[0])
                .unwrap()
                .mask(),
            "no wrapper, no mask — just the rect"
        );
    }

    /// Inside a `<clipPath>` the fill rule comes from `clip-rule`, not `fill-rule`.
    #[test]
    fn a_clip_reads_clip_rule_and_not_fill_rule() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <rect width="100" height="100" clip-path="url(#c)"/>
                  <defs><clipPath id="c">
                    <path clip-rule="evenodd" fill-rule="nonzero"
                          d="M0 0h100v100h-100z M30 30h40v40h-40z"/>
                  </clipPath></defs>
                </svg>"##,
        );
        let wrapper = doc.get(out.root).unwrap().children()[0];
        let clip_group = doc.get(wrapper).unwrap().children()[0];
        let shape = doc.get(clip_group).unwrap().children()[0];
        assert_eq!(
            doc.get(shape).unwrap().fill_rule(),
            FillRule::EvenOdd,
            "clip-rule wins inside a clip; fill-rule is noise there"
        );
    }

    /// A Figma-style export in miniature (root `fill="none"`, outer clipped `<g>`, compound icon, stroked line) imports with empty `skipped` and `approximated`.
    #[test]
    fn a_real_export_arrives_whole() {
        let (doc, out) = imported(
            r##"<svg width="120" height="80" viewBox="0 0 120 80" fill="none" xmlns="http://www.w3.org/2000/svg">
                  <g clip-path="url(#clip0_1_2)">
                    <rect width="120" height="80" rx="8" fill="#F4F4F5"/>
                    <path fill-rule="evenodd" clip-rule="evenodd"
                          d="M75 25H105V55H75V25ZM82 32H98V48H82V32Z" fill="#18181B"/>
                    <line x1="20" y1="70" x2="100" y2="70" stroke="#A1A1AA" stroke-width="2"/>
                  </g>
                  <defs>
                    <clipPath id="clip0_1_2"><rect width="120" height="80" rx="8" fill="white"/></clipPath>
                  </defs>
                </svg>"##,
        );
        assert!(out.skipped.is_empty(), "nothing skipped: {:?}", out.skipped);
        assert!(
            out.approximated.is_empty(),
            "and nothing approximated: {:?}",
            out.approximated
        );
        assert_eq!(out.size, Some(Size::new(120.0, 80.0)));
        assert_eq!(out.shapes, 4, "three shapes and the clip's own rect");

        // One group, holding its clip and its three shapes — no wrapper on top.
        let g = doc.get(out.root).unwrap().children()[0];
        let inside = doc.get(g).unwrap().children().to_vec();
        assert_eq!(inside.len(), 4);
        assert!(doc.get(inside[0]).unwrap().mask());
        assert_eq!(
            doc.get(inside[2]).unwrap().fill_rule(),
            FillRule::EvenOdd,
            "the compound icon keeps its holes"
        );
        assert_eq!(
            doc.get(inside[3]).unwrap().paint().strokes.len(),
            1,
            "and the line is stroked, not filled — the root's fill=\"none\" inherited"
        );
        assert!(doc.get(inside[3]).unwrap().paint().fills.is_empty());
    }

    /// `<use>` is a deep copy placed by `x`/`y`; the definition in `<defs>` is not drawn where it stands.
    #[test]
    fn a_use_copies_the_element_it_references_to_its_own_place() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <defs><rect id="tick" width="10" height="10"/></defs>
                  <use href="#tick" x="0" y="0"/>
                  <use href="#tick" x="50" y="20"/>
                </svg>"##,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert_eq!(out.shapes, 2, "two copies, and the original is not drawn");
        let kids = doc.get(out.root).unwrap().children().to_vec();
        assert_eq!(kids.len(), 2, "one group per instance");
        let copy = |i: usize| {
            let inner = doc.get(kids[i]).unwrap().children()[0];
            world_box(&doc, inner)
        };
        assert_eq!(copy(0), kurbo::Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(
            copy(1),
            kurbo::Rect::new(50.0, 20.0, 60.0, 30.0),
            "the second instance is placed by its own x and y"
        );
    }

    /// A `<symbol>` contributes its contents flat (no extra level per instance) and is never drawn in place.
    #[test]
    fn a_symbol_is_instantiated_rather_than_drawn() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <symbol id="s"><rect width="10" height="10"/><rect width="4" height="4"/></symbol>
                  <use href="#s" x="30"/>
                </svg>"##,
        );
        assert_eq!(out.shapes, 2, "the symbol's two rects, once");
        let kids = doc.get(out.root).unwrap().children().to_vec();
        assert_eq!(kids.len(), 1, "the symbol itself drew nothing");
        assert_eq!(
            doc.get(kids[0]).unwrap().children().len(),
            2,
            "and its contents came through flat rather than nested again"
        );
    }

    /// A `<use>` referencing its own ancestor terminates and is reported; the partial expansion is not pinned.
    #[test]
    fn a_recursive_use_stops_rather_than_recursing_forever() {
        let (_doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <g id="loop"><rect width="10" height="10"/><use href="#loop"/></g>
                </svg>"##,
        );
        assert!(
            out.skipped.iter().any(|s| s == "use (recursive)"),
            "it stopped, and said why: {:?}",
            out.skipped
        );
    }

    /// An unresolvable `<use>` is reported rather than silently empty.
    #[test]
    fn an_unresolvable_use_is_reported() {
        let (_doc, out) =
            imported(r##"<svg xmlns="http://www.w3.org/2000/svg"><use href="#gone"/></svg>"##);
        assert_eq!(out.skipped, vec!["use (unresolved)"]);
        assert_eq!(out.shapes, 0);
    }

    /// An Illustrator-style `<style>` class rule beats a presentation attribute and loses to an inline style.
    #[test]
    fn a_class_rule_beats_a_presentation_attribute_and_loses_to_inline() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>.cls-1{fill:#00ff00;}.cls-2{fill:#0000ff;}</style>
                  <rect class="cls-1" width="10" height="10" fill="#ff0000"/>
                  <rect class="cls-2" width="10" height="10" style="fill:#ffffff"/>
                </svg>"##,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let ids = shapes(&doc, &out);
        let fill = |id: NodeId| match doc.get(id).unwrap().paint().fills[0].brush.literal() {
            Some(Brush::Solid(c)) => c.to_rgba8().to_u8_array(),
            other => panic!("a solid fill, got {other:?}"),
        };
        assert_eq!(
            fill(ids[0]),
            [0, 255, 0, 255],
            "the class wins over the fill attribute"
        );
        assert_eq!(
            fill(ids[1]),
            [255, 255, 255, 255],
            "and an inline style wins over the class"
        );
    }

    /// An XML comment inside `<style>` does not truncate the sheet; both reports stay empty
    /// because an unstyled shape would silently be black.
    ///
    /// The whitespace-then-comment row fails through a different branch than the leading comment.
    /// flip: reading the sheet with `el.text()` → fails at the first row's `.b` (black against green).
    #[test]
    fn a_comment_inside_a_style_block_does_not_truncate_the_sheet() {
        let case = |sheet: &str| {
            let (doc, out) = imported(&format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg">
                      <style>{sheet}</style>
                      <rect class="a" width="10" height="10"/>
                      <rect class="b" width="10" height="10"/>
                    </svg>"##
            ));
            assert!(out.skipped.is_empty(), "skipped: {:?}", out.skipped);
            assert!(
                out.approximated.is_empty(),
                "approximated: {:?}",
                out.approximated
            );
            let ids = shapes(&doc, &out);
            let fill = |id: NodeId| match doc.get(id).unwrap().paint().fills[0].brush.literal() {
                Some(Brush::Solid(c)) => c.to_rgba8().to_u8_array(),
                other => panic!("a solid fill, got {other:?}"),
            };
            (fill(ids[0]), fill(ids[1]))
        };
        const RED: [u8; 4] = [255, 0, 0, 255];
        const GREEN: [u8; 4] = [0, 255, 0, 255];
        const BLACK: [u8; 4] = [0, 0, 0, 255];

        assert_eq!(
            case(".a{fill:#ff0000}<!--c-->.b{fill:#00ff00}"),
            (RED, GREEN),
            "a comment between two rules must not end the sheet at it"
        );
        assert_eq!(
            case("<!--c-->.a{fill:#ff0000}").0,
            RED,
            "nor a leading one lose the whole sheet — this is the legacy \
             `<style><!-- … --></style>` idiom"
        );
        assert_eq!(
            case("\n<!--c-->\n.a{fill:#ff0000}").0,
            RED,
            "nor whitespace and then a comment, which fails through the other \
             branch: `text()` finds the newline and returns it"
        );

        // Controls: these always worked; CDATA shows the cause is comments, not interleaving.
        assert_eq!(case(".a{fill:#ff0000}<!--c-->").0, RED, "control: trailing");
        assert_eq!(
            case(".a{fill:#ff0000}.b{fill:#00ff00}"),
            (RED, GREEN),
            "control: no comment at all"
        );
        assert_eq!(
            case(".a{fill:#ff0000}<![CDATA[.b{fill:#00ff00}]]>"),
            (RED, GREEN),
            "control: roxmltree merges CDATA with text, so this was never broken"
        );
        // The initial fill is black.
        assert_eq!(
            case(".a{fill:#ff0000}").1,
            BLACK,
            "an unmatched class is black"
        );
    }

    /// An id beats a class beats a type; equal specificity is settled by document order, later wins.
    #[test]
    fn specificity_and_document_order_decide_between_rules() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>
                    rect{fill:#ff0000}
                    .c{fill:#00ff00}
                    #only{fill:#0000ff}
                    .c{fill:#00ffff}
                  </style>
                  <rect id="only" class="c" width="10" height="10"/>
                  <rect class="c" width="10" height="10"/>
                  <rect width="10" height="10"/>
                </svg>"##,
        );
        let ids = shapes(&doc, &out);
        let fill = |id: NodeId| match doc.get(id).unwrap().paint().fills[0].brush.literal() {
            Some(Brush::Solid(c)) => c.to_rgba8().to_u8_array(),
            other => panic!("a solid fill, got {other:?}"),
        };
        assert_eq!(fill(ids[0]), [0, 0, 255, 255], "the id wins");
        assert_eq!(
            fill(ids[1]),
            [0, 255, 255, 255],
            "of two classes, the later one"
        );
        assert_eq!(
            fill(ids[2]),
            [255, 0, 0, 255],
            "and the type rule is the floor"
        );
    }

    /// Descendant and child combinators and compound selectors; the grandchild that `>` must not reach
    /// is what separates a combinator from a lookup, and the higher-specificity rule is written first
    /// so document order cannot explain the result.
    ///
    /// flip: `Combinator::Child` walking all ancestors, or `Sel::matches` skipping `chain` → fails at the grandchild assertion.
    #[test]
    fn a_selector_reads_combinators_and_compounds() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>
                    .outer .a{fill:#00ff00}
                    .a{fill:#0000ff}
                    .outer > .kid{fill:#00ffff}
                    rect.both{fill:#ff00ff}
                  </style>
                  <g class="outer">
                    <rect class="a" width="10" height="10"/>
                    <rect class="kid" width="10" height="10"/>
                    <g><rect class="kid" width="10" height="10"/></g>
                    <rect class="both" width="10" height="10"/>
                  </g>
                </svg>"##,
        );
        assert!(
            out.skipped.is_empty(),
            "every selector here is one this reads: {:?}",
            out.skipped
        );
        // `shapes` walks every descendant, so the two `<g>`s are in this list: the rects are 1, 2, 4 and 5.
        let ids = shapes(&doc, &out);
        let fill = |id| {
            let Some(Brush::Solid(c)) = doc.get(id).unwrap().paint().fills[0].brush.literal()
            else {
                panic!("a solid fill")
            };
            c.to_rgba8().to_u8_array()
        };

        assert_eq!(
            fill(ids[1]),
            [0, 255, 0, 255],
            "the descendant rule matched, and its two classes outrank the bare \
             `.a` written after it — specificity, not document order"
        );
        assert_eq!(
            fill(ids[2]),
            [0, 255, 255, 255],
            "a direct child matches the child combinator"
        );
        assert_eq!(
            fill(ids[4]),
            [0, 0, 0, 255],
            "and a grandchild does not — this is the assertion that separates \
             `>` from a descendant walk, and the only one that can. Black is \
             SVG's own default fill, which is what a shape no rule reached gets"
        );
        assert_eq!(
            fill(ids[5]),
            [255, 0, 255, 255],
            "a compound of a type and a class matches the element that is both"
        );
    }

    /// A long descendant selector over deep nesting returns; a recursive matcher would be exponential.
    ///
    /// There is no wall-clock assertion, only the suite's timeout. The leading compound is `nope` so the
    /// rule reaches the matcher (`declaration` checks the property and specificity first) and loses there.
    #[test]
    fn a_long_selector_over_deep_nesting_returns() {
        const LEVELS: usize = 60;
        let chain = "g ".repeat(10);
        let opens = "<g>".repeat(LEVELS);
        let closes = "</g>".repeat(LEVELS);
        let (doc, out) = imported(&format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>nope {chain}rect{{fill:#ff0000}}</style>
                  {opens}<rect width="10" height="10"/>{closes}
                </svg>"##
        ));
        let ids = shapes(&doc, &out);
        let rect = *ids.last().expect("the rect is the innermost shape");
        let Some(Brush::Solid(c)) = doc.get(rect).unwrap().paint().fills[0].brush.literal() else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [0, 0, 0, 255],
            "the rule is reached and loses on `nope`, which is what makes this \
             fixture exercise the matcher rather than the presence check"
        );
    }

    /// A stylesheet past `MAX_CSS_RULES` is reported, and the rules read before the cap still paint.
    ///
    /// flip: disabling the cap's branch → fails at the report assertion (the fill stays green).
    /// Raising `MAX_CSS_RULES` is not a flip: the fixture is defined in terms of it.
    #[test]
    fn a_stylesheet_past_the_rule_cap_is_reported_and_what_it_read_still_paints() {
        let mut sheet = String::from("rect{fill:#00ff00}");
        for i in 0..MAX_CSS_RULES + 10 {
            sheet.push_str(&format!(".pad{i}{{stroke-width:1}}"));
        }
        let (doc, out) = imported(&format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>{sheet}</style>
                  <rect width="10" height="10"/>
                </svg>"##
        ));
        assert!(
            out.skipped
                .contains(&"style (complex selector)".to_string()),
            "a sheet past the cap must be reported, not silently truncated: {:?}",
            out.skipped
        );
        let ids = shapes(&doc, &out);
        let Some(Brush::Solid(c)) = doc.get(ids[0]).unwrap().paint().fills[0].brush.literal()
        else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [0, 255, 0, 255],
            "and the rules read before the cap still apply — the first one is \
             written first for exactly this assertion"
        );
    }

    /// `.a > .b .c` matches through the second `.b` up the tree: greedy and backtracking walks agree
    /// except when a `>` sits left of a descendant.
    ///
    /// flip: the descendant arm taking the first matching ancestor without reconsidering → fails at the fill (black against green).
    #[test]
    fn a_descendant_chain_reconsiders_an_ancestor_that_fails_further_left() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>.a > .b .c{fill:#00ff00}</style>
                  <g class="a">
                    <g class="b">
                      <g>
                        <g class="b">
                          <rect class="c" width="10" height="10"/>
                        </g>
                      </g>
                    </g>
                  </g>
                </svg>"##,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let ids = shapes(&doc, &out);
        let rect = *ids.last().expect("the rect is the innermost shape");
        let Some(Brush::Solid(c)) = doc.get(rect).unwrap().paint().fills[0].brush.literal() else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [0, 255, 0, 255],
            "the nearest `.b` has no `.a` for a parent, so the match has to keep \
             looking further up — this is the only shape where greedy and \
             backtracking disagree"
        );
    }

    /// A selector list keeps its readable members when another is valid CSS this importer cannot read,
    /// as a browser does; the unreadable member is reported.
    ///
    /// A list with a syntactically invalid member is applied the same way, which a browser would drop.
    /// flip: dropping the whole rule when any member is unreadable → fails at the fill (black).
    #[test]
    fn a_selector_list_keeps_its_readable_members_as_a_browser_does() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>.a, rect:hover{fill:#ff0000}</style>
                  <rect class="a" width="10" height="10"/>
                </svg>"##,
        );
        let rect = shapes(&doc, &out)[0];
        let Some(Brush::Solid(c)) = doc.get(rect).unwrap().paint().fills[0].brush.literal() else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [255, 0, 0, 255],
            "`.a` still applies, as it does in a browser — `rect:hover` is valid \
             CSS this importer merely cannot read"
        );
        assert_eq!(
            out.skipped,
            vec!["style (complex selector)"],
            "and the member it could not read is reported"
        );
    }

    /// `*` matches any element.
    ///
    /// flip: treating `*` as a literal tag name → fails at the fill assertion.
    #[test]
    fn a_universal_compound_matches_any_element() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>.a * rect{fill:#00ff00}</style>
                  <g class="a"><g><rect width="10" height="10"/></g></g>
                </svg>"##,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let ids = shapes(&doc, &out);
        let rect = *ids.last().expect("the rect is the innermost shape");
        let Some(Brush::Solid(c)) = doc.get(rect).unwrap().paint().fills[0].brush.literal() else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [0, 255, 0, 255],
            "`*` stands for the intermediate `<g>`, so the chain matches"
        );
    }

    /// `parse_selector` refuses unsupported and malformed selectors rather than half-matching them
    /// (`.a > > .b` repaired leniently would widen to `.a > .b`).
    #[test]
    fn a_selector_this_cannot_read_is_refused_rather_than_guessed() {
        for sel in [
            "rect + rect", // sibling order
            "rect ~ rect", // sibling order
            "rect:first",  // a pseudo-class needs state
            "rect[fill]",  // attribute matching
            ".a > > .b",   // malformed: would widen to `.a > .b`
            ".a >",        // a combinator with nothing on its right
            "> .a",        // and nothing on its left
            ".a##b",       // two ids on one compound match nothing
            ".a.",         // an empty class name
        ] {
            assert!(
                parse_selector(sel).is_none(),
                "{sel:?} must be refused and reported, not half-matched"
            );
        }
        for sel in [
            "*", "rect", ".a", "#b", "rect.a#b", ".a .b", ".a>.b", "* > .a",
        ] {
            assert!(
                parse_selector(sel).is_some(),
                "{sel:?} is one this reader does handle"
            );
        }
    }

    /// A selector this cannot read is ignored and reported once for the sheet; the readable rules still apply.
    #[test]
    fn a_selector_that_needs_a_cascade_is_reported_once() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>rect + rect{fill:#ff0000}.ok{fill:#00ff00}</style>
                  <rect class="ok" width="10" height="10"/>
                </svg>"##,
        );
        assert_eq!(out.skipped, vec!["style (complex selector)"]);
        let id = shapes(&doc, &out)[0];
        let Some(Brush::Solid(c)) = doc.get(id).unwrap().paint().fills[0].brush.literal() else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [0, 255, 0, 255],
            "and the rules it *could* read still applied"
        );
    }

    /// The text box sits above the file's baseline by about the ascent (a band, not Inter's exact metrics).
    ///
    /// flip: dropping the `- ascent` in `text_node` → fails at the band assertion (y0 = 100).
    #[test]
    fn text_is_placed_by_its_baseline_and_not_by_its_box() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="20" y="100" font-size="40">Hg</text>
               </svg>"#,
        );
        assert_eq!(out.shapes, 1);
        let id = shapes(&doc, &out)[0];
        let b = world_box(&doc, id);
        assert!(
            (b.x0 - 20.0).abs() < 2.0,
            "x is the left edge for the default anchor: {b:?}"
        );
        assert!(
            b.y0 < 100.0 && b.y0 > 100.0 - 40.0 * 1.2,
            "the box is above the baseline by about the ascent, not on it: {b:?}"
        );
        assert!(
            b.y1 > 100.0,
            "and the descender hangs below it, which is what a baseline means: {b:?}"
        );
    }

    /// `text-anchor` is folded into the box position, since the model stores no anchor.
    #[test]
    fn a_text_anchor_shifts_the_box_rather_than_being_stored() {
        let boxes = |anchor: &str| {
            let (doc, out) = imported(&format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg">
                     <text x="100" y="50" text-anchor="{anchor}">Hello</text>
                   </svg>"#
            ));
            world_box(&doc, shapes(&doc, &out)[0])
        };
        let start = boxes("start");
        let middle = boxes("middle");
        let end = boxes("end");
        assert!((start.x0 - 100.0).abs() < 1.0, "{start:?}");
        assert!(
            (end.x1 - 100.0).abs() < 1.0,
            "the end anchor ends at x: {end:?}"
        );
        assert!(
            (middle.center().x - 100.0).abs() < 1.0,
            "and the middle one is centred on it: {middle:?}"
        );
    }

    /// A `<text>`'s opacity and visibility reach every run it splits into, and a `<tspan>`'s own opacity
    /// nests multiplicatively (0.3 x 0.5).
    ///
    /// flip: `Some(text_el)` in `text_node` back to `None` → fails at "the `<text>`'s opacity reaches the second run".
    #[test]
    fn a_texts_own_chrome_reaches_every_run_it_splits_into() {
        let opacity = |doc: &Document, id: NodeId| doc.get(id).map(|n| n.opacity());
        let visible = |doc: &Document, id: NodeId| doc.get(id).map(|n| n.visible());

        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" opacity="0.3" display="none">A<tspan x="0" y="60">B</tspan></text>
               </svg>"#,
        );
        let ids = shapes(&doc, &out);
        assert_eq!(ids.len(), 2, "the fixture: two runs, two nodes");
        assert_eq!(opacity(&doc, ids[0]), Some(0.3), "the control: run one");
        assert_eq!(
            opacity(&doc, ids[1]),
            Some(0.3),
            "the `<text>`'s opacity reaches the second run"
        );
        assert_eq!(visible(&doc, ids[0]), Some(false), "the control: run one");
        assert_eq!(
            visible(&doc, ids[1]),
            Some(false),
            "and so does `display=\"none\"` — a hidden layer is hidden whole"
        );

        // The `<tspan>`'s own opacity nests inside the `<text>`'s rather than replacing it.
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" opacity="0.3">A<tspan x="0" y="60" opacity="0.5">B</tspan></text>
               </svg>"#,
        );
        let ids = shapes(&doc, &out);
        let got = opacity(&doc, ids[1]).expect("a second run");
        assert!(
            (got - 0.15).abs() < 1e-6,
            "0.3 × 0.5, the way SVG nests it: {got}"
        );
        assert_eq!(
            opacity(&doc, ids[0]),
            Some(0.3),
            "and the first run, which is the `<text>` itself, is not squared"
        );
    }

    /// A nested `<tspan>` carries its style like a flat one, at any depth, and a nested positioned one still breaks a run.
    ///
    /// flip: `text_pieces` emitting one piece per element with text from `descendants()` → fails at "a nested tspan carries its own style".
    #[test]
    fn a_nested_tspan_carries_its_style_like_a_flat_one() {
        let spans_of = |svg: &str| {
            let (doc, out) = imported(svg);
            let ids = shapes(&doc, &out);
            assert!(out.skipped.is_empty(), "{:?}", out.skipped);
            let NodeKind::Text { spans, content, .. } =
                doc.get(ids[0]).expect("a text node").kind()
            else {
                panic!("a text node");
            };
            (content.clone(), spans.clone(), ids.len())
        };

        let flat = r##"<svg xmlns="http://www.w3.org/2000/svg">
             <text x="0" y="20" fill="black">a<tspan fill="#ff0000" font-size="40"
                   font-weight="700">b</tspan></text>
           </svg>"##;
        let nested = r##"<svg xmlns="http://www.w3.org/2000/svg">
             <text x="0" y="20" fill="black">a<tspan><tspan fill="#ff0000"
                   font-size="40" font-weight="700">b</tspan></tspan></text>
           </svg>"##;

        let (flat_text, flat_spans, _) = spans_of(flat);
        assert_eq!(flat_text, "ab", "the control's content");
        assert!(
            !flat_spans.is_empty(),
            "the control: the flat spelling has always carried its style"
        );

        let (text, spans, _) = spans_of(nested);
        assert_eq!(text, "ab", "the content was never the half that was lost");
        assert_eq!(
            spans, flat_spans,
            "a nested tspan carries its own style, and the same style the flat \
             spelling carries"
        );

        // A nested positioned tspan places a second run.
        let (_, _, made) = spans_of(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20">a<tspan><tspan x="0" y="60">b</tspan></tspan></text>
               </svg>"#,
        );
        assert_eq!(made, 2, "a nested positioned tspan is still a break");
    }

    /// Text after a nested `<tspan>` takes the enclosing tspan's style, since `text_pieces` yields one piece per text node.
    ///
    /// flip: `text_pieces` yielding one piece per element → fails at "y reads the inner one's" (12 against 40);
    /// the tail assertion stays green under that flip and only pins the ordering.
    #[test]
    fn text_after_a_nested_tspan_reads_the_outer_ones_style() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" font-size="8">
                   <tspan font-size="12">x<tspan font-size="40">y</tspan>z</tspan>
                 </text>
               </svg>"#,
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let ids = shapes(&doc, &out);
        let node = doc.get(ids[0]).expect("a text node");
        let NodeKind::Text { content, spans, .. } = node.kind() else {
            panic!("a text node");
        };
        assert_eq!(content.trim(), "xyz", "the fixture: {content:?}");

        // Sizes are read through `Spans::resolve`, the shaper's own door.
        let NodeKind::Text { style, .. } = node.kind() else {
            panic!("a text node");
        };
        let i = content
            .find(|c: char| !c.is_whitespace())
            .expect("some ink");
        let size_at = |b: usize| spans.resolve(style, b).font_size;
        assert_eq!(size_at(i), 12.0, "x reads the outer tspan's size");
        assert_eq!(
            size_at(i + 1),
            40.0,
            "y reads the inner one's — the control"
        );
        assert_eq!(
            size_at(i + 2),
            12.0,
            "and the tail reads the outer tspan's size, not the inner one's"
        );
    }

    /// A `<tspan>` with its own position starts a new layer; one without joins the text
    /// (this app writes a tspan per style run, Illustrator a tspan per line).
    #[test]
    fn a_positioned_tspan_starts_a_new_layer_and_a_bare_one_does_not() {
        let (doc, split) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20"><tspan x="0" y="20">one</tspan><tspan x="0" y="60">two</tspan></text>
               </svg>"#,
        );
        assert_eq!(split.shapes, 2, "two positioned runs, two layers");
        let ids = shapes(&doc, &split);
        assert!(
            world_box(&doc, ids[1]).y0 - world_box(&doc, ids[0]).y0 > 30.0,
            "and the second is where its own y put it"
        );

        let (doc2, joined) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20"><tspan>one </tspan><tspan>two</tspan></text>
               </svg>"#,
        );
        assert_eq!(joined.shapes, 1, "two style runs, one layer");
        let NodeKind::Text { content, .. } = doc2.get(shapes(&doc2, &joined)[0]).unwrap().kind()
        else {
            panic!("a Text node")
        };
        assert_eq!(content, "one two");
    }

    /// An unpositioned `<tspan fill>` becomes a colour span over exactly its own bytes: the range is clamped
    /// to trimmed content, and a tspan repeating the node's colour mints nothing (compared through `ink`).
    ///
    /// flip: dropping the `min` in `text_node` → fails at the trimmed-space case;
    /// reverting `TextStyle::set` to a raw assignment → fails only at the quantized-alpha assertion.
    #[test]
    fn a_tspans_own_colour_becomes_a_span_over_the_bytes_it_contributed() {
        use crate::typography::CharAttr;

        let spans_of = |doc: &crate::document::Document, id| {
            let NodeKind::Text { spans, style, .. } = doc.get(id).unwrap().kind() else {
                panic!("a Text node")
            };
            (spans.as_slice().to_vec(), style.color)
        };

        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" fill="#0000ff">one <tspan fill="#ff0000">two</tspan> three</text>
               </svg>"##,
        );
        assert_eq!(out.shapes, 1, "one layer, not one per run");
        let id = shapes(&doc, &out)[0];
        let NodeKind::Text { content, .. } = doc.get(id).unwrap().kind() else {
            panic!("a Text node")
        };
        assert_eq!(content, "one two three");
        let (list, default) = spans_of(&doc, id);
        assert_eq!(
            default.map(|c| c.to_rgba8().to_u8_array()),
            Some([0, 0, 255, 255]),
            "the node's own ink is the <text>'s"
        );
        assert_eq!(list.len(), 1, "one run states a colour: {list:?}");
        assert_eq!((list[0].start, list[0].end), (4, 7), "exactly \"two\"");
        let CharAttr::Color(Some(c)) = &list[0].attr else {
            panic!("a colour span, got {:?}", list[0].attr)
        };
        assert_eq!(c.to_rgba8().to_u8_array(), [255, 0, 0, 255]);

        // A trailing space inside the coloured run: the span must not run past the trimmed content.
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" fill="#0000ff">one <tspan fill="#ff0000">two </tspan></text>
               </svg>"##,
        );
        let id = shapes(&doc, &out)[0];
        let NodeKind::Text { content, .. } = doc.get(id).unwrap().kind() else {
            panic!("a Text node")
        };
        assert_eq!(content, "one two", "the trailing space is trimmed");
        let (list, _) = spans_of(&doc, id);
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!(
            (list[0].start, list[0].end),
            (4, 7),
            "clamped to the trimmed content, not the 8 it was collected at"
        );

        // The same colour twice is the default said again, not a span.
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" fill="#0000ff" fill-opacity="0.5">one <tspan fill="#0000ff" fill-opacity="0.5">two</tspan></text>
               </svg>"##,
        );
        let (list, default) = spans_of(&doc, shapes(&doc, &out)[0]);
        assert!(
            list.is_empty(),
            "a run repeating the node's ink mints nothing: {list:?}"
        );
        assert!(
            out.approximated.is_empty(),
            "and nothing is reported lost: {:?}",
            out.approximated
        );
        // The stored alpha is quantized to 1/255, the only value the opacity control can reproduce.
        let alpha = default.expect("an ink").components[3];
        assert!(
            (f64::from(alpha) - 128.0 / 255.0).abs() < 1e-6,
            "fill-opacity quantized to 1/255, got {alpha}"
        );
    }

    /// A `<tspan>`'s size, weight, italic and family become spans, each compared independently.
    /// The family goes through `Builder::pick_family` like the node's own; a run naming only missing families gets a report and no span.
    ///
    /// flip: dropping the `Weight` push → fails the first case; `pick_family` taking the list's head regardless of availability → fails the third.
    #[test]
    fn a_tspans_font_scope_becomes_spans_rather_than_being_dropped_in_silence() {
        use crate::typography::CharAttrKind;

        let kinds = |doc: &crate::document::Document, id| -> Vec<(usize, usize, CharAttrKind)> {
            let NodeKind::Text { spans, .. } = doc.get(id).unwrap().kind() else {
                panic!("a Text node")
            };
            spans
                .as_slice()
                .iter()
                .map(|s| (s.start, s.end, s.attr.kind()))
                .collect()
        };

        // A run stating a size and a weight gives two spans over the same bytes.
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" font-size="20">one <tspan font-size="32" font-weight="700">two</tspan></text>
               </svg>"#,
        );
        let id = shapes(&doc, &out)[0];
        let NodeKind::Text { content, style, .. } = doc.get(id).unwrap().kind() else {
            panic!("a Text node")
        };
        assert_eq!(content, "one two", "one node, both runs");
        // 20, not the default 16, so an importer ignoring the `<text>`'s `font-size` fails here.
        assert_eq!(style.font_size, 20.0, "the node keeps the <text>'s size");
        let mut list = kinds(&doc, id);
        list.sort_by_key(|e| format!("{:?}", e.2));
        assert_eq!(
            list,
            [(4, 7, CharAttrKind::Size), (4, 7, CharAttrKind::Weight),],
            "exactly the two the run stated, over exactly its bytes"
        );

        // Italic alone, so the kinds are independent.
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20">one <tspan font-style="italic">two</tspan></text>
               </svg>"#,
        );
        assert_eq!(
            kinds(&doc, shapes(&doc, &out)[0]),
            [(4, 7, CharAttrKind::Italic)],
            "italic alone"
        );

        // A family nothing here has: reported, and no span.
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20">one <tspan font-family="Nonexistent Face XYZ">two</tspan></text>
               </svg>"#,
        );
        assert!(
            out.approximated
                .contains(&"text (font substituted)".to_string()),
            "the substitution is reported: {:?}",
            out.approximated
        );
        assert!(
            kinds(&doc, shapes(&doc, &out)[0]).is_empty(),
            "and nothing was written that would re-point the run"
        );
    }

    /// A span covering every byte is hoisted onto the node (this app's own writer puts font properties on a single-run tspan);
    /// a run covering part of the content keeps its span.
    ///
    /// flip: dropping the `clear` → fails at `spans.is_empty()`; dropping the whole block → fails at the node's weight reading 400.
    #[test]
    fn a_span_over_the_whole_content_becomes_the_nodes_own_default() {
        let node = |markup: &str| {
            let (doc, out) = imported(markup);
            let id = shapes(&doc, &out)[0];
            let NodeKind::Text { style, spans, .. } = doc.get(id).unwrap().kind() else {
                panic!("a Text node")
            };
            (style.weight, spans.as_slice().to_vec())
        };

        // Full coverage.
        let (weight, spans) = node(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" font-weight="400"><tspan font-weight="700">ab</tspan></text>
               </svg>"#,
        );
        assert_eq!(weight, 700, "the run's weight became the node's");
        assert!(spans.is_empty(), "and left nothing behind: {spans:?}");

        // Two agreeing runs: `normalize` merges them into one span before the hoist.
        let (weight, spans) = node(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" font-weight="400"><tspan font-weight="700">a</tspan><tspan font-weight="700">b</tspan></text>
               </svg>"#,
        );
        assert_eq!(weight, 700, "two runs agreeing are still one default");
        assert!(spans.is_empty(), "{spans:?}");

        // Partial coverage is the control.
        let (weight, spans) = node(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" font-weight="400">a<tspan font-weight="700">b</tspan></text>
               </svg>"#,
        );
        assert_eq!(weight, 400, "the node keeps its own weight");
        assert_eq!(spans.len(), 1, "and the run keeps its span: {spans:?}");
        assert_eq!((spans[0].start, spans[0].end), (1, 2));
    }

    /// A size below `MIN_FONT_SIZE` clamps on both the node and the span, so no override span is minted.
    /// Both sizes are out of range, which makes them equal only after clamping.
    ///
    /// flip: building `ts` with a raw `font_size: style.font_size` literal → fails at the node-size assertion (0.1 against 1.0).
    #[test]
    fn a_size_outside_the_models_range_clamps_on_both_sides_and_mints_no_span() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" font-size="0.1">one <tspan font-size="0.2">two</tspan></text>
               </svg>"#,
        );
        let NodeKind::Text { style, spans, .. } = doc.get(shapes(&doc, &out)[0]).unwrap().kind()
        else {
            panic!("a Text node")
        };
        assert_eq!(
            style.font_size,
            crate::typography::MIN_FONT_SIZE,
            "the node's own size is clamped rather than stored as written"
        );
        assert!(
            spans.is_empty(),
            "and the run clamps to the same number, so there is nothing to override: {:?}",
            spans.as_slice()
        );
    }

    /// A per-run gradient (which `CharAttr::Color` cannot hold) is reported; one the run merely inherits is not.
    #[test]
    fn a_per_run_gradient_is_reported_where_a_per_run_colour_is_carried() {
        let defs = r##"<defs><linearGradient id="g"><stop offset="0" stop-color="#f00"/>
                       <stop offset="1" stop-color="#00f"/></linearGradient></defs>"##;
        let (_, out) = imported(&format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg">{defs}
                 <text x="0" y="20" fill="#0000ff">one <tspan fill="url(#g)">two</tspan></text>
               </svg>"##
        ));
        assert_eq!(
            out.approximated,
            vec!["text (per-run style)"],
            "a gradient on a run has nowhere to go"
        );

        // A gradient on the `<text>` itself is a node-level loss, reported once and not per run.
        let (doc, out) = imported(&format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg">{defs}
                 <text x="0" y="20" fill="url(#g)">one <tspan>two</tspan></text>
               </svg>"##
        ));
        assert_eq!(
            out.approximated,
            vec!["text (gradient fill)"],
            "the node cannot hold a paint server, and the run is not blamed for it"
        );
        let NodeKind::Text { spans, .. } = doc.get(shapes(&doc, &out)[0]).unwrap().kind() else {
            panic!("a Text node")
        };
        assert!(spans.is_empty(), "and mints no span either");

        // A positioned `<tspan>` with a gradient is its own node; the `positioned` shortcut in `text` stops a second report.
        let (doc, out) = imported(&format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg">{defs}
                 <text x="0" y="20" fill="#0000ff">one <tspan x="50" y="40" fill="url(#g)">two</tspan></text>
               </svg>"##
        ));
        assert_eq!(
            shapes(&doc, &out).len(),
            2,
            "a positioned run is its own layer"
        );
        assert_eq!(
            out.approximated,
            vec!["text (gradient fill)"],
            "one gradient, one report — not one per direction it was noticed from"
        );

        // The `style=""` spelling must be read through `property`, not `has_attribute` (Inkscape writes it there).
        let (_, out) = imported(&format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg">{defs}
                 <text x="0" y="20" fill="#0000ff">one <tspan style="fill:url(#g)">two</tspan></text>
               </svg>"##
        ));
        assert_eq!(
            out.approximated,
            vec!["text (per-run style)"],
            "a gradient spelled in style= is the same loss"
        );
    }

    /// XML whitespace in a pretty-printed `<text>` collapses to single spaces.
    #[test]
    fn whitespace_in_the_markup_is_collapsed_rather_than_typed() {
        let (doc, out) = imported(
            "<svg xmlns=\"http://www.w3.org/2000/svg\">\n  <text x=\"0\" y=\"0\">\n    Hello\n    world\n  </text>\n</svg>",
        );
        let NodeKind::Text { content, .. } = doc.get(shapes(&doc, &out)[0]).unwrap().kind() else {
            panic!("a Text node")
        };
        assert_eq!(content, "Hello world");
    }

    /// Only XML's four whitespace characters collapse: no-break, figure and ideographic spaces survive,
    /// including a leading one that `text_node`'s trims must not eat (`lead` re-bases span ranges on them).
    ///
    /// flip: `c.is_whitespace()` back in `push_collapsed` → fails at the NBSP assertion;
    /// the wide `trim()`/`trim_start()` back in `text_node` → fails at the ideographic one.
    #[test]
    fn a_no_break_space_is_not_collapsed_into_an_ordinary_one() {
        let text_of = |content: &str| -> String {
            let (doc, out) = imported(&format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\"><text x=\"0\" y=\"0\">{content}</text></svg>"
            ));
            let NodeKind::Text { content, .. } = doc.get(shapes(&doc, &out)[0]).unwrap().kind()
            else {
                panic!("a Text node")
            };
            content.clone()
        };

        assert_eq!(text_of("10\u{00a0}km"), "10\u{00a0}km", "no-break space");
        assert_eq!(text_of("1\u{2007}5"), "1\u{2007}5", "figure space");
        assert_eq!(text_of("1\u{202f}2"), "1\u{202f}2", "narrow no-break space");
        assert_eq!(
            text_of("\u{3000}a"),
            "\u{3000}a",
            "a leading ideographic space is content, and `trim` used to eat it"
        );

        // Controls: the rule is no narrower than before, and still collapses real XML whitespace.
        assert_eq!(
            text_of("a\u{feff}b"),
            "a\u{feff}b",
            "control: a zero-width no-break space has no White_Space property and \
             was preserved before this change too"
        );
        assert_eq!(
            text_of("\n    Hello\n    world\n  "),
            "Hello world",
            "control: the four XML separators still collapse, or the fix is \
             'stop collapsing'"
        );
    }

    /// A `font-family` list picks the first available family; an unavailable one is substituted and reported.
    #[test]
    fn an_unavailable_family_is_substituted_and_said_out_loud() {
        let (_doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="0" font-family="Nonesuch Grotesk, Fantasy">hi</text>
               </svg>"#,
        );
        assert_eq!(out.approximated, vec!["text (font substituted)"]);
        assert_eq!(out.shapes, 1, "and the text still arrived");
    }

    /// Font size reaches a run three ways: stated on the `<text>`, on a positioned `<tspan>`, or inherited from a `<g>`.
    ///
    /// flip: `ts.font_size` back to `property(el, "font-size", …)` → fails only at the inherited box (the third `<text>`).
    #[test]
    fn font_properties_inherit_and_reach_a_positioned_tspan() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <g font-family="Nonesuch, Arial" font-size="90">
                   <text x="90" y="184" font-size="180">TEST</text>
                   <text x="80" y="174"><tspan x="80" y="174" font-size="180">TEST</tspan></text>
                   <text x="0" y="400">TEST</text>
                 </g>
               </svg>"#,
        );
        assert_eq!(out.shapes, 3);
        let ids = shapes(&doc, &out);
        // `ids[0]` is the group.
        let (a, b, c) = (
            world_box(&doc, ids[1]),
            world_box(&doc, ids[2]),
            world_box(&doc, ids[3]),
        );
        assert!(a.height() > 100.0, "the size on the <text>: {a:?}");
        assert!(
            b.height() > 100.0,
            "the size on the <tspan> — the test card's second TEST: {b:?}"
        );
        assert!(
            (a.width() - b.width()).abs() < 1.0,
            "the same word at the same size is the same width: {a:?} {b:?}"
        );
        // The offset is the difference in their anchors (a drop shadow).
        assert!(
            (a.x0 - b.x0 - 10.0).abs() < 0.5 && (a.y0 - b.y0 - 10.0).abs() < 0.5,
            "ten units apart on both axes: {a:?} {b:?}"
        );
        assert!(
            (c.width() - a.width() * 0.5).abs() < 2.0,
            "and the third inherited 90 from the <g>, half of the other two: {c:?}"
        );
        assert_eq!(
            out.approximated,
            vec!["text (font substituted)"],
            "and the family on the <g> was seen — it is reported, so it was read"
        );
    }

    /// A `<!DOCTYPE>` is not a parse error; refusing it would fail the whole file as `SvgError::NotXml`.
    ///
    /// flip: `allow_dtd: false` → `Err(NotXml)`.
    #[test]
    fn a_doctype_is_not_a_parse_error() {
        let (doc, out) = imported(concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" ",
            "\"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\n",
            "<svg width=\"20\" height=\"20\" xmlns=\"http://www.w3.org/2000/svg\">",
            "<rect width=\"10\" height=\"10\"/></svg>"
        ));
        assert_eq!(out.shapes, 1, "the doctype is a preamble, not a refusal");
        assert_eq!(
            world_box(&doc, shapes(&doc, &out)[0]),
            kurbo::Rect::new(0.0, 0.0, 10.0, 10.0)
        );
    }

    /// A bomb of nested `<g>`s is reported and the built tree stays within `crate::io::MAX_TREE_DEPTH`;
    /// 30 levels is the control and comes through whole. Depth is counted iteratively because `shapes()` recurses.
    ///
    /// flip: raising `MAX_SVG_NESTING` far above the fixture → the harness dies with `STATUS_STACK_OVERFLOW`, not an assertion.
    #[test]
    fn a_deeply_nested_svg_is_reported_rather_than_overflowing_the_stack() {
        fn bomb(levels: usize) -> String {
            format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\">{}\
                 <rect width=\"10\" height=\"10\"/>{}</svg>",
                "<g>".repeat(levels),
                "</g>".repeat(levels)
            )
        }

        /// The deepest node under `from`, counted without recursing.
        fn max_depth(doc: &Document, from: NodeId) -> usize {
            let mut deepest = 0;
            let mut stack = vec![(from, 0usize)];
            while let Some((id, d)) = stack.pop() {
                deepest = deepest.max(d);
                for c in doc
                    .get(id)
                    .map(|n| n.children().to_vec())
                    .unwrap_or_default()
                {
                    stack.push((c, d + 1));
                }
            }
            deepest
        }

        // The control: an ordinary depth comes through whole and says nothing.
        let (doc, out) = imported(&bomb(30));
        assert_eq!(out.shapes, 1, "30 levels is an ordinary export");
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert_eq!(max_depth(&doc, out.root), 31, "the group chain is kept");

        // 90 sits between the two bounds, the only band where the builder's own skip is visible
        // (deeper is refused whole by `MAX_XML_DEPTH`).
        const { assert!(MAX_SVG_NESTING < 90 && 90 < MAX_XML_DEPTH) };
        let (doc, out) = imported(&bomb(90));
        assert!(
            out.skipped.iter().any(|s| s == "deeply nested elements"),
            "the user is told what was dropped: {:?}",
            out.skipped
        );
        assert_eq!(
            out.shapes, 0,
            "the rect is below the bound, so it is not there"
        );
        assert!(
            max_depth(&doc, out.root) <= crate::io::MAX_TREE_DEPTH,
            "whatever it built has to load again: {}",
            max_depth(&doc, out.root)
        );
    }

    /// `<use>`'s depth cap bounds nesting, not branching: the node budget stops a doubling `<use>` bomb.
    /// The budget counts nodes (groups included), so `Import::shapes` after a refusal is not the budget;
    /// the 1,024-shape control must come through whole.
    ///
    /// flip: `MAX_SVG_NODES` = `usize::MAX` → fails at the `skipped` assertion; the control stays green.
    #[test]
    fn a_branching_use_bomb_stops_and_says_so_while_real_artwork_does_not() {
        fn bomb(l: usize) -> String {
            let defs: String = (0..l)
                .map(|i| {
                    format!(
                        "<g id='g{i}'><use href='#g{}'/><use href='#g{}'/></g>",
                        i + 1,
                        i + 1
                    )
                })
                .collect();
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20'>\
                 <defs>{defs}<g id='g{l}'><rect width='2' height='2'/></g></defs>\
                 <use href='#g0'/></svg>"
            )
        }
        // The control: an ordinary amount of instancing comes through whole.
        let (_, out) = imported(&bomb(10));
        assert_eq!(out.shapes, 1024, "1,024 shapes is not a bomb");
        assert!(
            out.skipped.is_empty(),
            "and nothing was refused: {:?}",
            out.skipped
        );

        // The bomb: refused, reported, and bounded.
        let (_, out) = imported(&bomb(14));
        assert!(
            out.skipped.iter().any(|s| s == "too many elements"),
            "the user is told the paste was cut short: {:?}",
            out.skipped
        );
        assert!(
            out.shapes < 16_384,
            "and it stopped before the 2^14 the file asks for: {}",
            out.shapes
        );
    }

    /// A gradient stop offset of `nan` still saves and reloads (`f64::clamp` propagates NaN, and serde writes it as `null`).
    /// The infinities are controls: they clamp to 1.0 and 0.0 rather than being dropped.
    ///
    /// flip: removing `.filter(|f| !f.is_nan())` from `stop_of` → fails at the `io::load` assertion.
    #[test]
    fn a_gradient_stop_offset_of_nan_does_not_cost_the_document() {
        let svg = |off: &str| {
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>\
                 <defs><linearGradient id='g'>\
                 <stop offset='{off}' stop-color='#f00'/>\
                 <stop offset='1' stop-color='#00f'/>\
                 </linearGradient></defs>\
                 <rect width='9' height='9' fill='url(#g)'/></svg>"
            )
        };

        let (doc, _) = imported(&svg("nan"));
        let saved = crate::io::save(&doc).expect("a document with a gradient saves");
        crate::io::load(&saved).expect("and opens again — this is the whole finding");

        // Controls: if either moves, the NaN filter went too wide.
        for (off, want) in [("1e999", 1.0f32), ("-1e999", 0.0)] {
            let (doc, _) = imported(&svg(off));
            let saved = crate::io::save(&doc).expect("saves");
            crate::io::load(&saved).expect("and reloads");
            let got = first_stop_offset(&doc);
            assert_eq!(
                got, want,
                "offset={off} must clamp to {want}, not be dropped"
            );
        }
    }

    /// A descending stop list is clamped up on import (SVG's rule: `[0, 0.8, 0.2, 1]` means `[0, 0.8, 0.8, 1]`), not sorted.
    /// This keeps the model right; the render boundary's `color::brush_to_backend` applies the same rule separately.
    ///
    /// flip: removing the `make_stops_monotonic` call in `Gradients::collect` → fails at stop 2 (`0.2` against `0.8`).
    #[test]
    fn a_descending_stop_list_is_clamped_up_on_import() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>\
             <defs><linearGradient id='g'>\
             <stop offset='0' stop-color='#f00'/>\
             <stop offset='0.8' stop-color='#0f0'/>\
             <stop offset='0.2' stop-color='#00f'/>\
             <stop offset='1' stop-color='#fff'/>\
             </linearGradient></defs>\
             <rect width='9' height='9' fill='url(#g)'/></svg>";
        let (doc, _) = imported(svg);
        assert_eq!(
            all_stop_offsets(&doc),
            vec![0.0, 0.8, 0.8, 1.0],
            "the third stop collapses onto the second — SVG's clamp, not a sort"
        );

        // The control: an ascending list is untouched.
        let ok = svg.replace("offset='0.2'", "offset='0.9'");
        assert_eq!(all_stop_offsets(&imported(&ok).0), vec![0.0, 0.8, 0.9, 1.0]);
    }

    /// `!important` is stripped before `parse_paint`, and an unreadable `fill` leaves the inherited paint standing and is reported;
    /// `fill='none'` is the control and still means unpainted (told apart by its text).
    ///
    /// flip: removing the `!important` strip → fails at the first assertion (black against red);
    /// returning the `fill` arm to a plain assignment → fails at "the green survived".
    #[test]
    fn a_paint_this_reader_cannot_parse_leaves_the_inherited_one_and_says_so() {
        let red = |doc: &Document, id: NodeId| match &doc.get(id).unwrap().paint().fills[..] {
            [f] => match f.brush.literal() {
                Some(Brush::Solid(c)) => c.to_rgba8().to_u8_array(),
                other => panic!("expected a solid, got {other:?}"),
            },
            other => panic!("expected exactly one fill, got {}", other.len()),
        };

        // `!important` is stripped, so the value behind it parses like any other.
        let (doc, out) = imported(
            "<svg xmlns='http://www.w3.org/2000/svg'><style>.a{fill:#ff0000 !important;}</style>\
             <rect class='a' width='10' height='10'/></svg>",
        );
        let ids = shapes(&doc, &out);
        assert_eq!(red(&doc, ids[0])[0..3], [0xFF, 0x00, 0x00]);
        assert!(
            out.skipped.is_empty(),
            "nothing was lost, so nothing may be reported: {:?}",
            out.skipped
        );

        // A value this reader cannot read leaves the inherited paint standing (CSS drops an invalid declaration).
        let (doc, out) = imported(
            "<svg xmlns='http://www.w3.org/2000/svg'><g fill='#00ff00'>\
             <rect style='fill:var(--brand)' width='10' height='10'/></g></svg>",
        );
        let ids = shapes(&doc, &out);
        let inherited = ids
            .iter()
            .copied()
            .find(|id| !doc.get(*id).unwrap().paint().fills.is_empty())
            .expect("the green survived");
        assert_eq!(red(&doc, inherited)[0..3], [0x00, 0xFF, 0x00]);
        assert!(
            out.skipped.iter().any(|s| s.contains("unreadable")),
            "an unreadable paint has to reach a report list: {:?}",
            out.skipped
        );

        // The control: `none` also parses to `None`, and means unpainted.
        let (doc, out) = imported(
            "<svg xmlns='http://www.w3.org/2000/svg'><g fill='#00ff00'>\
             <rect fill='none' width='10' height='10'/></g></svg>",
        );
        let ids = shapes(&doc, &out);
        assert!(
            ids.iter()
                .all(|id| doc.get(*id).unwrap().paint().fills.is_empty()),
            "`fill='none'` still means no fill, not an unreadable one"
        );
        assert!(
            out.skipped.is_empty(),
            "and it is not a loss to report: {:?}",
            out.skipped
        );
    }

    /// A gradient's stop list is resampled to `MAX_GRADIENT_STOPS` (each shape holds its own copy, so memory is quadratic)
    /// and reported once as approximated; a gradient under the cap is kept exactly with nothing reported.
    /// It asserts the stop count held, not a wall clock.
    ///
    /// flip: deleting the resample block in `Gradients::collect` → fails at the stop-count assertion (600 against 256).
    #[test]
    fn a_gradients_stop_list_is_bounded_and_says_so() {
        let file = |n: usize| {
            let stops: String = (0..n)
                .map(|i| {
                    format!(
                        "<stop offset='{}' stop-color='#{:02x}0000'/>",
                        i as f64 / (n - 1) as f64,
                        (i * 255 / n) as u8
                    )
                })
                .collect();
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>\
                 <defs><linearGradient id='g'>{stops}</linearGradient></defs>\
                 <rect width='9' height='9' fill='url(#g)'/></svg>"
            )
        };

        let (doc, out) = imported(&file(600));
        assert_eq!(
            all_stop_offsets(&doc).len(),
            MAX_GRADIENT_STOPS,
            "600 stops must come down to the cap — this is the quantity that is \
             quadratic in the number of shapes"
        );
        assert!(
            out.approximated.iter().any(|s| s.contains("gradient")),
            "and the file must say so: {:?} / {:?}",
            out.approximated,
            out.skipped
        );
        // Resampling keeps the ends, which truncating would not.
        let offs = all_stop_offsets(&doc);
        assert!(
            offs[0] == 0.0 && (offs[MAX_GRADIENT_STOPS - 1] - 1.0).abs() < 1e-6,
            "the resample must span the whole ramp, got {}..{}",
            offs[0],
            offs[MAX_GRADIENT_STOPS - 1]
        );

        // The control: a gradient under the cap is kept exactly.
        let (doc, out) = imported(&file(64));
        assert_eq!(all_stop_offsets(&doc).len(), 64);
        assert!(
            out.approximated.is_empty() && out.skipped.is_empty(),
            "an ordinary gradient must cost nothing to say: {:?} / {:?}",
            out.approximated,
            out.skipped
        );
    }

    /// Every stop offset of the first gradient fill in `doc`, in order.
    fn all_stop_offsets(doc: &Document) -> Vec<f32> {
        fn walk(doc: &Document, id: NodeId, out: &mut Vec<Vec<f32>>) {
            if let Some(n) = doc.get(id) {
                for f in &n.paint().fills {
                    if let Some(crate::Brush::Gradient(g)) = f.brush.literal() {
                        out.push(g.gradient.stops.iter().map(|s| s.offset).collect());
                    }
                }
                for c in n.children() {
                    walk(doc, *c, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(doc, doc.root(), &mut out);
        out.into_iter()
            .next()
            .expect("a gradient fill was imported")
    }

    /// The offset of the first stop of the first gradient fill in `doc`.
    fn first_stop_offset(doc: &Document) -> f32 {
        fn walk(doc: &Document, id: NodeId, out: &mut Vec<f32>) {
            if let Some(n) = doc.get(id) {
                for f in &n.paint().fills {
                    if let Some(crate::Brush::Gradient(g)) = f.brush.literal()
                        && let Some(s) = g.gradient.stops.first()
                    {
                        out.push(s.offset);
                    }
                }
                for c in n.children() {
                    walk(doc, *c, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(doc, doc.root(), &mut out);
        *out.first().expect("a gradient fill was imported")
    }

    /// A `clip-path` referencing its own `<clipPath>` (directly or mutually) terminates through `MAX_SVG_NESTING` and is reported.
    ///
    /// flip: raising `MAX_SVG_NESTING` far above the fixture → the test binary dies with `STATUS_STACK_OVERFLOW`, with no assertion message.
    #[test]
    fn a_clip_path_cycle_stops_rather_than_recursing_forever() {
        // Self-referential: `#a` holds a rect clipped by `#a`.
        let (_, out) = imported(
            "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>\
             <defs><clipPath id='a'><rect clip-path='url(#a)' width='9' height='9'/></clipPath></defs>\
             <rect clip-path='url(#a)' width='9' height='9' fill='#000'/></svg>",
        );
        assert!(
            out.skipped.iter().any(|s| s == "deeply nested elements"),
            "the cycle is reported, not silently truncated: {:?}",
            out.skipped
        );

        // Mutual: `#a` holds a shape clipped by `#b`, `#b` one clipped by `#a`.
        let (_, out) = imported(
            "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>\
             <defs>\
             <clipPath id='a'><rect clip-path='url(#b)' width='9' height='9'/></clipPath>\
             <clipPath id='b'><rect clip-path='url(#a)' width='9' height='9'/></clipPath>\
             </defs>\
             <rect clip-path='url(#a)' width='9' height='9' fill='#000'/></svg>",
        );
        assert!(
            out.skipped.iter().any(|s| s == "deeply nested elements"),
            "the mutual form too: {:?}",
            out.skipped
        );
    }

    /// `depth` nested `<g>` in an `<svg>`, with one rect at the bottom.
    fn nested(depth: usize) -> String {
        let mut s = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg">"#);
        s.push_str(&"<g>".repeat(depth));
        s.push_str(r#"<rect width="1" height="1"/>"#);
        s.push_str(&"</g>".repeat(depth));
        s.push_str("</svg>");
        s
    }

    /// Runs `f` on a thread with the 1 MiB stack the shipped binary's main thread has, where `paste_svg` runs.
    fn on_the_main_threads_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(f)
            .expect("spawn")
            .join()
            .expect("the import returned rather than overflowing")
    }

    /// Nesting the parser cannot survive (2,000 `<g>`) is refused with `SvgError::TooDeep` before `roxmltree` sees it.
    ///
    /// flip: removing the `nests_deeper_than` refusal → the test binary dies with `STATUS_STACK_OVERFLOW`.
    #[test]
    fn markup_nested_past_the_parsers_depth_is_refused_before_the_parser() {
        let answer = on_the_main_threads_stack(|| {
            let mut ids = IdSource::new(1);
            let root = ids.mint();
            import(&nested(2_000), &mut ids, root, 0, None).map(|_| ())
        });
        assert_eq!(answer, Err(SvgError::TooDeep));
    }

    /// At `MAX_XML_DEPTH` the document still imports on the main thread's stack, with what lies past `MAX_SVG_NESTING` skipped and reported.
    ///
    /// flip: `MAX_XML_DEPTH` lowered below the fixture → fails at "at the bound it imports" (`Err(TooDeep)`).
    #[test]
    fn nesting_at_the_bound_still_imports_on_the_main_threads_stack() {
        let skipped = on_the_main_threads_stack(|| {
            let mut ids = IdSource::new(1);
            let root = ids.mint();
            import(&nested(MAX_XML_DEPTH - 1), &mut ids, root, 0, None).map(|out| out.skipped)
        });
        let skipped = skipped.expect("at the bound it imports");
        assert!(
            !skipped.is_empty(),
            "and the builder's own depth bound still reports what it cut"
        );
    }

    /// The depth scan steps over comments, CDATA, PIs, quoted attribute values and self-closed tags,
    /// measures depth not size (20,000 siblings), and counts an entity holding markup ten times.
    ///
    /// flip: dropping the `ENTITY_DEPTH * entity_depth` term → fails at "an entity holding markup counts ten times".
    #[test]
    fn the_depth_scan_counts_tags_and_nothing_else() {
        let deep = |s: &str| nests_deeper_than(s, 8);
        let noise = r#"<svg><!-- <g><g><g><g><g><g><g><g><g> --><![CDATA[<g><g><g><g><g><g><g><g><g>]]><?pi <g><g><g><g><g><g><g><g><g> ?><rect title="<g><g><g><g><g><g><g><g><g>" data-x='>'/><g/><g/><g/></svg>"#;
        assert!(
            !deep(noise),
            "comments, CDATA, PIs, attribute values and self-closing tags do not nest"
        );
        assert!(deep(&nested(9)), "nine real levels inside an svg do");
        assert!(!deep(&nested(6)), "and six do not");
        let siblings = format!("<svg>{}</svg>", "<g></g>".repeat(20_000));
        assert!(
            !nests_deeper_than(&siblings, 8),
            "control: twenty thousand siblings are shallow"
        );
        let entity = r#"<!DOCTYPE svg [<!ENTITY e "<g><g><g></g></g></g>">]><svg>&e;</svg>"#;
        assert!(
            nests_deeper_than(entity, 8),
            "an entity holding markup counts ten times — three levels read as thirty"
        );
        assert!(
            !nests_deeper_than(r#"<!DOCTYPE svg [<!ENTITY e "plain">]><svg>&e;</svg>"#, 8),
            "an entity holding only text adds nothing"
        );
    }
}
