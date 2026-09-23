//! SVG import — markup in, a [`Transaction`] out (§15 D394, and D259 for why the
//! paste path is where it lives; §7.1).
//!
//! **The inverse of `ondin_export::svg`, and deliberately not beside it.** That
//! writer lives in `ondin-export` because it keeps company with the PNG and JPEG
//! writers, which need a renderer; reading is model construction and needs nothing
//! but the model, so it belongs here with [`crate::build`] and is testable without a
//! graphics context. ⚠️ **The cost of that split is that the two mappings can drift**,
//! and nothing the compiler does will say so — what binds them is the round-trip
//! test in `ondin-export/tests/svg_roundtrip.rs`, which is the reason to keep it
//! green rather than to relax it.
//!
//! **The entry point is a paste**, not a file dialog: *"pasting a copied SVG"* is
//! what was asked for, and §15 D259 had already named the paste path as where this
//! would land. `import` therefore takes a string and a parent, mints its own ids and
//! hands back one transaction — the same shape every builder in [`crate::build`]
//! has, so the caller commits it exactly as it commits a group or a boolean.
//!
//! ## What it costs, which is almost nothing
//!
//! Three grammars have to be read and **two of them are already in core's own
//! crates**: `kurbo::BezPath::from_svg` is the whole `d` grammar including arcs, and
//! `peniko::color::parse_color` is the whole CSS colour grammar including the named
//! colours. What is ours is the XML (`roxmltree`, one added dependency) and the
//! `transform` list, which is [`parse_transform`] below and is sixty lines.
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
//! ⚠️ **The gradient half of that list is longer than it looks, and it was written
//! short.** "Both gradient kinds in both `gradientUnits`" was true of the *elements*
//! and false of every real file: a gradient inherits its stops and its position
//! through `xlink:href`, which is how Inkscape writes all of them, and it carries a
//! `gradientTransform` that composes inside the units mapping. Reading the first two
//! and not the third pair is not a partial success — it is a drawing with no
//! gradients in it at all, which is what pasting `AJ_Digital_Camera.svg` produced.
//! See [`gradient_chain`], [`Paint::mapping`] and [`transform_gradient`], all three of
//! which carry a warning about the way the *obvious* implementation is wrong.
//!
//! ## What it does not read, and says so
//!
//! Two lists, and the difference between them is what a reader can act on.
//! **[`Import::skipped`]** is *missing*: a `<filter>` holding a primitive this has no
//! kind for — an arbitrary `feColorMatrix` among them, which is why this app's own
//! image **adjustments** do not come back (see [`FilterRead`]) — `<foreignObject>`,
//! which is a **decided non-goal** rather than a gap (§15 D813): it is HTML inside
//! an SVG, nothing in this model draws HTML, and there is therefore no version of
//! it to build. It sat on `roadmap.md` as open work for weeks only because nobody
//! had said the words; the ruling is that it is reported like any other unread
//! element and that is the whole answer —
//! an `<image>` whose href points at a file
//! this paste never had, a reference that resolves to nothing, and a CSS selector
//! needing **sibling order, a pseudo-class or an attribute** — combinators
//! themselves are read since §15 D806. **[`Import::approximated`]** is
//! *there and slightly wrong*: an SVG `<mask>` whose luminance and alpha readings
//! differ, `objectBoundingBox` clip units, a stretched image, a substituted font, a
//! `<tspan>` whose own **gradient or stroke** was dropped — its *colour* and its
//! whole **font scope** (size, weight, italic, family) are carried now, as character
//! spans — and an `feGaussianBlur` with
//! two unequal deviations, averaged. Nothing is lost silently — a paste that quietly
//! loses half a drawing is worse than one that refuses.
//!
//! ⚠️ **A radial gradient squashed into an ellipse left that list on 2026-09-03 and
//! is worth a line for the shape of the mistake** (§15 D412). It was reported as
//! *drawn round*, on the reasoning that `peniko::RadialGradientPosition` is two
//! circles — true of that type and irrelevant, since an ellipse *is* a circle under
//! a squash and both renderers had been taking a brush transform for image framing
//! all along. The model carries one now ([`crate::image::GradientBrush`]), so this
//! is exact. **An entry on either list is a claim about the model, and a model can
//! grow** — which is a reason to re-derive an old loss rather than to trust it.
//!
//! ⚠️ **Which is why the *spelling* a property is read in belongs on this list too.**
//! `filter` was read with `has_attribute`, and Inkscape writes the whole presentation
//! block into `style=""` — so a drawing's blurs were dropped **and the paste did not
//! say so**, which is worse than either half alone. Anything consulted through
//! [`property`] is safe; anything reached with `attribute` or `has_attribute` is a
//! claim that the SVG in question never uses `style`.
//!
//! ⚠️ **The one *structural* loss is the paragraph, not the text.** The writer emits
//! one `<text>` per *line* at a baseline the canvas computed (§15 D81), and nothing
//! in the markup says those lines were one node — so a two-line text layer comes back
//! as two layers in the right places. The picture closes and the structure does not,
//! and rejoining them would mean inventing the line height that decides where every
//! line after the first sits.

use crate::id::{IdSource, NodeId};
use crate::image::Brush;
use crate::node::{Fill, FillRule, MaskMode, NodeKind, Stroke, StrokeAlign, StrokeSides};
use crate::op::{Operation, Transaction};
use crate::typography::CharAttr;
use kurbo::{Affine, BezPath, Point, Size};
use peniko::Color;

/// How deeply nested an SVG's elements may be before the walk gives up
/// (§15 D416).
///
/// **Deliberately far tighter than [`crate::io::MAX_TREE_DEPTH`], and the gap is
/// the point.** Two bounds have to hold and they are not the same number:
///
/// 1. **The walk's own stack.** `children → node → node_ignoring → children` is
///    recursive, and a generated `<g><g>…</g></g>` aborted a debug build at
///    depth 200 with `STATUS_STACK_OVERFLOW`.
/// 2. **What the *document* it builds may be.** A clipped or masked element
///    becomes a wrapper plus a clip node, so one level of SVG nesting can become
///    two or three levels of document. If the import could produce a tree past
///    `MAX_TREE_DEPTH`, the file would import cleanly, save cleanly and then
///    **refuse to reopen** — which is the loader's new check turned into a worse
///    bug than the one it fixes.
///
/// 64 clears both: it is inside the stack bound by a factor of three, and even
/// the pathological every-level-is-masked case lands well under 256. Against
/// that it is generous by any real measure — Figma and Illustrator exports nest
/// in the tens, and a subtree past it is *skipped and reported* rather than
/// dropped silently.
///
/// 🚨 **"Clears both" was true of the walk and not of the import** (§15 D851,
/// `[X7-L1-01]`). The XML parser runs first and recurses too, and nothing here
/// bounded it: 1,500 nested `<g>` aborted the release build inside `roxmltree`
/// before this constant was ever consulted. [`MAX_XML_DEPTH`] is that third bound.
const MAX_SVG_NESTING: usize = 64;

/// How many nodes one import may emit before the rest is skipped and reported
/// (§15 D446).
///
/// **[`MAX_SVG_NESTING`] bounds how *deep* the walk goes and nothing bounds how
/// *wide* it goes**, which is a different bomb with the same postage. A `<defs>`
/// of L groups each instantiating the next **twice**, plus one `<use>` at the
/// root, expands 2^L from a file that grows ~50 bytes per level: 843 bytes is
/// 16,384 shapes. `use_depth`'s cap of 16 admits **65,536**, and three `<use>`
/// per level instead of two is 3^16 ≈ 43 million — a `Vec<Operation>` that
/// exhausts memory, and an allocation failure is `handle_alloc_error`, an
/// **abort** nothing can catch.
///
/// `use_depth` cannot do this job: it counts *nesting*, so it is unchanged by how
/// many instances a level produces. Nor can `import`'s `nodes_limit`, which
/// bounds the XML nodes roxmltree *parses* — the 843-byte fixture is 47 elements.
///
/// **50,000 is chosen against real artwork, not against the bomb.** A traced
/// illustration or a detailed map runs to tens of thousands of shapes and must
/// still paste; nothing legitimate is on the other side of this line. Past it the
/// import stops and says so, which is this module's standing degradation.
///
/// ⚠️ **This bounds the importer and not the paste.** Measured after
/// `index_in`'s Θ(n²) was fixed: `import` is linear at ~1.2 µs/shape (32,000
/// shapes in 38 ms, from 1.81 s), but `Document::apply` of the same transaction
/// takes **62.6 s** — `op_create` resolves an unnamed node's name through
/// `child_names(parent, &[])`, which walks every sibling into a fresh
/// `FxHashSet<&str>` once per created child (it borrows rather than clones; the
/// cost is the walk and the set, not the strings), so applying n children to one
/// parent is Θ(n²) and outweighs the import by 1,600×. **That is the real cost of
/// pasting a large SVG and no constant here can bound it.**
const MAX_SVG_NODES: usize = 50_000;

/// How many CSS rules one stylesheet may declare before the rest is skipped and
/// reported (§15 D838).
///
/// **The third bomb shape in this module and the second one that is neither
/// deep nor wide but *long*.** [`MAX_SVG_NESTING`] bounds depth and
/// [`MAX_SVG_NODES`] bounds the emitted count; neither looks at the
/// `<style>` element, so a sheet of 50,000 rules against 1,000 shapes froze a
/// paste for **24.5 s** — `Css::declaration` walks every rule for every element
/// for every property it is asked about, which is linear in each and therefore
/// cubic in the file. §15 D806's ordering (the cheap tests before the tree
/// walk) is what keeps the constant small and is not a bound; re-flipping that
/// ordering was measured at 44.0 s against the shipped 24.5 s, so the shipped
/// order is already the faster one and the cap is the fix rather than a
/// reorder.
///
/// ⚠️ **2,048 is chosen with headroom over real exports rather than measured
/// against a corpus**, which is the honest statement of where it comes from.
/// Illustrator's `.st0….stN` convention emits one rule per distinct style and
/// runs to the low hundreds on a detailed drawing; Inkscape emits fewer. Past
/// this the sheet is reported through [`Css::complex`], which the import
/// already surfaces as *"style (complex selector)"* — the shapes still import,
/// they just take their presentation attributes instead.
const MAX_CSS_RULES: usize = 2_048;

/// What one `import` produced.
#[derive(Debug)]
pub struct Import {
    /// Every operation, in apply order: the root group first, then its subtree.
    pub tx: Transaction,
    /// The group everything was placed under, so the caller can select it.
    pub root: NodeId,
    /// The size the markup declares — its `viewBox`, else its `width`/`height`.
    /// `None` when it declares neither, which is legal and common for a fragment.
    pub size: Option<Size>,
    /// How many shapes were created. `0` is not an error at this level — an `<svg>`
    /// holding only a `<defs>` is valid markup — but a caller pasting on a user's
    /// behalf will want to refuse it.
    pub shapes: usize,
    /// Element names met and not understood, deduplicated and in the order first
    /// met. **The caller owes the user this list**; see the module docs.
    pub skipped: Vec<String>,
    /// Features read but **not exactly** — an SVG `<mask>` whose luminance and alpha
    /// readings differ, or clip units this resolves in the wrong space.
    ///
    /// **A different report from [`Self::skipped`] and worth the second field**: one
    /// says a thing is *missing* and the other says it is *there and slightly
    /// wrong*. Telling a user their mask was skipped, over a drawing that is visibly
    /// masked, sends them looking for the wrong bug.
    pub approximated: Vec<String>,
}

/// What the host does with the bytes an `<image>` carries.
///
/// ⚠️ **A callback rather than a function, because core can do neither half of it.**
/// The intrinsic pixel size comes from *decoding*, which lives in `ondin-render`, and
/// the id is a content hash, which uses a crate only the app has — and the bytes have
/// to reach the renderer's decode cache anyway or the picture is a placeholder. So
/// the one caller that can answer this is the same one that already answers it for a
/// drop and a file dialog (`OndinApp::load_image_bytes`), and passing it in is what
/// keeps this module headless.
///
/// `None` for bytes that will not decode, which is a skipped `<image>` rather than a
/// failed import.
pub trait ImageSink {
    fn place(
        &mut self,
        bytes: Vec<u8>,
        format: crate::image::ImageFormat,
    ) -> Option<(crate::image::ImageId, crate::image::ImageEntry)>;
}

/// The four characters XML calls whitespace — its `S` production, `#x20 | #x9 |
/// #xD | #xA`, and nothing else (§15 D660, `[S7.1-L1-08]`).
///
/// 🚨 **Not `char::is_whitespace`, which was the rule here until 2026-09-09 and
/// is Unicode's `White_Space` property — twenty-three characters.** Four of them
/// are exactly the ones an author reaches for *because* they are not ordinary
/// spaces, and collapsing them threw away the reason they were typed. Measured on
/// import, reading the content off the transaction:
///
/// | `<text>` content | imported as |
/// | --- | --- |
/// | `10\u{00a0}km` (no-break space) | `"10 km"` — breakable |
/// | `1\u{2007}5` (figure space) | `"1 5"` — a different width |
/// | `1\u{202f}2` (narrow no-break space) | `"1 2"` |
/// | `\u{3000}a` (leading ideographic space) | `"a"` — gone entirely |
///
/// A pasted price, measurement or date the source file deliberately kept
/// unbreakable came back breakable, and nothing was reported. ⚠️ **`\u{feff}` was
/// always left alone** — it carries no `White_Space` property — which is the
/// control saying the rule really was `char::is_whitespace` rather than something
/// narrower that happened to include these.
///
/// CSS's `white-space: normal` collapses the same four, so the old predicate was
/// wider than both specifications *and* than this function's own doc comment,
/// which has said *"XML whitespace collapsing"* since it was written.
const XML_SPACE: [char; 4] = [' ', '\t', '\r', '\n'];

/// Append `text` to `out` with **XML whitespace collapsing**, which is what
/// `xml:space="default"` means and is not optional for pretty-printed markup: a
/// `<text>` written across three indented lines carries newlines and tabs that are
/// layout in the file and nothing in the drawing.
///
/// Collapsing [`XML_SPACE`] and no more — see that constant for what used to
/// happen to a no-break space.
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
/// `None` for a gradient, which is the one fill a character span cannot hold.
///
/// **Folding the opacity here is what makes this comparable with the node's own
/// [`crate::typography::TextStyle::color`]**, which [`Builder::text_node`] builds
/// through this same function; comparing the raw colours would call two runs
/// identical when they differ only in `fill-opacity`.
fn solid_fill(s: &Style) -> Option<Color> {
    match &s.fill {
        Some(Paint::Solid(c)) => Some(c.multiply_alpha(s.fill_opacity)),
        _ => None,
    }
}

/// [`solid_fill`] at the precision a span actually stores it in.
///
/// ⚠️ **The comparison that decides "does this run differ" has to ask the
/// question [`crate::typography::Spans`] will ask, and that question is in
/// `u8`** (§15 D396). `CharAttr::Color` is canonicalized to `to_rgba8` on its way into a
/// span, and `Spans::normalize` then drops any span equal to the node default —
/// so comparing at `f32` can call a run different, mint a span for it, and have
/// the span silently vanish. Answering in `u8` on both sides means a run that
/// gets a span keeps it.
fn ink(s: &Style) -> Option<[u8; 4]> {
    solid_fill(s).map(|c| c.to_rgba8().to_u8_array())
}

/// The bytes and format inside a `data:` URI, and `None` for anything else.
///
/// **Only the base64 form**, which is what every exporter writes for a picture:
/// percent-encoded binary in an attribute is legal and does not happen. The MIME
/// type decides the format rather than sniffing the bytes, because the file said so
/// and a `<image>` whose MIME lies is broken markup rather than a case to recover.
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
    // Whitespace inside the payload is legal in an XML attribute and is not legal
    // base64, so it goes before the decode rather than after it fails.
    let clean: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(clean)
        .ok()?;
    Some((bytes, format))
}

/// Why a string could not be read as SVG at all.
///
/// **Neither of the first two is about the drawing.** Anything inside a
/// well-formed `<svg>` that this cannot read is a *skip* rather than an error,
/// because a paste that refuses a whole icon over one unsupported element is the
/// wrong trade. ⚠️ **The third is the one exception, and it is not about the
/// drawing either**: [`SvgError::TooDeep`] is a refusal to hand the text to a
/// parser that would overflow the stack on it (§15 D851), so it is decided before
/// there is anything to skip.
#[derive(Debug, PartialEq, Eq)]
pub enum SvgError {
    /// Not well-formed XML.
    NotXml,
    /// Well-formed XML whose root is not `<svg>` — HTML, a plist, a fragment.
    NotSvg,
    /// Elements nested past [`MAX_XML_DEPTH`], refused before the XML parser is
    /// asked (§15 D851).
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

/// How deeply the *markup* may nest before [`import`] refuses it unparsed
/// (§15 D851, `[X7-L1-01]`).
///
/// 🚨 **A 25 KB SVG aborted the process.** 1,500 nested `<g>` overflowed the
/// 1 MiB main-thread stack the shipped binary reserves — `STATUS_STACK_OVERFLOW`,
/// which nothing catches and nothing reports, taking the session with it — and it
/// did so inside `roxmltree`, *before* [`MAX_SVG_NESTING`] or any other budget
/// here is consulted. Measured: a `<notsvg>` root that bails three lines after
/// the parse aborted at the same depth, and 20,000 *sibling* elements imported in
/// 9 ms, so it is depth and it is the parser. `roxmltree` offers no depth option,
/// so the depth is read off the text first ([`nests_deeper_than`]).
///
/// **96, measured rather than chosen, and the debug profile is what sets it.**
/// On a fresh thread with the 1 MiB main-thread stack, the parser alone survives
/// **170** levels in debug and overflows at **180** (≈6 KB a level), and survives
/// **1,200** in release and overflows at **1,400** (≈0.8 KB a level). The first
/// draft of this said 256 on the release figure and its own test overflowed in
/// debug at 255. The real main thread is not fresh — `paste_svg` runs under the
/// whole egui frame — so the bound keeps about 45% of the debug stack for that:
/// 96 levels is ~560 KB there and ~75 KB in the shipped build.
///
/// **1.5× the builder's own [`MAX_SVG_NESTING`]**, because the two bound
/// different things: a document nested 65–96 deep still imports, with the
/// subtrees past 64 skipped and reported as they always were, and only one the
/// parser itself might not survive is refused whole.
pub const MAX_XML_DEPTH: usize = 96;

/// Whether `svg`'s elements nest deeper than `limit`, read from the text without
/// parsing it — so a document the XML parser would overflow its stack on is
/// refused before it gets there (§15 D851).
///
/// **Conservative in the direction that refuses**, and it has to be: the question
/// is asked of the most untrusted input the app takes. Comments, CDATA,
/// processing instructions and quoted attribute values are stepped over, since a
/// `<` inside any of them is not a tag. ⚠️ **Entity expansion is the door a plain
/// tag count cannot see**: a `<!DOCTYPE>` internal subset can declare an entity
/// whose value is markup, and `roxmltree` expands references ten deep — so the
/// deepest markup inside any quoted string in the doctype is counted **ten
/// times** on top of the document's own depth. That over-counts every real file,
/// none of which puts nested elements in an entity.
///
/// Malformed text (an unclosed comment, a tag with no `>`) answers `false` and is
/// left to the parser to refuse as not XML.
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

/// **A cheap "is this worth parsing at all".**
///
/// The paste path asks this of every clipboard string before doing anything, so it
/// has to be fast and must not allocate. It answers the question a user would:
/// *does this look like SVG* — an `<svg` tag start, or an XML declaration followed by
/// one. Deliberately not a parse: [`import`] does that, and a string that passes here
/// and fails there is a paste that says why rather than a paste that silently makes a
/// text layer.
/// ⚠️ **This is the only implementation, and it was not** (§15 D671,
/// `[S16.3-L3-04]`). The app's drop path carried a second one that answered
/// `false` for a `<!DOCTYPE svg …>`-first file — the shape most SVG written before
/// about 2015 has — so the same drawing was *"Could not read: logo.svg"* when
/// dropped and imported as layers when pasted. `app::looks_like_svg` is now the
/// byte-to-`str` adapter and nothing else.
pub fn looks_like_svg(s: &str) -> bool {
    let head = before_root(s);
    head.starts_with("<svg") || head.starts_with("<!DOCTYPE svg")
}

/// `s` with everything that may legally precede the root element removed.
///
/// ⚠️ **The comment skipping is new and the sentence describing it was not** (§15
/// D671). This function's text used to read *"A leading `<?xml …?>` or a comment
/// is ordinary; skip to the first element"* sitting above code that skipped a
/// prolog and no comment at all — so the claim was half false, and the half it was
/// false about is what Illustrator writes: a `<!-- Generator: Adobe Illustrator …
/// -->` line between the declaration and the doctype. The sentence is now true
/// rather than corrected, because a comment before the root really is ordinary and
/// [`import`]'s parser has never minded one.
///
/// **A byte-order mark is deliberately still refused**, and it is not the same
/// question: a BOM reaches the *parser* as well, so admitting one here would trade
/// a wrong refusal for an error message from `roxmltree`. That wants driving
/// rather than reasoning, and is left where `[S16.3-L3-04]` put it.
fn before_root(s: &str) -> &str {
    let s = s.trim_start();
    let mut head = s.strip_prefix("<?xml").map_or(s, |rest| {
        rest.find("?>")
            .map_or(rest, |i| &rest[i + 2..])
            .trim_start()
    });
    // A loop rather than one strip: a generator comment and a licence comment in a
    // row is an ordinary way for a file to open.
    while let Some(rest) = head.strip_prefix("<!--") {
        // An unterminated comment is not markup anything can look past — and
        // answering `rest` here would let the *inside* of a comment pass the test:
        // `<!--<svg/>` with no `-->` would read as a drawing.
        let Some(i) = rest.find("-->") else {
            return "";
        };
        head = rest[i + 3..].trim_start();
    }
    head
}

/// Read `svg` and build the nodes it describes under `parent`.
///
/// The subtree is wrapped in one `Group` whichever way the markup is shaped, for two
/// reasons that are really one: a paste is **one thing** to the user — one undo step,
/// one selection, one layer to drag — and an `<svg>` root is itself a container, so
/// the group is what its transform and its `viewBox` have to live on.
pub fn import<'s>(
    svg: &str,
    ids: &'s mut IdSource,
    parent: NodeId,
    index: usize,
    // `None` where the caller has nowhere to put pictures — a test, or a headless
    // path — and then an `<image>` is skipped and counted like anything else.
    images: Option<&'s mut dyn ImageSink>,
) -> Result<Import, SvgError> {
    // ⚠️ **`allow_dtd`, or half the SVG in the world is "not XML".** roxmltree refuses
    // a `<!DOCTYPE>` by default, and an SVG 1.1 file written by Inkscape, Illustrator
    // or anything else of that era opens with one — Wikipedia's own test card does. It
    // is a *whole-file* refusal, so the symptom is not a missing element but a paste
    // that silently becomes a text layer, and no fixture written by hand has a
    // doctype in it. **Found by running the importer over the real file** after a
    // rendering difference sent me to look at something else entirely.
    //
    // What the flag costs is entity expansion, and roxmltree's own note is the reason
    // this is safe to turn on: it carries billion-laughs checks regardless, and it
    // never resolves an *external* entity, so the file cannot reach the network or
    // the disk. `nodes_limit` is the second belt — this parses whatever is on the
    // clipboard, which is as untrusted as input gets.
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 500_000,
    };
    // 🚨 **Depth first, because the parser is where the stack goes** (§15 D851):
    // `nodes_limit` bounds how *many* nodes, never how deep, and 1,500 nested
    // `<g>` — 0.3% of it — aborted the process inside the call below.
    if nests_deeper_than(svg, MAX_XML_DEPTH) {
        return Err(SvgError::TooDeep);
    }
    let doc = roxmltree::Document::parse_with_options(svg, opts).map_err(|_| SvgError::NotXml)?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err(SvgError::NotSvg);
    }

    let size = declared_size(root);
    // **Declared above `out`, and that is load-bearing rather than tidy** (§15
    // D544): the CSS rules inside `out.gradients` borrow these strings, and Rust
    // drops in reverse declaration order — so a `sheets` declared *after* `out`
    // would be dropped first and would not compile.
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
    // Through `push` like every other op, even though nothing reads
    // `child_counts[parent]` — `parent` is the *caller's* node and the walk below
    // only ever asks about nodes it made itself. Routed anyway so the invariant on
    // the `ops` field is true without an exemption: an invariant with one
    // documented exception is one nobody can check by grepping.
    out.push(Operation::CreateNode {
        id: group,
        parent,
        index,
        kind: NodeKind::Group,
        // The `viewBox` is a transform and nothing else here: it maps the markup's
        // user units onto the width and height the file claims to be. When there is
        // no `width`/`height` to map onto, it is the identity — a fragment's own
        // units are as good a space as any to paste into.
        transform: Some(view_transform(root)),
        name: element_name(root),
    });

    // ⚠️ **The root element's own presentation attributes count**, and forgetting
    // them is invisible on hand-written markup and wrong on every real file: Figma
    // writes `<svg … fill="none">` and lets each shape state its own, so a reader
    // that starts from SVG's initial values instead gives **black** to every child
    // that states none — which is a filled square where an outline was meant to be.
    // Found by importing a real export rather than by reading (`the_root_element_is_styled_too`).
    let inherited = resolve(root, &Style::initial(), &out.gradients);
    // **Reported once for the sheet rather than once per element it should have
    // matched**, which is the only count a reader can act on: "some rules were too
    // complex" is a fact about the file, where "nine elements are unstyled" is a
    // symptom they would have to trace back to it themselves.
    if out.gradients.css.complex {
        out.skip("style (complex selector)");
    }
    // The same shape for the same reason, one line down (§15 D459): a gradient
    // whose stop list was resampled is *there and slightly wrong*, which is
    // `approximate`'s half of the pair rather than `skip`'s.
    if out.gradients.resampled {
        out.approximate("gradient (too many stops)");
    }
    // And once more, for the paint server this reader has none of (§15 D477).
    // `skip` rather than `approximate`: a pattern is *not there* — the shape takes
    // whatever fill it inherits, which for this app's own output is SVG's initial
    // black. See `Gradients::patterns`.
    if out.gradients.patterns {
        out.skip("pattern (image fill)");
    }
    out.children(root, group, &inherited);
    // ⚠️ **After the walk, not before it, and that is the whole reason this flag is
    // a `Cell`** (§15 D493). The two above are facts about the *defs*, settled by
    // `Gradients::collect` before a single shape is built; an unreadable `fill` is
    // a fact about an element, and the elements are what `children` has just
    // walked. Reported as `skip` rather than `approximate` on D477's rule: the
    // paint is not there in some other form, the shape simply takes whatever it
    // inherits.
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
/// **`viewBox` second, not first.** A file saying `width="24" height="24"
/// viewBox="0 0 48 48"` is 24 units wide and drawn at half scale; the caller wants
/// the size it will *occupy*, which is the width and height, and [`view_transform`]
/// is what makes the drawing agree.
fn declared_size(root: roxmltree::Node<'_, '_>) -> Option<Size> {
    if let Some(s) = declared_pair(root) {
        return Some(s);
    }
    view_box(root).map(|r| r.size())
}

/// The layer name an element carries: `data-name` if it has one, else `id`.
///
/// **Six sites read `id` as the name and the writer emitted neither**, which is
/// §15 D611, `[S8.1-L6-04]`: an Ondin document exported to SVG and re-imported came back
/// with every layer unnamed. `ondin_export::svg::name_attr` writes `data-name`
/// now — the argument for that spelling rather than `id` is there — and this is
/// the other half of the pair.
///
/// **`id` stays as the fallback and is not a compatibility shim**: it is what a
/// file from Illustrator or Inkscape puts a layer's identity in, and reading it
/// was right before this and is still right. What changed is only which one wins
/// when a file has both, and a file this writer produced always will.
///
/// One function rather than six copies for the reason `preview_session` is one:
/// a copy that forgot the new attribute would silently be the old behaviour, in
/// exactly one of the six element kinds, and nothing downstream could tell.
fn element_name(el: roxmltree::Node<'_, '_>) -> Option<String> {
    // ⚠️ **Through `naming::usable`, which is the rule both rename fields
    // already applied and this path did not** (§15 D644, `[S2.2-L1-07]`). A
    // `<rect id="">` installed a layer with no label at all, `id="   "` one with
    // three spaces, and `id="a&#10;b"` a name with a line break in a panel that
    // lays out one line — every one of them something `layer_rename_field`
    // refuses per D54. `None` is the right answer for an unusable name here
    // because `op_create` turns it into a generated one, which is exactly what
    // an element carrying no `id` already gets.
    el.attribute("data-name")
        .or_else(|| el.attribute("id"))
        .and_then(crate::naming::usable)
}

/// The `width`/`height` pair the root **declares**, or `None` when it declares
/// nothing usable (§15 D511).
///
/// ⚠️ **One function because two of them disagreed.** The guard was written here
/// and not in [`view_transform`] ten lines below, which read the same two
/// attributes through the same parser and divided by them unchecked. Measured:
/// `width="24" height="0" viewBox="0 0 48 48"` reported a size of `48×48` and gave
/// the pasted subtree the **singular** transform `[0, 0, 0, 0, 12, 0]` — the whole
/// drawing collapsed to a point, which no scale gesture can undo because every
/// handle is in the same place. `width="-24" height="-24"` gave `[-0.5, 0, 0,
/// -0.5, 0, 0]`, the drawing rotated through the origin, where a browser renders
/// nothing at all.
///
/// `view_box` already checked (`w > 0.0 && h > 0.0` on its own numbers), so this
/// was the one of the three sizing reads that did not.
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
/// **`preserveAspectRatio` is not read, and the default is what it would say
/// anyway**: `xMidYMid meet` — fit, centred, aspect preserved — which is what this
/// computes. A file asking for `none` (stretch) or a corner alignment gets the
/// centred fit instead, which is a wrong picture only for markup that deliberately
/// distorts itself.
fn view_transform(root: roxmltree::Node<'_, '_>) -> Affine {
    let Some(vb) = view_box(root) else {
        return Affine::IDENTITY;
    };
    let Some(size) = declared_pair(root) else {
        // No box to fit into: the drawing's own units are the space, and the only
        // thing the viewBox still says is where its origin is.
        //
        // **A non-positive `width`/`height` arrives here too** (§15 D511), through
        // the guard [`declared_pair`] holds for both callers. That is the same
        // reading `declared_size` already took — a zero or negative declaration is
        // no declaration — and it degrades the way a browser's "render nothing"
        // does most gracefully: the drawing pastes at its own units instead of
        // collapsing to a point or arriving upside down.
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
/// ⚠️ **Which properties inherit is a specification detail worth stating, because
/// getting it backwards is invisible on simple input and wrong on real files.**
/// `fill`, `stroke`, the stroke's shape properties and `fill-rule` **inherit**;
/// `opacity`, `transform` and `display` do **not** — they apply to the element and,
/// through it, to its subtree, which is exactly what a `Group` node does with its
/// own opacity and transform. So this struct holds the first list and the second is
/// read per element.
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
    /// ⚠️ **The font properties inherit like the paint does, and this is where that
    /// was learnt.** They were read off the `<text>` element alone until 2026-08-31,
    /// which is wrong twice over: a `font-family` on an enclosing `<g>` — how every
    /// Inkscape file states it — never reached the text at all, and a `font-size` on
    /// a **`<tspan>`** was ignored, so Wikipedia's test card rendered its drop-shadow
    /// "TEST" at the default 16pt off to one side. Carried here, they resolve down
    /// the tree like every other inherited property and a positioned `<tspan>` gets
    /// its own.
    ///
    /// The family is the **raw list**, unresolved: picking from it needs to know what
    /// this machine has, and choosing wrongly is a thing to *report*, which cannot be
    /// done from a pure function.
    font_family: Option<String>,
    font_size: f64,
    weight: u16,
    italic: bool,
    anchor: Anchor,
}

/// One item of a `<text>`'s flattened content — see [`Builder::text_pieces`].
///
/// **Text nodes, not elements**, which is the whole of why the depth stops
/// mattering: each piece names the innermost element its characters came from and
/// that element's resolved style, so `<tspan>x<tspan>y</tspan>z</tspan>` is three
/// pieces and `z` reads the outer tspan's style rather than the inner one's.
enum TextPiece<'d, 'input> {
    /// Characters, the element they came from, and its resolved style.
    Run(&'d str, roxmltree::Node<'d, 'input>, Style),
    /// A positioned `<tspan>`, which ends the run so far and starts a node of its
    /// own. Emitted *before* the pieces inside it.
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
    /// **The transform is carried rather than baked in**, because it composes
    /// *inside* whichever mapping [`GradientUnits`] asks for and that mapping is not
    /// known until the shape it paints is — see [`Paint::brush`].
    ///
    /// The gradient is **boxed**, which is the one thing here that is not about SVG:
    /// a `peniko::Gradient` carries its stops inline and is 217 bytes, against the 16
    /// of a [`Color`], so an unboxed variant would make every `Solid` — the common
    /// case by far, and the initial value of both `fill` and `stroke` — pay a 224-byte
    /// [`Style`] field for a gradient it does not have.
    Gradient(Box<peniko::Gradient>, GradientUnits, Affine),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GradientUnits {
    User,
    /// The default, and the one that needs the shape's own box to resolve.
    ObjectBounding,
}

impl Style {
    /// SVG's initial values, which are **not** "nothing": an element with no `fill`
    /// attribute anywhere above it is filled **black**, and that is why a paste of
    /// markup with no paint at all still shows something.
    fn initial() -> Self {
        Self {
            fill: Some(Paint::Solid(Color::BLACK)),
            stroke: None,
            stroke_width: 1.0,
            stroke_cap: kurbo::Cap::Butt,
            stroke_join: kurbo::Join::Miter,
            dashes: Vec::new(),
            dash_offset: 0.0,
            // SVG's initial value, and ours: 4.
            miter_limit: 4.0,
            fill_rule: FillRule::NonZero,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            current: Color::BLACK,
            // SVG's initial `font-size` is `medium`, which is 16 in every browser and
            // is also this model's default — so the two agree without a conversion.
            font_family: None,
            font_size: 16.0,
            weight: 400,
            italic: false,
            anchor: Anchor::Start,
        }
    }
}

/// One element's presentation attributes, read from the attributes **and** from an
/// inline `style="…"`.
///
/// **`style` wins, which is CSS's own rule** (a declaration beats a presentation
/// attribute), and it is not the only piece of CSS this reads: a `<style>` element's
/// rules are resolved by [`Css`], which since §15 D806 is a small cascade engine —
/// compounds of type, class, id and `*`, joined by descendant and child combinators.
/// What is still skipped and counted is a selector needing sibling order, a
/// pseudo-class or an attribute.
fn property<'a>(el: roxmltree::Node<'a, '_>, name: &str, css: &Css<'a>) -> Option<&'a str> {
    // CSS's own cascade, in the order it puts them: an inline declaration beats a
    // stylesheet rule, and a stylesheet rule beats a presentation attribute. That
    // last step is the one worth stating — a presentation attribute is *weaker* than
    // any rule, however unspecific, which is exactly what makes an Illustrator export
    // work: every shape carries `class="cls-1"` and the class decides.
    // ⚠️ **No `?` on the `style` attribute.** Most elements do not have one, and a
    // `?` here returns `None` for the whole *property* rather than falling through to
    // the sheet and the attribute — which reads as "nothing has any paint" and is how
    // this function was first written.
    if let Some(style) = el.attribute("style")
        && let Some(v) = declaration(style, name)
    {
        return Some(v);
    }
    css.declaration(el, name).or_else(|| el.attribute(name))
}

/// One `name: value` out of a `;`-separated declaration block.
///
/// The returned slice borrows from `block`, which borrows from the document — so the
/// value outlives this call without being copied.
fn declaration<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    block.split(';').find_map(|decl| {
        let (k, v) = decl.split_once(':')?;
        // ⚠️ **`!important` is stripped rather than parsed** (§15 D493,
        // `[S7.1-L1-03]`). It is a *specificity* input in real CSS, and this reader
        // has no important-vs-normal ordering to get wrong — it is a lookup, not a
        // cascade engine, which is what the `Css` doc below says at length. So the
        // only thing the marker can do here is make an otherwise perfectly good
        // value unreadable, and it did: `fill:#ff0000 !important` imported the
        // shape with **zero** fills, invisible, with both report lists empty.
        let v = v.trim();
        let v = v
            .strip_suffix("!important")
            .map_or(v, |head| head.trim_end())
            .trim_end_matches(|c: char| c.is_whitespace());
        (k.trim() == name).then_some(v)
    })
}

/// The document's `<style>` rules, flattened to the three selectors that matter.
///
/// ⚠️ **This is a small cascade engine, and the line is drawn at the *selector*.**
/// Illustrator's default export — the reason this exists — writes
/// `.cls-1{fill:#231f20;}` and hangs `class="cls-1"` on every shape, which a type,
/// class and id matcher already covered. Since §15 D806 it also reads **compounds**
/// (`rect.a#b`, `*`) and the **descendant** and **child** combinators, because a
/// hand-written sheet reaches for `.outer .inner` immediately and that was the
/// commonest thing this refused.
///
/// **What is still refused is what needs something other than an ancestor walk**:
/// `+` and `~` need sibling order, `:pseudo` needs state, `[attr]` needs attribute
/// matching. Those are **counted and ignored** rather than half-matched — as is a
/// malformed selector, which is refused rather than repaired, since repairing
/// `.a > > .b` into `.a > .b` would silently apply a *wider* rule than the one
/// written.
#[derive(Default)]
struct Css<'a> {
    /// In document order, which is the tiebreak when specificity ties.
    rules: Vec<(Sel<'a>, u32, &'a str)>,
    /// Whether any selector was too complex to read, so the import can say so.
    complex: bool,
}

/// The simple selectors that must all match **one** element — `rect.a#b`, or `*`.
///
/// `tag: None` is the universal selector, which matches any element and adds
/// nothing to specificity, exactly as CSS has it.
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
/// ancestors to its left (§15 D806).
///
/// **`chain` runs right to left**, so `chain[0]` is the compound immediately left
/// of the subject. That is the order matching walks in, and it is why CSS engines
/// match right-to-left too: the subject is the cheap filter, and most rules fail
/// on it before an ancestor is ever looked at.
struct Sel<'a> {
    subject: Compound<'a>,
    chain: Vec<(Combinator, Compound<'a>)>,
}

impl Sel<'_> {
    fn matches(&self, el: roxmltree::Node<'_, '_>) -> bool {
        self.subject.matches(el) && match_chain(&self.chain, el)
    }

    /// Specificity as one number, ids scaled over classes over types.
    ///
    /// ⚠️ **The scale is 100/10/1 and a selector with more than nine classes on
    /// one side would carry into the next place.** That is the same scale the
    /// single-component version used before combinators, kept because a real SVG
    /// stylesheet does not reach it and because the alternative — comparing the
    /// triple — is a different ordering to write and test for no case anybody has.
    /// Named so it is a known bound rather than an assumption.
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
/// **A descendant combinator backtracks**, and the shape that proves it needs a
/// `>` to the **left** of a descendant. 🚨 **`.a .b .c` does not**, which is the
/// example this doc and §15 D806 both gave: a descendant chain's ancestor sets
/// are nested, so if the nearest `.b` has no `.a` above it then no `.b` further
/// up has one either, and greedy and backtracking **necessarily agree**. In
/// `.a > .b .c` they do not — committing to the nearest `.b` fails on its plain
/// `<g>` parent while a `.b` further up has `.a` for a direct parent, which is
/// what `a_descendant_chain_reconsiders_an_ancestor_that_fails_further_left` is
/// built on — plain backticks, the test being `cfg(test)` and so invisible to
/// `cargo doc` (§15 D319).
///
/// 🚨 **It used to backtrack by recursing, and that made a kilobyte of SVG hang
/// the import** (§15 D837). Each descendant arm looped over every ancestor and
/// recursed on each match, with no memo, so
/// `nope g g g g g g g g g g text` against sixty nested `<g>` searched every
/// increasing sequence of ten ancestors before failing on the `nope` that never
/// matches — and it is called once per element per property the cascade asks
/// about. Measured at **334 bytes / 30 levels / 12.6 s**, with a 534-byte
/// fixture not finishing in 120 s. Neither [`MAX_SVG_NESTING`] nor
/// [`MAX_SVG_NODES`] is consulted here and neither is even approached: the file
/// is tiny and shallow by both measures. `architecture.md` §5.11's series — *a
/// depth bound is not a size bound* (§15 D446), *a size bound is not a payload
/// bound* (§15 D459) — gains a **fourth** quantity in [`MAX_CSS_RULES`] (§15
/// D838), and this is the case that is none of them: what explodes is the work
/// *per element*, which is not a quantity a file declares.
///
/// **The fix is not a cap, because the search was never exponential in
/// nature.** An element's ancestors are a **path** — `parent_element` walks one
/// line, not a tree — so "does `chain` match some increasing subsequence of the
/// ancestors" is ordinary subsequence matching, and the blow-up was the
/// recursion re-deriving the same sub-answers rather than anything inherent.
/// Written as a table over `(chain index, ancestor index)` it is exactly the
/// same predicate in `O(chain × depth)`, so **no selector that used to be
/// honoured stops being honoured**. A `MAX_SELECTOR_COMPOUNDS` was the other
/// candidate and is deliberately not taken: it would cost expressiveness to buy
/// a bound this already gives for free, and at 8 compounds over 64 levels
/// `C(64, 8) ≈ 4.4 × 10⁹` is not a bound anyway — the cap would have looked like
/// a fix while leaving the hang reachable.
///
/// `g(i, j)`, below, is *"can `chain[i..]` be satisfied using ancestors from the
/// `j`-th upwards"*, and the answer is `g(0, 0)`.
fn match_chain(chain: &[(Combinator, Compound<'_>)], el: roxmltree::Node<'_, '_>) -> bool {
    if chain.is_empty() {
        return true;
    }
    // Nearest ancestor first, so `path[0]` is the parent. Bounded by the
    // document's own nesting, which is all this ever walks.
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
    /// Specificity is scaled so one id beats any number of classes and one class beats
    /// any number of types, which is what the real cascade does. ⚠️ **It is summed
    /// across the whole selector since §15 D806** — this sentence used to end
    /// *"cheaper than counting components — every selector here has exactly one"*,
    /// which combinators made false.
    ///
    /// ⚠️ **The tree walk is deliberately last.** `Sel::matches` is the only part of
    /// this that touches ancestors, so the specificity test and the declaration's own
    /// presence are checked first and most rules never reach it.
    fn declaration(&self, el: roxmltree::Node<'_, '_>, name: &str) -> Option<&'a str> {
        let mut best: Option<(u32, &'a str)> = None;
        for (sel, spec, block) in &self.rules {
            // `>=` rather than `>`: a later rule of equal specificity wins, which is
            // the tiebreak CSS uses and the reason the rules are kept in order.
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
/// ⚠️ **An empty compound is `None`, not the universal selector.** `.a > > .b`
/// tokenizes to one, and reading it as `*` would silently widen the rule to every
/// element rather than reporting a selector nobody can parse.
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
    // The leading run, before any `.` or `#`, is the element name. `*` is the
    // universal selector and leaves `tag` as `None`.
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

/// A whole selector, with descendant and child combinators (§15 D806).
///
/// **What is read and what is still reported**: type, class, id, `*` and any
/// compound of them, joined by whitespace or `>`. A `+` or `~` needs sibling
/// order, and a `:pseudo` or `[attr]` needs state or attribute matching — each
/// is a different program, and each still comes back `None` so the import says
/// so rather than half-matching.
fn parse_selector(sel: &str) -> Option<Sel<'_>> {
    // Rejected up front rather than by falling through `parse_compound`, so the
    // reason a selector was skipped stays readable in one place.
    if sel.contains(['+', '~', '[', ']', ':', '(', ')', ',']) {
        return None;
    }
    // Tokenize into compounds and the combinators between them. Whitespace is a
    // descendant combinator unless a `>` is adjacent, in which case the `>` wins —
    // which is why the combinator is decided when the *next* compound starts and
    // not when the whitespace is seen.
    let mut parts: Vec<(Combinator, &str)> = Vec::new();
    let mut pending = Combinator::Descendant;
    let mut first: Option<&str> = None;
    for token in sel.split_whitespace().flat_map(|w| {
        // `a>b` has no spaces in it, so `>` splits within a word too. The empty
        // strings a leading or trailing `>` produces are kept, and
        // `parse_compound` refuses them.
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
            // A `>` with nothing before it anywhere is not a selector, and neither
            // is a second one with no compound in between — `.a > > .b` would
            // otherwise read as `.a > .b`, which is a *wider* rule than the one
            // written and is the shape of silently accepting malformed input.
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
        // ⚠️ **A rule with a CSS comment in it is dropped, not parsed around.** Every
        // value here is a *slice* of the document, so stripping comments would mean
        // copying the sheet into a `String` that this function does not outlive.
        // Dropping is also the right answer most of the time — a comment in a
        // stylesheet usually *is* a disabled rule — and the rest is reported.
        if sels.contains("/*") || block.contains("/*") || block.contains("*/") {
            complex = true;
            continue;
        }
        for sel in sels.split(',') {
            let sel = sel.trim();
            if sel.is_empty() {
                continue;
            }
            // The sheet's own length is a bound too (§15 D838). Reported rather
            // than refused, and the rules already read still apply — a sheet
            // past this is a generated one, and the shapes in such a file carry
            // presentation attributes as well.
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
    /// **Append only through [`Self::push`]**, which is what keeps
    /// [`Self::child_counts`] true. A bare `ops.push` of a `CreateNode` would make
    /// [`Self::index_in`] answer one low for that parent from then on, and the
    /// symptom is a paste whose children arrive in the wrong order — silent, and a
    /// long way from the line that caused it.
    ops: Vec<Operation>,
    /// How many nodes this import has emitted in total, against [`MAX_SVG_NODES`].
    ///
    /// Separate from [`Self::shapes`], which counts only what the *user* would
    /// call a shape and is a report rather than a bound: a group, a clip wrapper
    /// and a `<use>` instance all cost memory and none is a shape.
    created: usize,
    /// How many children each parent has been given so far, so [`Self::index_in`]
    /// is a lookup rather than a scan of every op emitted.
    ///
    /// ⚠️ **This is a performance fix with a correctness trap in it, which is why
    /// the funnel above exists** (§15 D446). `index_in` used to count matching `CreateNode`s
    /// in `ops` — always right, and Θ(n²) over the import, because `ops` only
    /// grows and every created node scanned all of it. Measured before the change:
    /// 4,000 shapes 11.9 ms, 8,000 37.7 ms, 16,000 380 ms, 32,000 **1.81 s** —
    /// 4.75× for a 2× input, on the egui UI thread, and a traced illustration is
    /// routinely in that range. Same shape as `export::svg`'s `Defs::add`
    /// (`[A4-L4-02]`), on the other side of the same file format.
    child_counts: rustc_hash::FxHashMap<NodeId, usize>,
    shapes: usize,
    skipped: Vec<String>,
    approximated: Vec<String>,
    gradients: Gradients<'d, 'input>,
    /// Whether the walk is inside a `<clipPath>`, which changes exactly one thing:
    /// the fill rule comes from `clip-rule` rather than from `fill-rule`. A flag
    /// rather than a parameter because it is true for a *subtree* and false
    /// everywhere else, and threading it would touch every arm of [`Self::node`].
    in_clip: bool,
    /// How deep the walk is inside `<use>` instances, so a reference cycle stops
    /// rather than recursing forever.
    use_depth: usize,
    /// How deep the walk is inside nested elements, so a bomb stops rather than
    /// overflowing the stack. See [`MAX_SVG_NESTING`]; `use_depth` is the same
    /// idea for a different kind of descent and the two are counted separately
    /// because they bound different things.
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

    /// **Read, but not exactly** — a different report from [`Self::skip`], and the
    /// distinction is the one a user cares about: a skipped thing is *missing* and a
    /// approximated one is *there and slightly wrong*. Saying "skipped mask" over a
    /// drawing that is visibly masked would send someone looking for the wrong bug.
    fn approximate(&mut self, name: &str) {
        if !self.approximated.iter().any(|s| s == name) {
            self.approximated.push(name.to_string());
        }
    }

    /// Walk `el`'s element children into `parent`, in document order — which is
    /// **paint order**, and therefore already this app's bottom-to-top child order.
    ///
    /// ⚠️ **This is the one choke point of the element recursion, so the depth
    /// bound lives here.** `children → node → node_ignoring → children` is
    /// unbounded otherwise, and a `<g><g>…</g></g>` of a few hundred levels —
    /// which is one line to generate and, as this module says of clipboard SVG,
    /// *"as untrusted as input gets"* — aborted the process at depth 200 in a
    /// debug build. That is a `STATUS_STACK_OVERFLOW`, not a panic: nothing
    /// catches it, nothing reports it, and the session goes with it. Past the
    /// bound the subtree is **skipped and reported**, which is the degradation
    /// this module is built around.
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
    /// **The whole mapping is that our mask node clips the run of siblings *above*
    /// it** (§15 D282–D286), so a wrapper holding `[clip, content]` is exactly SVG's
    /// `clip-path` — with no new model concept and no special case in the renderer.
    ///
    /// ⚠️ **The transform goes on the clip, not on the wrapper**, and getting that
    /// backwards is the one way to build this wrongly while every shape still has the
    /// right size. SVG resolves `clipPathUnits="userSpaceOnUse"` — the default — in
    /// the user space of the *referencing* element, which includes that element's own
    /// `transform`. So the clip must be transformed exactly as the content is: the
    /// wrapper stays at the identity, and both children carry `el`'s transform, the
    /// content because it always did.
    ///
    /// **`clip-path` and `mask` differ only in the mode.** A clip is
    /// [`MaskMode::Shape`] — hard-edged, the outline decides — and a `<mask>` is
    /// [`MaskMode::Alpha`]. ⚠️ *SVG's mask is a **luminance** mask and ours is not*,
    /// which agrees exactly when the mask is opaque white and disagrees otherwise;
    /// [`luminance_equivalent`] is what decides whether that gets reported.
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
        // ⚠️ **A container clips *itself* rather than getting a wrapper**, which is
        // the difference between two layers and one on every real export — a Figma
        // file's outermost `<g>` carries `clip-path`, so the wrapper would be a
        // permanent extra level on top of every paste. A group's own first child
        // clips the run above it inside that group, which is exactly the same
        // picture, and the clip needs no transform of its own because it is already
        // in the group's space.
        //
        // ⚠️ **Not when the *other* clip attribute is still to be consumed.** Two
        // masks in one parent are two *runs*, not two clips over one run: the first
        // would clip only the (empty) stretch before the second, and one of the two
        // would silently do nothing. So an element carrying both takes a wrapper for
        // the first and hoists the second.
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
            // The element builds first, and the clip goes in front of its children
            // afterwards — `CreateNode`'s index *inserts*, so index 0 puts it at the
            // bottom of a list that already exists.
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

        // ⚠️ **`objectBoundingBox` units are read as user space and reported**, not
        // silently applied: resolving them needs the *referencing* element's bounds,
        // which do not exist until it is built, and the wrong answer here is a clip
        // in the wrong place rather than a missing one. Rare in exported files —
        // Figma and Illustrator both write `userSpaceOnUse`.
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
            // **Hoisted, the clip is already inside the element's own space**, so it
            // needs no transform; wrapped, it has to carry the element's, because
            // `userSpaceOnUse` resolves in the *referencing* element's user space —
            // which includes that element's `transform`.
            transform: (!hoist).then(|| element_transform(el)),
            // Named, because an unnamed group in a layer tree is "Group 4" and this
            // one is the only child a reader cannot see.
            name: Some(if attr == "clip-path" { "Clip" } else { "Mask" }.to_string()),
        });
        self.push(Operation::SetMask {
            id: clip,
            mask: true,
        });
        if mode != MaskMode::default() {
            self.push(Operation::SetMaskMode { id: clip, mode });
        }

        // **The clip's contents inherit from the *def*, not from the element it
        // clips** — which is SVG's rule and matters for a `<mask>`, whose
        // `fill="white"` is conventionally written on the `<mask>` element itself.
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

    /// Whether the node budget is spent — see [`MAX_SVG_NODES`].
    ///
    /// **Asked at the one choke point every emitted node passes through**
    /// ([`Self::node_ignoring`]), rather than at the six `CreateNode` pushes. A
    /// budget checked at the push would stop mid-element — a group created and its
    /// clip not, a shape created and its fill not — and leave the transaction
    /// describing something nobody asked for. Checked at the top of the element
    /// instead, the last thing built is whole.
    fn out_of_budget(&mut self) -> bool {
        if self.created >= MAX_SVG_NODES {
            self.skip("too many elements");
            return true;
        }
        false
    }

    /// The next child index under `parent`, counted from the ops built so far.
    ///
    /// ⚠️ **Read *before* the `CreateNode` it indexes, never after** — [`Self::push`]
    /// bumps the count, so asking afterwards gives the index of the node after this
    /// one. Every caller today reads it into a local first.
    fn index_in(&self, parent: NodeId) -> usize {
        self.child_counts.get(&parent).copied().unwrap_or(0)
    }

    fn node(&mut self, el: roxmltree::Node<'d, 'input>, parent: NodeId, inherited: &Style) {
        self.node_ignoring(el, parent, inherited, &[]);
    }

    /// [`Self::node`], with the clip attributes a wrapper has already consumed.
    ///
    /// **`ignore` is what stops the recursion**: [`Self::masked`] builds the element
    /// again inside the wrapper it just made, and the element still carries the
    /// attribute that sent it there. An element with *both* a clip and a mask comes
    /// through twice, one wrapper each.
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
        // ⚠️ **Inside a `<clipPath>` the rule is `clip-rule`, not `fill-rule`**, and
        // they are different properties with the same values — an element inside a
        // clip has a `fill-rule` too and it means nothing there. Applied here rather
        // than in `resolve` because the difference is a property of *where the walk
        // is*, and the overridden style is what descends, so a `clip-rule` on a `<g>`
        // inside the clip inherits like any other.
        if self.in_clip
            && let Some(v) = property(el, "clip-rule", &self.gradients.css)
        {
            style.fill_rule = if v.trim() == "evenodd" {
                FillRule::EvenOdd
            } else {
                FillRule::NonZero
            };
        }
        // **A clip or a mask becomes a mask *node***, which the model has had since
        // §15 D282–D286 — so these two stop being losses and become a wrapper.
        // `clip-path` first: an element carrying both is clipped by the path and
        // then masked, and one wrapper each is exactly that, innermost last.
        //
        // ⚠️ **Both read as *properties*, for the reason the module header now states
        // as a rule**: they are presentation properties, so `style="clip-path:url(#c)"`
        // is as legal as the attribute and is what a CSS-shaped export writes. Reading
        // them with `attribute` was the same latent bug `filter` had actually hit —
        // found by grepping every attribute read in this file against the list of
        // properties, once the `filter` one had cost a drawing its blurs.
        for (attr, mode) in [("clip-path", MaskMode::Shape), ("mask", MaskMode::Alpha)] {
            if ignore.contains(&attr) {
                continue;
            }
            // `none` is the property's *initial* value, so an element stating it is
            // saying it has no clip — reporting that as a dropped one would be a
            // report about nothing, which is the failure mode the whole list has.
            let Some(value) =
                property(el, attr, &self.gradients.css).filter(|v| v.trim() != "none")
            else {
                continue;
            };
            if let Some(def) = self.gradients.clip(value) {
                return self.masked(el, def, mode, attr, parent, inherited, ignore);
            }
            // A reference that resolves to nothing is not a clip. SVG says an
            // element with an unresolvable `clip-path` is **not rendered** at all;
            // drawing it unclipped is the friendlier wrong answer and is what a
            // paste wants, so it is reported rather than obeyed.
            self.skip(attr);
        }
        let index = self.index_in(parent);

        match el.tag_name().name() {
            // Containers. `<a>` is a link round a drawing and is a group to us; its
            // href is not something the model can hold, and losing it is not losing
            // any ink.
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
            // **Definitions are read by lookup, not by walking.** A `<defs>` holds
            // gradients this already collected and things it cannot use; walking it
            // would paste its contents as visible shapes, which is the one thing
            // `<defs>` means it must not do. A `<symbol>` is the same rule stated by
            // a different element: it exists to be instantiated by a `<use>` and is
            // never drawn where it stands.
            // `<style>` is read once, up front, by `Gradients::collect` — walking it
            // here would try to draw a stylesheet.
            //
            // ⚠️ **`clipPath` and `mask` are on this list because they are
            // *definitions wherever they sit*** (§15 D477). `Gradients::collect`
            // walks `doc.descendants()` and registers both by id whatever their
            // depth, so one outside a `<defs>` had already been read — and falling
            // through to the unknown-element arm below reported it **skipped**
            // when it had round-tripped perfectly. `[A5-L6-01]` measured exactly
            // that on this app's own output: `skipped = ["clipPath"]` for an
            // artboard clip the import had used. **A false positive on the
            // skipped list is worse than a missing entry** — the list is the one
            // thing telling a user what a paste lost, and an item on it that was
            // not lost teaches them not to read it.
            "defs" | "title" | "desc" | "metadata" | "symbol" | "style" | "clipPath" | "mask" => {
                None
            }
            // **`<use>` is a deep copy, and copying is all it is here.** SVG defines
            // it as a clone of the referenced element inside a `<g>` carrying the
            // use's own transform and its `x`/`y`, which is exactly the group built
            // below — so the *same* walk that built the original builds the copy, and
            // there is no second mapping to keep in step.
            //
            // ⚠️ **`width`/`height` are read only for a `<symbol>` or `<svg>` target**
            // in the specification, and are ignored entirely here — they would
            // establish a viewport for the instance, which is a `viewBox` question
            // rather than a transform. Reported, because an icon sprite that sizes its
            // instances this way would come out at the symbol's own size.
            "use" => {
                let href = el
                    .attribute("href")
                    .or_else(|| el.attribute(("http://www.w3.org/1999/xlink", "href")));
                let Some(target) = href.and_then(|h| self.gradients.element(h)) else {
                    self.skip("use (unresolved)");
                    return None;
                };
                // ⚠️ **A `<use>` can reference something that contains it.** The
                // markup is legal to write and the recursion is not — a browser
                // detects the cycle and draws nothing. A depth cap is the cheap
                // version of that check and cannot be wrong about a real drawing:
                // nothing legitimate nests instances sixteen deep.
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
                // A `<symbol>` contributes its *contents*; anything else contributes
                // itself, which is the one place the two differ.
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
                // ⚠️ **`rx`/`ry` default to each other**, which is the one place SVG's
                // rounded rect is not the obvious reading: `rx="4"` alone means both.
                // Each is then clamped to half its own side, which is the *other*
                // non-obvious part — `ry="65"` on a 130-tall rect is not an overshoot
                // to reject but a corner that consumes the whole edge.
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
                    // Our ellipse is a box at the origin; SVG's is a centre and two
                    // radii. The corner of that box is the translation.
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
            // **A point list is a `Path`, not a `Polygon`.** `NodeKind::Polygon` is a
            // regular *n*-gon with a radius, which is a different shape entirely —
            // the writer emits `<polygon>` for one because the markup happens to fit,
            // and reading every `<polygon>` back as one would turn an arbitrary
            // outline into a regular hexagon. Asymmetry on purpose, and the round-trip
            // test asserts the geometry rather than the kind because of it.
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
            // **One text node per *positioned* run**, which is the rule that makes
            // both real shapes come out right — see [`Builder::text`].
            "text" => {
                let made = self.text(el, parent, &style);
                made.last().copied()
            }
            // **A picture is a `Rect` with an image fill**, which is how this model
            // holds every image — there is no `NodeKind::Image` (§5.5a), and that is
            // what makes an imported picture croppable and re-fillable like any other.
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
                    // **An external reference is a skip, not a fetch.** Core does no
                    // I/O, and a paste that reached out to the network for a URL it
                    // found in the clipboard would be a worse feature than a missing
                    // picture.
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
                // **Once per distinct picture**, because the id is a content hash: the
                // same photo used twice is one table entry and two fills, and a second
                // `AddImage` would give the transaction an inverse that removes an
                // entry the first one is still using.
                if !self.added.contains(&image) {
                    self.added.push(image.clone());
                    self.push(Operation::AddImage {
                        id: image.clone(),
                        entry,
                    });
                }
                // ⚠️ **SVG's default is *contain* and ours is *cover***, which is the
                // one place these two models disagree by default rather than by
                // instruction: `preserveAspectRatio` defaults to `xMidYMid meet`. A
                // `slice` asks for cover, and `none` asks for a stretch this model
                // cannot express — reported, since a stretched photo drawn contained
                // is a visible difference.
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
                    // Painted below rather than by `shape`, whose fills come from the
                    // style — a picture is not a colour and has no `fill` to inherit.
                    &Style {
                        fill: None,
                        stroke: None,
                        ..style.clone()
                    },
                );
                self.push(Operation::SetFills {
                    id,
                    fills: vec![Fill {
                        brush,
                        visible: true,
                    }],
                });
                Some(id)
            }
            "path" => {
                let d = el.attribute("d")?;
                let Ok(path) = BezPath::from_svg(d) else {
                    self.skip("path (bad d)");
                    return None;
                };
                // ⚠️ **kurbo's parser accepts `1e999`** — it is a valid `f64`
                // literal that overflows to infinity — and a non-finite
                // coordinate makes the saved document unopenable for good, so
                // `Document::apply` refuses the whole transaction now. Skipping
                // the one path is the graceful half of that: the rest of the
                // file still imports and the user is told what did not.
                // `numbers()` is the same filter for the shapes with coordinate
                // *attributes*; this is the `d` grammar's half.
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

    /// A `<text>` element, as one text node per **positioned** run.
    ///
    /// ⚠️ **The rule is "a `<tspan>` carrying its own `x` or `y` starts a new node",
    /// and it is the only rule that gets both real shapes right.** Two files disagree
    /// about what a `<tspan>` is for. *This app's own writer* emits one `<text>` per
    /// line holding one `<tspan>` per **style run** (§15 D81), so its tspans have no
    /// position and must not each become a layer. *Illustrator* emits one `<text>`
    /// holding one `<tspan x y>` per **line**, so its tspans must. Reading the
    /// position rather than the element is what tells them apart.
    ///
    /// ⚠️ **A multi-line paragraph of ours comes back as several layers, and that is
    /// a real loss rather than an oversight.** The writer put each line at a baseline
    /// the canvas computed; nothing in the markup says those lines were one
    /// paragraph, and inventing a line height to rejoin them would be guessing at the
    /// one number that decides where every line after the first sits. *The round trip
    /// closes for the picture and not for the paragraph* — which is worth knowing
    /// before anyone tries to make it close for both.
    ///
    /// 🚨 **Rejoining it is a decided non-goal for v1** (§15 D826, `roadmap.md` §0),
    /// so the paragraph above is now the end of the argument rather than the start of
    /// one. The only signal a rejoin could read — same `x`, a constant `y` step, a
    /// matching resolved style — is exactly what a *stack of separate labels* in a
    /// foreign file looks like, so the heuristic welds unrelated layers together and
    /// the user has no way to say which reading was meant. ⚠️ **And the case it would
    /// have been worth most for is answered elsewhere**: a copy crossing between two
    /// `ondin` windows carries real nodes now, paragraph and all (§15 D823,
    /// `crate::io::clip`), so what is left here is the foreign file, which never had
    /// the structure to lose.
    ///
    /// **A `<tspan>`'s own style is read, as character spans over the bytes it
    /// contributed** — its colour (§15 D396) and its **font scope**: size, weight,
    /// italic and family (§15 D398). That is `CharSpans`, the model's own per-run
    /// styling (§15 D154), and one `<tspan>` per style run is what this app's writer
    /// emits (§15 D81) — so a line of ours that changes face mid-way survives the
    /// round trip as one node rather than as a report.
    ///
    /// ⚠️ **The byte ranges are collected against `pending`, which is not what the
    /// node is built from.** [`Builder::text_node`] trims, so every range owes that
    /// trim a shift; collecting them here and applying them there is what keeps the
    /// one place that knows how many bytes came off the front in charge of it.
    ///
    /// What a `<tspan>` still loses, both **reported**: a **gradient** fill, a
    /// `CharAttr` holding a colour and not a paint server, and a **stroke** only one
    /// of the two states, strokes being node-level here. ⚠️ **Nothing on a
    /// `<tspan>` is lost silently any more**, which was this module's one breach of
    /// its own contract and is worth checking against rather than assuming the next
    /// time a property is added to `Style`: a field read into `Style` and compared
    /// nowhere below is exactly how the font scope came to be dropped in silence.
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
        // **`<textPath>` moves both the content and the placement** (§15 D406). The
        // runs are inside the child rather than under the `<text>`, and `x`/`y` stop
        // meaning anything at all — a curve places the type. So the loop below reads
        // the child's children, and everything else about this function is unchanged:
        // a `<tspan>` inside a `<textPath>` is a `<tspan>`.
        //
        // **Only the first is read.** SVG allows several `<textPath>`s in one
        // `<text>`, each on its own curve; a text node here holds one rail, so the
        // rest are reported rather than silently merged onto the first one's.
        let rail_el = el.children().find(|c| c.has_tag_name("textPath"));
        if el.children().filter(|c| c.has_tag_name("textPath")).count() > 1 {
            self.skip("text (second textPath on one element)");
        }
        // The style the runs inherit — a `<textPath>` can carry its own `fill` and
        // `font-*` like any other element, and ours does not but Illustrator's does.
        // Resolved before the rail so the anchor below is the one that applies.
        let resolved = match rail_el {
            Some(tp) => resolve(tp, inherited, &self.gradients),
            None => inherited.clone(),
        };
        // **`startOffset` is a field now rather than a report** (§15 D409): the
        // model has somewhere to put "the type starts a third of the way along",
        // which is what the Text tool's click writes, so the reader can keep it.
        //
        // ⚠️ **A percentage only.** SVG also allows a *length*, which is in the
        // rail's own user units — and the field is a fraction, deliberately, so it
        // survives the rail being scaled. Converting would mean measuring the
        // curve's arc length here, which is exactly the work `PathWarp` exists to
        // do and which this reader has no business duplicating; so a length is
        // reported and dropped, which is one of two spellings rather than the whole
        // attribute.
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
        // **The element a run came from, which is not always the `<text>`.** A
        // positioned `<tspan>` is the source of everything about its run — its id, its
        // opacity, and through `run_style` its font — so `text_node` has to be given
        // it rather than the element it hangs under.
        let mut run_el = el;
        // The spans the finished node owes, as `(start, end)` into `pending` —
        // cleared with `pending`, because a range means nothing once the string it
        // indexes has gone.
        //
        // **One entry per attribute rather than a style per range**, because that is
        // the shape `Spans::set` takes and because the four properties are decided
        // independently: a `<tspan>` may state a size and inherit its weight.
        let mut spans: Vec<(usize, usize, crate::typography::CharAttr)> = Vec::new();

        // **Flattened first, at whatever depth the file states them** (§15 D575).
        // The loop below used to walk `body.children()` and read exactly one level,
        // so a `<tspan>` inside a `<tspan>` had its whole style dropped in silence —
        // its *text* arrived, because the collection underneath was already
        // `descendants()`, which is what made the loss invisible. See `text_pieces`.
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
            // **Text that came from the element the run *is* needs no span**, which
            // covers three cases at once: the `<text>`'s own text before any
            // `<tspan>`, a positioned `<tspan>`'s text — it *became* the run at the
            // `Break` above — and, since D575, the text of any tspan nested inside
            // the one that became it.
            //
            // ⚠️ **This used to be `if positioned { continue; }` and the reason is
            // not that the comparisons are with themselves.** The colour and stroke
            // arms are indeed self-comparisons. The **gradient** arm is not a
            // comparison at all, so without this a positioned `<tspan fill="url(#g)">`
            // would report `text (per-run style)` *here* and `text (gradient fill)`
            // again from `text_node`, which sees it as the node it becomes. One
            // gradient, one report, and the node is the honest place for it.
            //
            // Its flip was green for want of a fixture with that input, not for want
            // of a mechanism. **A flip that does not bite is a finding about
            // coverage**, and reading it as a finding about the code is how a live
            // line comes to be documented as decoration. With the fixture written it
            // bites: `["text (per-run style)", "text (gradient fill)"]` against one.
            if child == run_el {
                continue;
            }
            let end = pending.len();
            match solid_fill(&s) {
                Some(c) if ink(&s) != ink(&run_style) => {
                    spans.push((start, end, CharAttr::Color(Some(c))));
                }
                // No colour to carry, and a fill stated on the `<tspan>` itself —
                // so the fill is a gradient, and a `CharAttr` has no room for one.
                // ⚠️ **Stated, not merely resolved**: a gradient the run *inherits*
                // is the run's own paint and nothing is lost by joining it.
                None if property(child, "fill", &self.gradients.css).is_some() => {
                    self.approximate("text (per-run style)");
                }
                _ => {}
            }
            if s.stroke.is_some() != run_style.stroke.is_some() {
                self.approximate("text (per-run style)");
            }
            // **The font scope, which was the silent loss** (§15 D398). Size, weight
            // and italic are plain comparisons; the family is not, because a list is a
            // *preference order* and picking from it needs to know what this machine
            // has — see [`Builder::pick_family`], which is also where the substitution
            // gets reported.
            //
            // ⚠️ **A value equal to the node's own is pushed anyway and dropped by
            // `Spans::normalize`.** Two families that resolve to the same face is the
            // case that makes this worth stating: `"Foo, Arial"` against `"Arial"`
            // differ as lists and agree as spans, and asking the model rather than
            // pre-filtering here is what keeps the two answers from disagreeing.
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

    /// A `<text>`'s content flattened into runs, **at whatever depth the file states
    /// them** (§15 D575).
    ///
    /// `[S7.1-L1-04]`: `fn text` walked `body.children()` and read exactly one level,
    /// so `<tspan><tspan fill="red" font-size="40" font-weight="700">b</tspan></tspan>`
    /// imported with `Spans([])` — no colour, no size, no weight — against three
    /// spans for the identical markup one level flatter. ⚠️ **The *text* arrived**,
    /// because the collection underneath was already `descendants()`, which is
    /// exactly what made the loss silent: the content was right and only the styling
    /// was gone, so nothing looked broken and `skipped`/`approximated` were both
    /// empty. A nested *positioned* tspan was worse — one node where SVG places two.
    ///
    /// The module's own contract is the thing this restores: `fn text`'s doc says
    /// *"nothing on a `<tspan>` is lost silently any more"* and §15 D398 closed *"the
    /// importer's last silent loss"*, neither carving out nesting.
    ///
    /// **One piece per text node rather than one per element**, which is what makes
    /// the depth stop mattering: a piece carries the innermost element the text came
    /// from and that element's fully resolved [`Style`], so the loop above compares
    /// against `run_style` without knowing how deep it was. Interleaving falls out —
    /// `<tspan>x<tspan>y</tspan>z</tspan>` yields three pieces and `z` correctly reads
    /// the *outer* tspan's style, which a per-element walk could not express at all.
    ///
    /// A `Break` is emitted **before** the pieces inside it, so a positioned tspan
    /// starts its node and then contributes its own text to it.
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

    /// The curve a `<textPath>` names, in the referencing element's own user space
    /// (§15 D406).
    ///
    /// **The writer's inverse** — `svg::collect_defs` puts a text node's rail in
    /// `<defs>` as a plain `<path>` and points a `<textPath href>` at it, so this is
    /// the same `id` lookup `<use>` already needs and the same `d` grammar every
    /// shape goes through. `Gradients::ids` is *every* element carrying an id, which
    /// is why nothing new had to be collected for this.
    ///
    /// ⚠️ **The referenced element's own `transform` is ignored, per SVG, and that
    /// is worth stating because it is the opposite of what `<use>` does.** A
    /// `<textPath>` takes the target's *geometry* and places it in its own user
    /// space; a `transform` on the target belongs to the target's own rendering,
    /// which for a path in `<defs>` never happens. Reported rather than applied, so
    /// a file that leans on it is not silently misplaced.
    ///
    /// `None` — with a report — for a missing `href`, a target that is not in the
    /// document, one with no `d`, or a `d` that does not parse. Each of those is a
    /// file that renders as nothing in a browser too; what it must not do here is
    /// come back as *straight* text, which is a plausible-looking wrong answer.
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
            // Finiteness beside emptiness, for the reason the `"path"` arm gives:
            // an infinite coordinate is refused by `apply`, so an unfiltered rail
            // would cost the whole import rather than this one `<textPath>`.
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
    /// **A `font-family` is a preference order**, so taking the head of it regardless
    /// would shape every foreign file in a family the document does not carry. Pulled
    /// out of [`Builder::text_node`] when per-run fonts landed (§15 D398), because a
    /// `<tspan>`'s family needs the identical treatment and a second copy of this is
    /// how the node and its runs would come to substitute differently.
    ///
    /// `None` means nothing in the list is available *and the caller has already been
    /// charged for saying so* — `approximate` dedupes by name, so a file whose every
    /// run names the same missing font reports once.
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

    /// One text node holding `content`, with its **first baseline** at `at`.
    ///
    /// ⚠️ **SVG places text by its baseline and this model places it by its box**,
    /// which is the whole of the arithmetic here and the one thing that cannot be
    /// approximated: the offset between the two is the shaped ascent, so the text has
    /// to be laid out before it can be positioned. `text::layout` does that headlessly
    /// with the bundled family, which is why this can live in core at all.
    ///
    /// `text-anchor` is folded into the same placement rather than stored, because
    /// the model has no anchor — an auto-width box *is* its text, so `middle` is the
    /// box shifted half its width and there is nothing left over.
    ///
    /// `inks` are the per-run colours [`Builder::text`] collected, indexed into the
    /// **untrimmed** `content`. They arrive as ranges rather than as spans because
    /// the trim below is the only thing that knows what it removed, and a range
    /// shifted by anyone else would be a span over the wrong letters.
    ///
    /// ⚠️ **Of the trim's two ends, only the *back* one can bite, and the front is
    /// defensive rather than dead-in-the-obvious-way.** [`push_collapsed`] starts
    /// with `space = out.is_empty()`, so leading whitespace never enters the string
    /// at all and `lead` is `0` for every input this module can build — the
    /// `saturating_sub` is there so that a change to *collapsing* cannot silently
    /// become a change to *which letters are coloured*. The `min` is the half that
    /// runs: a trailing space does reach the string, `trim_end` takes it, and a run
    /// ending on it is left indexing past the content.
    // Eight, and D240's transposition hazard does not apply: every parameter is a
    // distinct type, so a swapped pair does not compile. Two call sites, both in
    // [`Builder::text`].
    //
    // 🚨 This attribute was written **twice**, on consecutive lines, from the first
    // commit of the current `.git` until 2026-09-22 — with every gate green the whole
    // time (§15 D828). Clippy has `duplicated_attributes` for exactly this and it is
    // warn-by-default, so the interesting half is why it never fired: **the lint is
    // silent on any item inside an `impl` block**, inherent or trait, and fires on a
    // free function — measured both ways in an isolated crate. This workspace is
    // mostly `impl` blocks, so that is a blind spot over nearly every item in it, and
    // the sweep that found this one was three hand-written greps rather than a gate.
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
        // **The same four characters [`push_collapsed`] collapses, and it has to
        // be the same set** (§15 D660, `[S7.1-L1-08]`): these two trims and that
        // function are one rule about what counts as ignorable markup, and a
        // `trim()` on Unicode's wider `White_Space` was what made a *leading*
        // ideographic space vanish outright rather than merely become a space.
        //
        // ⚠️ **`lead` is the reason they must not drift**, and it is D396's
        // bookkeeping: it re-bases every span's byte range onto the trimmed
        // string, so a front trim that removes characters `push_collapsed` would
        // have kept moves every run in the node. The argument that the front trim
        // is structurally zero — `push_collapsed` refuses to emit a leading
        // separator — still holds under the narrower set, because narrowing only
        // removes reasons to trim.
        let lead = content.len() - content.trim_start_matches(XML_SPACE).len();
        let content = content.trim_matches(XML_SPACE);
        if content.is_empty() {
            return None;
        }
        // ⚠️ **Every font property comes from `style`, not from `el`.** They inherit,
        // so the resolved style is the only thing that has seen the `<g>` two levels
        // up *and* the `<tspan>` this run started at — reading the element is how the
        // Wikipedia test card's second "TEST" came out at 16pt in a corner.
        // ⚠️ **Through the setters rather than a struct literal**, for the reason the
        // colour below is: `TextStyle::set` clamps a size to `MIN_FONT_SIZE..MAX` and
        // a weight to `1..=1000`, and a *span* of either goes through that clamp
        // whatever this does. A default assigned raw is a default a span can never
        // equal once the file states something out of range, so the span survives
        // `normalize` as an override of a value it is identical to.
        let mut ts = crate::typography::TextStyle::default()
            .with(CharAttr::Size(style.font_size))
            .with(CharAttr::Weight(style.weight))
            .with(CharAttr::Italic(style.italic));
        // `style` is a parameter rather than a borrow of `self`, so this needs no
        // clone to satisfy the borrow checker — it had one, allocating per text node
        // for nothing, and no lint fires on that.
        if let Some(list) = &style.font_family
            && let Some(f) = self.pick_family(list)
        {
            ts.set(CharAttr::Family(f));
        }
        // ⚠️ **Through the setter, not the field**, so `fill-opacity` is quantized to
        // the 1/255 the hex field and the opacity control can express
        // (`typography::canonical_color`). An `f32` alpha of exactly 0.5 is a value
        // *the user cannot reproduce or clear*: the picker can only reach 128/255, so
        // a span set from it would never coalesce with an imported default, and every
        // save would churn a diff saying nothing.
        //
        // ⚠️ **This is not what stops an invisible span, though it reads like it.**
        // Tried it: assigning the field raw leaves every test green, because `ink`
        // has already compared the two runs at `to_rgba8` precision in `text` and
        // never pushed the range. The guard against an invisible span is *there*, one
        // function up; this is about the value that gets stored.
        if let Some(c) = solid_fill(style) {
            ts.set(crate::typography::CharAttr::Color(Some(c)));
        } else if style.fill.is_some() {
            // ⚠️ **A gradient on the text itself, which was dropped in silence until
            // 2026-09-01** — the one loss in this module that nothing said out loud
            // (§15 D396). `TextStyle::color` is a *colour*; a text node cannot be
            // filled with a paint server, so the drawing comes back in the default
            // ink and the report is all the reader gets.
            //
            // **Here rather than in `text`, because this is the only place that sees
            // a whole node's fill** — and a positioned `<tspan>` arrives here as its
            // own node, which is what lets the per-run arm up there skip it instead
            // of reporting the same gradient from two directions.
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
        // **A span covering every byte is a default wearing the wrong hat.** Hoist it
        // (§15 D398), or importing this app's own output leaves a `Weight(500)` sitting
        // over the whole string with a node underneath it saying 400 — which renders
        // identically, so nothing would ever have shown it, and which is *why* it is
        // worth doing rather than a nicety.
        //
        // ⚠️ **Our writer is the case, not an exotic one.** §15 D81 emits one
        // `<tspan>` per style run and puts the font properties **on the tspan**, so a
        // single-run line comes back with every property it states as a full-coverage
        // span. Two runs agreeing also land here: `Spans::normalize` merges touching
        // spans of equal value first, so they arrive as one span over everything.
        //
        // Kinds, not values — `set` on the default and then `clear` over the range,
        // which leaves other attributes' spans alone. The clear is what makes it a
        // hoist rather than a duplicate; `normalize` would drop the span on the next
        // write anyway, and "on the next write" is not a state to ship a node in.
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
        // ⚠️ **On a rail the anchor becomes `align` instead of being folded into the
        // placement**, which is the one place these two readings of `text-anchor`
        // part company (§15 D406). Flat text has a box, so `middle` is the box
        // shifted half its width and nothing is left over — that is what the doc
        // above describes. Railed text has no `x` to shift *from*: the anchor is the
        // whole of where along the curve the type sits, and the model has a field
        // for exactly that.
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
            // **Always `false`, and it is not a gap.** SVG 2's `side="right"` is the
            // only markup that says "the other side", and our own writer does not
            // emit it — a flipped node is written as a *reversed rail*, which is the
            // same drawing in every viewer and comes back here as a reversed rail
            // with no flag, drawing identically. What the round trip loses is which
            // of two spellings the flip was in, not the flip.
            on_path_flip: false,
            on_path_offset: rail_offset,
        });
        // The first glyph's `y` is the baseline, measured down from the layout's own
        // origin — so subtracting it puts the box where the baseline lands on `at`.
        let ascent = layout
            .runs
            .first()
            .and_then(|r| r.glyphs.first())
            // **0.8 of the size is the fallback and not the rule**, reached only when
            // the text shapes to no glyphs at all — which is text made entirely of
            // characters the family has no coverage for. A wrong-but-close baseline
            // beats a layer at the wrong end of the line.
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
            // ⚠️ **The `<text>`'s transform, passed in, not `el`'s.** `el` here may be
            // a `<tspan>`, which cannot carry a transform in SVG — reading it would
            // silently drop the one on the element above.
            //
            // **A rail places the node itself**, so there is nothing to translate by:
            // the curve is in this element's own user space and the node's local
            // space *is* that space. Applying the baseline arithmetic as well would
            // move the type off the curve it was just put on, by an ascent.
            transform: Some(if rail.is_some() {
                outer
            } else {
                outer * Affine::translate((at.0 - shift - layout.origin.x, at.1 - ascent))
            }),
            name: element_name(el),
        });
        self.shapes += 1;
        // The `<text>`'s own chrome as well as this run's — see `chrome`'s `over`.
        // `el` here is a positioned `<tspan>` for every run after the first, exactly
        // as it is for the transform three fields up.
        self.chrome(el, id, style.current, Some(text_el));
        Some(id)
    }

    /// A `<rect>` with its two corner radii already clamped, as whichever of the
    /// model's three shapes holds it *exactly*.
    ///
    /// **SVG's rounded rect has an ellipse at each corner and ours has a circle**, so
    /// one attribute pair spans three of our kinds:
    ///
    /// - `rx == ry` — a [`NodeKind::Rect`] with that radius, which is the common case
    ///   and stays a rect the user can re-round.
    /// - both radii at half their side — the corners have eaten the straight edges
    ///   and the shape *is* an [`NodeKind::Ellipse`]. Still editable, still exact.
    /// - anything else — a [`NodeKind::Path`], because nothing else is exact.
    ///
    /// ⚠️ **This used to read the pair as `min(rx, ry)` and call the difference a
    /// fidelity nobody asked for.** Somebody asked: `rx="7.5" ry="65"` on a 15×130
    /// rect is Inkscape's spelling of an ellipse, and the old reading drew a stadium —
    /// straight-sided where the drawing had a point. The lesson is that the *large*
    /// disagreements live at the clamp, not near it: taking the smaller radius is a
    /// small error while `rx` and `ry` are both small, and turns into a different
    /// shape exactly when one of them reaches half its side.
    ///
    /// Losing the rect-ness in the third case is the price, and it is only paid where
    /// the alternative is drawing the wrong outline.
    fn rounded_rect(
        &mut self,
        el: roxmltree::Node<'_, 'input>,
        parent: NodeId,
        index: usize,
        // The box in the element's own user space — `x`/`y` and `width`/`height` in
        // one, which is also what keeps this under clippy's argument count.
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
        // The quarter-ellipse's cubic constant — the same 0.5522847 every renderer
        // uses to approximate an arc, and accurate to about two parts in ten
        // thousand of the radius.
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
        // **The element's own transform, then the shape's placement inside it** —
        // `transform="rotate(45)"` on a `<rect x="10">` turns the rect about the
        // *user* origin and not about the rect's corner, which is what this order
        // says and what the opposite order would get wrong.
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

        // ⚠️ **An elliptical radial gradient was reported here until 2026-09-03 and
        // is now exact** (§15 D412). The report was right about the model at the
        // time and wrong about why: `peniko::RadialGradientPosition` is two circles,
        // but an ellipse *is* a circle under a squash, and the model's brush carries
        // one now ([`crate::image::GradientBrush`]). [`Paint::brush`] bakes a
        // mapping it can express and hands the rest to that field, so nothing on
        // this path is approximated any more and there is nothing left to say.

        let fills: Vec<Fill> = style
            .fill
            .as_ref()
            .map(|p| Fill {
                brush: p.brush(bounds, local, style.fill_opacity),
                visible: true,
            })
            .into_iter()
            .collect();
        if !fills.is_empty() {
            self.push(Operation::SetFills { id, fills });
        }
        let strokes: Vec<Stroke> = style
            .stroke
            .as_ref()
            .filter(|_| style.stroke_width > 0.0)
            .map(|p| Stroke {
                brush: p.brush(bounds, local, style.stroke_opacity),
                width: style.stroke_width,
                join: style.stroke_join,
                cap: style.stroke_cap,
                dashes: style.dashes.clone(),
                dash_offset: style.dash_offset,
                // **`stroke-miterlimit` is read; `dash_fit` cannot be.** The first is
                // an SVG property with the same meaning and the same default of 4;
                // the second is ours — a rule about how a pattern is *fitted* to an
                // outline, which the markup has already resolved into the numbers it
                // wrote. Importing it as `true` would re-fit a pattern that is
                // already fitted.
                miter_limit: style.miter_limit,
                dash_fit: false,
                // **SVG has no stroke alignment**: its stroke is always centred on
                // the outline, so anything else would be inventing a value the file
                // does not carry.
                align: StrokeAlign::Center,
                sides: StrokeSides::All,
                visible: true,
            })
            .into_iter()
            .collect();
        if !strokes.is_empty() {
            self.push(Operation::SetStrokes { id, strokes });
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
    /// **`display="none"` becomes a hidden layer rather than nothing.** The markup
    /// says the shape exists and is not shown, and the model has exactly that state —
    /// dropping it would lose content a user can turn back on with one click.
    ///
    /// ⚠️ **`over` is the element `el` hangs *under*, when the two are different
    /// elements making one node** (§15 D573). It exists for exactly one caller:
    /// `text_node`, whose `el` is a positioned `<tspan>` for every run after the
    /// first, and a `<tspan>` carries none of these three properties.
    /// `[S7.1-L1-05]`: a `<text opacity="0.3" display="none">A<tspan x=… >B</tspan>
    /// </text>` imported as two nodes of which only the **first** was faded and
    /// hidden — so a diagram pasted from a file that hides a layer arrived with that
    /// layer half on screen, and `skipped` was empty.
    ///
    /// **Not "read the `<text>` instead"**, which is why this composes rather than
    /// replaces: `opacity` on a `<tspan>` is legal and multiplies with its parent's,
    /// the same rule SVG gives for nested groups. `display` and `visibility` are the
    /// degenerate case of the same composition — either one hides.
    ///
    /// This is the treatment `text_node`'s `outer` transform already had, arrived at
    /// separately and for the same reason; the three chrome properties were not moved
    /// with it.
    fn chrome(
        &mut self,
        el: roxmltree::Node<'_, 'input>,
        id: NodeId,
        current: Color,
        over: Option<roxmltree::Node<'_, 'input>>,
    ) {
        // **Both read before either is written**: the reads borrow `self.gradients`
        // and the writes borrow `self.ops`, and interleaving them does not compile.
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
        // A product, because that is what nesting means: a 50% run inside a 50%
        // `<text>` is 25%, and `unwrap_or(1.0)` on each half makes "stated on neither"
        // and "stated on both as 1" the same answer.
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
        // ⚠️ **`filter` is read as a *property*, not as an attribute**, and that was a
        // reporting bug of its own before it was a rendering one: Inkscape writes the
        // whole presentation block into `style="…;filter:url(#f)"`, so `has_attribute`
        // saw nothing and the paste did not even say the filters had been dropped. A
        // silent loss is the one thing [`Import::skipped`] exists to prevent, so the
        // spelling this reads matters as much as what it does with the answer.
        // **The run's own if it states one, the outer element's otherwise** — the one
        // property of the three that does not compose, because two filters are a
        // graph and the model holds one effect stack per node. A `<tspan filter=…>`
        // is legal and rare; a `<text filter=…>` is ordinary, and before D573 it
        // reached the first run only.
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
            // The whole of what the model can hold, and it composites on a container
            // exactly as it does on a shape — which is why this belongs in `chrome`,
            // beside the other two properties every element carries and neither
            // inherits, rather than in `shape`.
            Some((_, Some(read))) => {
                if read.uneven {
                    self.approximate("uneven feGaussianBlur (averaged)");
                }
                // An empty stack is a filter that resolved to "unchanged" — the bare
                // `<feOffset/>` — so writing it would put an empty effects list on a
                // node that has one already. Reading it as nothing is the point.
                if !read.effects.is_empty() {
                    self.push(Operation::SetEffects {
                        id,
                        effects: read.effects,
                    });
                }
            }
            // Everything else a `<filter>` can be — a graph this cannot invert, a
            // reference to nothing, `objectBoundingBox` primitive units.
            Some(_) => self.skip("filter"),
        }
    }
}

impl Paint {
    /// The affine that carries this paint's gradient from the coordinates it was
    /// written in to the shape's own space. Identity for a solid.
    ///
    /// **Extracted so the report and the brush cannot disagree** — the composition is
    /// the fiddly part (see [`Paint::brush`]) and computing it twice is how a warning
    /// comes to be about a transform nobody applied.
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

    /// Whether this is a **radial** gradient whose mapping is not a similarity — i.e.
    /// one whose ellipse has to ride on [`crate::image::GradientBrush::transform`]
    /// rather than being baked into the two circles.
    ///
    /// The test is the ratio of the mapping's two singular values, which is the right
    /// question rather than "does the matrix look diagonal": a rotation has a wildly
    /// uneven diagonal and is *perfectly* representable, and that confusion is exactly
    /// the bug [`transform_gradient`] carries a warning about.
    ///
    /// ⚠️ **The threshold is numerical noise, not a visibility judgement, and it was
    /// `1.05` until 2026-09-03** (§15 D412). Five percent was the right deadband while
    /// this predicate chose between *approximating and saying so* and *approximating
    /// silently* — under it nobody could see the error, and reporting one on every
    /// ordinary file would have been noise. It is the wrong deadband now that the
    /// answer is between **exact** and *approximating silently*: there is no longer
    /// anything to trade, and a 4% squash was the one case left that was wrong with
    /// nothing saying so. Dropping it is what makes the module head's "nothing on this
    /// path is approximated" true rather than nearly true. ⚠️ **`1e-6` rather than
    /// `0.0` is insurance and measurably not a working part** — a similarity's two
    /// singular values come out bit-identical, so the ratio is exactly `1.0`; see the
    /// flip recorded on
    /// `a_squash_under_the_old_deadband_is_carried_and_a_similarity_is_not`.
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
    /// **Both `gradientUnits` need a mapping, and only one of them is about the box.**
    /// `local` is the shape's placement inside its element's user space — a rect's
    /// `x`/`y`, identity for a path — and it is what separates the two: our brush is
    /// written in the shape's *own* space, starting at its origin, while a
    /// `userSpaceOnUse` gradient is written in the space that placement was measured
    /// in. Leaving it alone was the same bug as never mapping the unit square, one
    /// coordinate system further out.
    fn brush(&self, bounds: kurbo::Rect, local: Affine, alpha: f32) -> Brush {
        match self {
            Paint::Solid(c) => Brush::Solid(c.multiply_alpha(alpha)),
            Paint::Gradient(g, _, _) => {
                // `(**g)` rather than `g.clone()`: the binding is a `&Box<_>`, so the
                // short spelling clones the box and leaves the rest of this arm
                // writing through it into a value it then has to unbox again.
                let mut g = (**g).clone();
                // [`Paint::mapping`] is the whole of the two `gradientUnits` and the
                // `gradientTransform` inside them — **the unit square onto the shape's
                // own box** for `objectBoundingBox`, which is SVG's default and so the
                // common path rather than the exotic one, and the inverse of the
                // placement for `userSpaceOnUse`.
                let m = self.mapping(bounds, local);
                // **Baked where baking is exact, carried where it is not**, which is
                // the whole of why the brush's transform is usually the identity. A
                // linear gradient's two points take any affine; a radial's two
                // circles take a similarity and no more. So only a *squash* survives
                // as a transform, and the geometry stays in the shape's own space
                // for everything else — which is the space the paint panel reads and
                // writes stops in.
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
                    // **Baked into the stops above, not carried here** (§15 D767).
                    // SVG's own `opacity` on a gradient-filled shape is a *shape*
                    // property rather than a paint one, and the loop above has
                    // already multiplied it through — so importing it into the new
                    // field as well would apply it twice.
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
    // A radius has no direction, so it scales by the transform's average scale.
    //
    // ⚠️ **This arm only ever sees a *similarity* now, so the average is exact**
    // (§15 D412). The sentence here used to end "an ellipse-shaped radial gradient
    // is not something the model can hold", which stopped being true the day
    // [`crate::image::GradientBrush`] gained a transform: [`Paint::brush`] routes a
    // non-similarity radial mapping to that field instead of here, so what reaches
    // this line is a rotation and a uniform scale, where "the average scale" is
    // simply *the* scale. The averaging below is kept for the linear and sweep
    // arms and for numerical noise, not as an approximation of a squash.
    //
    // ⚠️ **`sqrt(|determinant|)`, not the average of the diagonal.** The diagonal
    // reading is only the scale when the transform has no rotation in it: for
    // `matrix(0, -1.33, -1.33, 0, …)` — a quarter turn at 4/3 scale, which is how
    // Inkscape writes a highlight that has been rotated into a corner — `a` and `d`
    // are both **zero**, so the old reading collapsed the radius to nothing and the
    // gradient painted its last stop over the whole shape. The four corner highlights
    // on the camera that sent me here were white-to-transparent, so *nothing* was
    // exactly the wrong answer to be invisible: the corners simply went dark.
    //
    // The determinant is the area scale, so its square root is the geometric mean of
    // the two axes — exact for any rotation with a uniform scale, which is the case
    // that has a right answer, and area-preserving for the squashed case that does
    // not (see [`Builder::skip`]'s companion, [`Import::approximated`]).
    let s = t.determinant().abs().sqrt() as f32;
    match kind {
        peniko::GradientKind::Linear(l) => {
            // ⚠️ **Mapping the two endpoints is not the same as mapping the
            // gradient**, and the difference is the whole angle of the bands.
            //
            // A linear gradient's iso-lines are perpendicular to `start → end` **in
            // the space the gradient was written in**. A transform carries the lines
            // and the vector separately, and only a *similarity* keeps them at right
            // angles — so under `matrix(2.33, 0, 0, 0.51, …)`, or under the
            // `objectBoundingBox` map of any rect that is not square, the naive
            // endpoints tilt the bands. The flash window on the camera that sent me
            // here has bands 0.4° off horizontal in the browser and came out at 8°.
            //
            // The exact answer is still a linear gradient, because an affine of one
            // is one. What has to be transformed is the *covector*: the gradient
            // parameter at a point is `(q − T·start) · u`, where
            // `u = Tₗᵢₙ⁻ᵀ·d / |d|²` — and peniko reads `u` back as the vector
            // `u / |u|²` laid off from the mapped start.
            let d = l.end - l.start;
            let len2 = d.hypot2();
            let c = t.as_coeffs();
            let inv = Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
                .inverse()
                .as_coeffs();
            // The inverse *transpose* — the two off-diagonal entries swap, which is
            // the whole of what makes this a covector rather than a vector.
            let u =
                kurbo::Vec2::new(inv[0] * d.x + inv[1] * d.y, inv[2] * d.x + inv[3] * d.y) / len2;
            let start = p(l.start);
            // A degenerate gradient — zero length, or a singular transform — has no
            // direction to correct, and the endpoint mapping is as good as anything.
            // ⚠️ **The transform clause is `affine_is_invertible` now** (§15 D762).
            // It was `t.determinant().abs() > 1e-12`, which is the same question
            // this file's other guard asked as `== 0.0` — twelve orders of
            // magnitude apart, thirteen hundred lines apart, for one rule. The two
            // length clauses stay: they are about the *gradient* being degenerate,
            // not about the transform, and `affine_is_invertible` answers neither.
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
/// **SVG's gradient inheritance is per *attribute*, and it is not an edge case.** A
/// gradient that states no `x1` takes the referenced one's; one with no `<stop>`
/// children takes its stops; `gradientUnits`, `gradientTransform` and the rest follow
/// the same rule. Inkscape writes *every* gradient this way — one stop-holding
/// element in `<defs>` and one positioned element per use, joined by `xlink:href` —
/// so a file that looks like it has nine gradients has a hundred and seventy-five,
/// and reading only the element itself finds stops on nine of them.
///
/// ⚠️ **A missing link is not a missing gradient.** Reading the chain nearest-first
/// and stopping at whatever resolves means a broken `href` costs only the attributes
/// that were behind it, which is what a browser does; refusing the whole gradient
/// would put the failure back where it was.
///
/// The depth cap is what stops a cycle — `a` referencing `b` referencing `a` is
/// well-formed XML — and eight is far past anything a real file writes.
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
/// **Collected in one pass rather than looked up by walking**, because a `fill`
/// may reference a gradient defined after it — `<defs>` at the end of a file is
/// ordinary — and because `xlink:href` inheritance between gradients means a
/// lookup can need a second one.
struct Gradients<'a, 'input> {
    // Keyed on `&'a str`, the *document borrow*, rather than on the input string:
    // `roxmltree::Node::attribute` hands back the former, and an id read off a node
    // cannot outlive the tree it was read from.
    by_id: rustc_hash::FxHashMap<&'a str, (peniko::Gradient, GradientUnits, Affine)>,
    /// `<clipPath>` and `<mask>` elements by id, kept as *nodes* rather than as
    /// anything resolved: what they contain is ordinary markup, so the walk that
    /// builds a shape is the walk that builds a clip, and pre-digesting them would
    /// be a second one.
    clips: rustc_hash::FxHashMap<&'a str, roxmltree::Node<'a, 'input>>,
    /// **Every** element carrying an id, which is what `<use href="#x">` needs and
    /// what the two maps above are special cases of. Kept separate rather than
    /// merged, because a lookup that could return a gradient where a clip belongs is
    /// a lookup that has to be checked at every use.
    ids: rustc_hash::FxHashMap<&'a str, roxmltree::Node<'a, 'input>>,
    /// The document's `<style>` rules — see [`Css`].
    css: Css<'a>,
    /// Whether the document holds a `<pattern>` — a paint server this reader has
    /// no support for at all (§15 D477).
    ///
    /// ⚠️ **This app's own SVG writer emits every image fill as one**, so a file
    /// exported and pasted back lost its pictures — and lost them **silently**,
    /// which is the one thing this module's header forbids: *"nothing is lost
    /// silently — a paste that quietly loses half a drawing is worse than one that
    /// refuses."* `[A5-L6-01]` measured an 80×40 rect with a picture on it coming
    /// back as an **opaque black rectangle** with 0 images in the document, on
    /// neither report list. The black is SVG's own doing: `fill="url(#img-…)"`
    /// resolving to nothing leaves the inherited fill, and the initial value of
    /// `fill` is black.
    ///
    /// **A flag reported once for the file**, which is the choice `css.complex`
    /// and [`Self::resampled`] already made and for the same reason: *"some
    /// pictures could not be read"* is a fact a reader can act on, where naming
    /// nine ids is a symptom they would have to trace back to it.
    ///
    /// ⚠️ **This is the report, not the feature.** Reading a `<pattern>` back into
    /// an image fill is a new paint-server kind in this reader and is *not* done —
    /// `Gradients::collect` keys `by_id` on `linearGradient`/`radialGradient` and
    /// `clips` on `clipPath`/`mask`, and `parse_paint` has no arm for a pattern.
    /// The round trip is still lossy; it is no longer quiet about it.
    patterns: bool,
    /// Whether any `fill` or `stroke` carried a value this reader could not parse
    /// (§15 D493).
    ///
    /// **A `Cell` because `resolve` sees this by shared reference**, and it is the
    /// only field here set outside `collect`: the two flags above are facts about
    /// the *defs*, which are walked once up front, and this is a fact about the
    /// elements, which are walked with the gradient table already borrowed. A
    /// second walk to find out would be a second cascade.
    ///
    /// A flag rather than a list of values, on the same argument the two above
    /// make: *"some paints could not be read"* is a fact a reader can act on, where
    /// naming nine CSS values is a symptom they would have to trace back to it.
    unreadable_paint: std::cell::Cell<bool>,
    /// Whether any gradient's stop list was resampled down to
    /// [`MAX_GRADIENT_STOPS`] (§15 D459).
    ///
    /// **A flag rather than a list of ids, and reported once**, which is the same
    /// choice [`Css::complex`] made and for the same reason: *"some gradients had
    /// more stops than we keep"* is a fact about the file that a reader can act
    /// on, where naming nine ids is a symptom they would have to trace back to it.
    resampled: bool,
}

/// The most stops this reader keeps from one `<linearGradient>`/`<radialGradient>`
/// (§15 D459).
///
/// **The bound on a gradient's *payload*, which is a third quantity beside the
/// two the importer already bounds** — `roxmltree`'s `nodes_limit` on the XML and
/// D446's `MAX_SVG_NODES` on what is emitted. Neither sees this: `[S7.2-L5-02]`'s
/// 452 KB fixture uses 2.4% of the XML budget and holds **864 MB** of stops,
/// because every shape naming a gradient gets its own copy of the whole list and
/// the copies end up in the `Document` and then in the saved JSON.
///
/// **256 is far above anything real and far below anything dangerous.** Gradients
/// in the wild are single digits; an Illustrator mesh export reaches dozens; this
/// app's own writer emits what the paint panel holds. At the cap a gradient has a
/// stop every 0.4% of its length, which is below what any renderer resolves — so
/// a file that hits this is a generated one, and resampling it is the honest
/// answer rather than a loss.
const MAX_GRADIENT_STOPS: usize = 256;

/// Every `<style>` element's full text content, keyed by the element.
///
/// **Owned, and built by the caller rather than inside [`Gradients::collect`],
/// because the rules borrow it** (§15 D544). A sheet interrupted by an XML
/// comment is two text nodes with no single `&str` in the document to point at,
/// so somebody has to own the join — and `Css` is returned inside the `Builder`,
/// so it cannot be a local of `collect`. The caller declares it above the
/// `Builder` it hands it to, which is what makes the borrow outlive it.
///
/// Keyed by [`roxmltree::Node::id`] rather than positional, so this and the walk
/// in `collect` cannot drift into disagreeing about which sheet is which.
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
                // ⚠️ **Every text child, not `text()`** (§15 D544). `text()` is the
                // first child *and only when that child is a text node* — so an XML
                // **comment** anywhere inside `<style>` ended the sheet there, and a
                // comment in the first position (or a whitespace text node and then
                // a comment) yielded nothing parseable at all. `[S7.2-L1-03]`,
                // measured against Chrome reading the same markup:
                //
                // | `<style>` content | was | Chrome |
                // | --- | --- | --- |
                // | `.a{…red}<!--c-->.b{…green}` | `.b` **black** | `.b` green |
                // | `<!--c-->.a{…red}` | `.a` **black** | `.a` red |
                //
                // **Black is not "no fill"** — it is SVG's initial `fill` — so the
                // loss is invisible: the shape is there, painted the wrong colour,
                // and `skipped` and `approximated` are **both empty**. The second
                // row is the damaging one, because `<style><!-- … --></style>` is
                // the legacy idiom and any leading comment sends every classed
                // shape in the drawing to black at once.
                //
                // The comment this replaces named the assumption correctly and
                // then missed the case: *"a `<style>` holding elements is not a
                // stylesheet"* — true, and a comment is not an element.
                // **CDATA was never affected**, because roxmltree merges text and
                // CDATA nodes, and that is the control that pins the cause on
                // comments rather than on interleaving in general.
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
                // Same rule, and it was a plain `insert` until §15 D642 — so a
                // duplicate `<clipPath id>` resolved to the last one here and to
                // the first one through `ids`.
                clips.entry(id).or_insert(el);
            }
            // **First wins**, which is what a browser does with a duplicate id and
            // is the only answer that does not depend on document order twice.
            //
            // ⚠️ **Stated here and broken in the two maps beside it for as long
            // as they existed** (§15 D642). This comment is three lines from
            // `clips` and two loops from `by_id`, and neither obeyed it; a rule
            // written next to one of its three implementations is not a rule the
            // other two are held to by anything.
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
            // **Everything below reads the *chain*, not the element.** See
            // [`gradient_chain`]: a gradient inherits, per attribute and per stop
            // list, from whatever it references.
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
            // **SVG's own rule about a descending offset, applied here so the
            // model holds what the file *means*** (§15 D455): *"each gradient
            // offset value is required to be equal to or greater than the
            // previous"*, and a smaller one is corrected up. `[S11.1-L1-02]` /
            // `[S7.2-L1-…]`: `<stop offset="0"/><stop offset="0.8"/><stop
            // offset="0.2"/><stop offset="1"/>` was stored as `[0, 0.8, 0.2, 1]`
            // and drew a **flat fill**.
            //
            // ⚠️ **Also done at the render boundary, and this is not that guard
            // twice.** `color::brush_to_backend` is what makes a *hand-edited*
            // `.ondin` and every file imported before today safe; this is what
            // makes the imported model say what the source said, so a re-export
            // round-trips and the layers panel shows the right ramp. Neither
            // covers the other.
            crate::image::make_stops_monotonic(&mut stops);
            // ⚠️ **The stop list is capped, because a gradient's payload is the one
            // thing `nodes_limit` does not bound** (§15 D459). `[S7.2-L5-02]`: every
            // shape naming a gradient gets its **own** copy of the whole list — and
            // not a transient one, since the copies end up in the `Vec<Operation>`
            // the paste applies, then in the `Document`, then in the JSON `io::save`
            // writes. `[S7.2-L5-02]`'s own measurement, through `import` in
            // release, *S* stops and *S* rects:
            //
            // | S | source | import | solid-fill control | stop bytes held |
            // | --- | --- | --- | --- | --- |
            // | 1 000 | 66 KB | 9.4 ms | 2.0 ms | 24 MB |
            // | 2 000 | 134 KB | 35.0 ms | 4.1 ms | 96 MB |
            // | 4 000 | 271 KB | 133 ms | 10.6 ms | 384 MB |
            // | 6 000 | 452 KB | **288 ms** | 21.6 ms | **864 MB** |
            //
            // ⚠️ **`a_gradients_stop_list_is_bounded_and_says_so`'s before-and-after
            // table is a *different file* at the same S**, and the source column is
            // what says so: 104 / 210 / 421 / 633 KB against 66 / 134 / 271 / 452,
            // because that fixture gives each rect an `x` attribute so the shapes do
            // not all coincide. The timings and the byte counts agree to a few
            // percent, which is what makes the two tables comparable — but they are
            // two files and neither comment said so until now.
            //
            // Exactly quadratic — ×4 per doubling against the control's ×2 — and
            // `size_of::<peniko::ColorStop>()` is 24, so the byte column is
            // `S × K × 24` rather than an estimate.
            //
            // ⚠️ **`nodes_limit` reads as a belt here and is not one.** It bounds
            // the *XML* nodes at 500 000; 6 000 stops plus 6 000 rects is 12 001
            // elements, so the 452 KB file above is using 2.4% of its budget and
            // the limit as written permits 250 000 × 250 000. D446's
            // `MAX_SVG_NODES` bounds the emitted side and not this one either.
            // **The payload per node is a third quantity and this is its only
            // bound.**
            //
            // **Resampled rather than truncated or refused.** Truncating keeps the
            // first `MAX_GRADIENT_STOPS` and compresses the whole ramp into the
            // front of its range — a visibly different drawing; refusing loses the
            // paint entirely. Taking an even spread keeps the ramp's shape, and at
            // this cap the difference is below what any renderer resolves: 256
            // stops across a gradient is one every 0.4% of its length.
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
                    // `fx`/`fy` — the focal point — default to the centre, which is
                    // the concentric case the model draws.
                    start_center: peniko::kurbo::Point::new(num("fx", cx), num("fy", cy)),
                    start_radius: 0.0,
                    end_center: peniko::kurbo::Point::new(cx, cy),
                    end_radius: r as f32,
                })
            };
            let mut g = peniko::Gradient::new_linear((0.0, 0.0), (0.0, 0.0));
            g.kind = kind;
            // **`spreadMethod` is `extend`, and it is not decoration.** A *reflected*
            // gradient is how every brushed-metal sheen in a drawing is written — one
            // ramp, mirrored, so the highlight has two edges — and padding it instead
            // leaves the whole outer half flat at the last stop, which reads as a
            // white band rather than a curve. Ondin's model has held the field all
            // along (it is `peniko::Gradient::extend`, so it saves and renders); only
            // the reader was dropping it.
            g.extend = match attr("spreadMethod") {
                Some("reflect") => peniko::Extend::Reflect,
                Some("repeat") => peniko::Extend::Repeat,
                _ => peniko::Extend::Pad,
            };
            g.stops = stops.as_slice().into();
            // ⚠️ **`or_insert`, because the rule two loops up applies here too**
            // (§15 D642, `[S7.2-L1-05]`). `ids` is written with
            // `entry().or_insert()` under a comment stating *"first wins, which
            // is what a browser does with a duplicate id"*, and this was a plain
            // `insert` — so one document had **two** answers for `#g`. Reading
            // `ids`, and so unaffected: `Gradients::element` (the `<use href>`
            // resolver), `Gradients::filter`, and `gradient_chain`'s `href`
            // walk. Reading the two plain-`insert` maps, and so last-wins:
            // `Gradients::get`, which is the fill and stroke path and by far the
            // common one, and `Gradients::clip`, which is `clips` and covers
            // `<mask>` as well as `<clipPath>`. Measured against Chrome on two
            // `<linearGradient id="g">`, red then blue, on one rect: Chrome
            // paints red and Ondin painted blue.
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

    /// Resolve a `<use>`'s `href`, which is a **bare** `#id` rather than a `url(#id)`
    /// — the one reference in SVG that is not wrapped, and the reason
    /// [`reference()`] is not used here.
    ///
    /// ⚠️ **The `()` is a disambiguator, not decoration.** Written `[`reference`]` this
    /// is ambiguous between the private helper below and Rust's own `reference`
    /// *primitive type*, and rustdoc refuses to choose. It sat here unnoticed because
    /// without `--document-private-items` the helper is invisible and the link
    /// silently resolved to the primitive — the gate hole `CLAUDE.md` records as the
    /// fifth, caught by the flag the same day it was added.
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
    /// **Off [`Self::ids`] rather than a map of its own**, unlike [`Self::clip`]: a
    /// filter is looked up once per element that has one and a clip is looked up
    /// during a walk, so the second map would buy nothing. The tag check is what
    /// stops `filter="url(#some-gradient)"` from resolving.
    fn filter(&self, value: &str) -> Option<roxmltree::Node<'a, 'input>> {
        let el = *self.ids.get(reference(value)?)?;
        el.has_tag_name("filter").then_some(el)
    }
}

/// The filter shapes this model can hold, read out of a `<filter>` element.
///
/// **A `<filter>` is a whole image-processing graph and an effect stack is four
/// kinds**, so most arrangements of it are still a [`Builder::skip`]. What is read
/// is the *shadow idiom* and the two lone primitives, which between them cover
/// every filter this app writes but one, and the two other authoring tools produce:
///
/// - a lone `<feGaussianBlur>`, which is what Inkscape's *Blur* slider writes — a
///   drawing with fifty-six filters in it can have fifty-six of this one, and the
///   car drawing that sent me here does;
/// - a lone `<feDropShadow>`, one primitive that says the whole thing, which is what
///   a modern tool writes and what a hand-authored file most often carries;
/// - the **shadow chain** — a silhouette, an optional inversion, an optional
///   dilation, an offset, a blur, then a flood cut to the silhouette — which is what
///   this app's own writer emits per shadow (`ondin_export::svg::shadow_graph`) and
///   what every pre-`feDropShadow` file does by hand.
///
/// ⚠️ **The one thing this app writes and cannot read back is
/// [`crate::effect::EffectKind::Filters`]**, and it is unreadable rather than
/// unwritten: the writer emits it as a 20-value `feColorMatrix`, and brightness and
/// contrast are *both* a scale on the same channels — their product is one matrix
/// that any number of pairs produce. So the matrix does not determine the four
/// numbers that made it, and a reader that guessed would round-trip a value the user
/// never typed. Such a stack is skipped whole and reported.
///
/// ⚠️ **All or nothing per `<filter>`.** One unrecognised primitive refuses the
/// element rather than reading the rest, which is
/// `a_filter_this_cannot_hold_is_skipped_rather_than_half_read`'s rule and is why
/// the paragraph above costs a whole stack rather than one row: a half-read filter
/// is a drawing that is wrong in a way nothing reports.
struct FilterRead {
    /// In the order the markup lays them out, which is the order the writer's own
    /// stack was in — stage one emits in list order and the shadow chains follow in
    /// list order, so document order *is* stack order and nothing has to be sorted.
    effects: Vec<crate::effect::Effect>,
    /// Some `stdDeviation` in it named two unequal numbers. The model's blur is
    /// isotropic, so the mean is the best available answer and the caller reports it.
    uneven: bool,
}

/// ⚠️ **The filter *region* is not modelled and is deliberately not reported.**
///
/// Every `<filter>` clips its own output to a region — `-10% -10% 120% 120%` of the
/// bounding box by default — and [`crate::effect::EffectKind::LayerBlur`] has no
/// region at all. That
/// is a real difference, and reporting it would still be wrong.
///
/// **The measurement, on the car drawing's 22 explicit regions**: a first pass
/// compared each against the default and called **14** of them tighter, which is what
/// a reader would have been told. Not one is a clip. Inkscape fits the region to the
/// blur *per axis*, so a wide flat shape gets a smaller fraction in `x` and a larger
/// one in `y` for the same absolute reach — the fractions are not comparable across
/// axes, and comparing them is what the false positives are. What it fits to is
/// 2.4 σ; [`crate::effect::BLUR_CUTOFF`] reaches 3 σ, so the model paints an extra
/// 0.6 σ of gaussian tail carrying about 1.6% of the kernel.
///
/// So the honest report is silence, and the reason it is safe is that a filter region
/// is nearly always *generated to fit the blur* rather than authored to cut. Judging
/// the rare authored one needs the shape's bounds, which `chrome` does not have —
/// it runs for a `<g>` as readily as for a shape. **Named here rather than left
/// unsaid**, because a check that was written, measured and removed is worth more to
/// the next reader than one that was never considered.
const _FILTER_REGION_IS_NOT_MODELLED: () = ();

/// A `<filter>` as an effect stack, or `None` for anything this cannot hold.
///
/// **A linear walk, because the markup is one.** A filter graph is a DAG in
/// principle, wired by `result` and `in` names, and reading it as a graph would mean
/// a topological sort and a shape-match over the result. Every producer of the idioms
/// above — this app's writer included — emits the primitives in the order they
/// compose, so walking them in document order recognises the same set with none of
/// that machinery. ⚠️ **The cost is a file that names its results out of order**,
/// which is legal and which this refuses rather than misreads.
///
/// `current` is the **referencing** element's own `color`, threaded here for
/// `flood_color`'s `currentColor` — a `<filter>` lives in `<defs>` and has no
/// inherited paint of its own, so the value has to come from the element that
/// pointed at it (§15 D572).
fn effects_of<'a>(
    def: roxmltree::Node<'a, '_>,
    css: &Css<'a>,
    current: Color,
) -> Option<FilterRead> {
    // ⚠️ **`primitiveUnits="objectBoundingBox"` makes `stdDeviation` a *fraction*,**
    // not a length — so reading it as one turns a 0.5 into half a pixel where the file
    // meant half the shape. Refusing is the only honest answer without the bounds,
    // and it is rare enough that nothing is lost by it.
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
            // One primitive saying the whole thing. It has no spread — the SVG
            // primitive simply does not offer one — so the model's is zero rather
            // than approximated, and nothing is lost to report.
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
            // Stage one: the layer's own appearance. A blur that belonged to a
            // shadow was consumed by `shadow_chain` above, so anything reaching here
            // blurs the drawing itself.
            let (dev, uneven) = deviation_of(p.attribute("stdDeviation")?)?;
            out.uneven |= uneven;
            // **A zero deviation disables the primitive**, which SVG states and
            // which makes this the identity — so it contributes no effect and is
            // *not* a skip, exactly as the bare `<feOffset/>` below is not (§15
            // D572). Reporting `skipped: ["filter"]` here was a spurious report: the
            // picture Ondin drew was already the right one.
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
            // ⚠️ **The identity primitive, and it must be read rather than refused.**
            // A `<filter>` with no primitives renders its element as *nothing*, so
            // this app's writer emits a bare `<feOffset/>` for a stack whose every
            // entry was invisible or neutral (`ondin_export::svg::effect_def`). It
            // means "unchanged", which is an empty stack and not an unreadable one.
            i += 1;
        } else if p.has_tag_name("feMerge") {
            // Assembly, not an effect: every primitive it names has been read
            // already, and the order it puts them in is the order they were met.
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

/// Whether `p` begins a shadow chain rather than acting on the layer itself.
///
/// **Two spellings of the same idea**, and a chain has to start with one of them:
/// the silhouette is either extracted by a `feColorMatrix` that keeps alpha and
/// nothing else — which is what this app's writer emits, because the stack may
/// already have changed what the layer looks like and `SourceAlpha` would ignore
/// that — or taken straight from `SourceAlpha`, which is what a hand-authored file
/// does.
fn starts_shadow(p: roxmltree::Node<'_, '_>) -> bool {
    p.attribute("in") == Some("SourceAlpha") || is_alpha_matrix(p)
}

/// The 20-value `feColorMatrix` that keeps alpha and discards colour.
fn is_alpha_matrix(p: roxmltree::Node<'_, '_>) -> bool {
    if !p.has_tag_name("feColorMatrix") || p.attribute("type").unwrap_or("matrix") != "matrix" {
        return false;
    }
    let v = numbers(p.attribute("values").unwrap_or_default());
    // Row four takes the source's alpha and the other fifteen cells are zero, so the
    // result is an opaque black stencil of the layer.
    v.len() == 20
        && v.iter()
            .enumerate()
            .all(|(n, c)| *c == if n == 18 { 1.0 } else { 0.0 })
}

/// One shadow, read from `prims[start..]`. Returns it, whether it is an *inner*
/// shadow, and the index after the last primitive it consumed.
///
/// ⚠️ **The offset and the blur are accepted in either order, and that is not
/// laxity.** Blurring an offset silhouette and offsetting a blurred one are the same
/// picture; this app's writer offsets first and the pre-`feDropShadow` idiom every
/// hand-written file uses blurs first (`feGaussianBlur in="SourceAlpha"`, then
/// `feOffset`). Insisting on one order would read half the files that mean this.
fn shadow_chain<'a>(
    prims: &[roxmltree::Node<'a, '_>],
    start: usize,
    css: &Css<'a>,
    current: Color,
    uneven: &mut bool,
) -> Option<(crate::effect::Shadow, bool, usize)> {
    let mut i = start;
    // The silhouette, when it is a primitive of its own rather than a `SourceAlpha`
    // the next primitive reads directly.
    if is_alpha_matrix(prims[i]) {
        i += 1;
    }
    let at = |i: usize| prims.get(i).copied();

    // Everything *outside* the layer, which is what an inner shadow casts from.
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

    // ⚠️ **The dilation's *sign* depends on which shadow this is.** The writer picks
    // the operator as `(spread > 0) != inner`, because growing the hole an inner
    // shadow casts from is shrinking the shadow — so reading `dilate` as "positive"
    // would flip the sign on every inner shadow in the file.
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
    // A silhouette that is neither moved nor softened is a recolour of the shape,
    // not a shadow, and the model has no row for one — so refuse rather than import
    // a shadow nobody would find.
    //
    // ⚠️ **The test is on the *values*, not on which primitives were present**
    // (§15 D572). It read `!seen_offset && !seen_blur`, which was sound only because
    // a zero `stdDeviation` refused the whole filter one function away: with a zero
    // now legal (`[S7.2-L1-04]`), a chain spelling `feOffset dx="0" dy="0"` followed
    // by `feGaussianBlur stdDeviation="0"` had two primitives, two flags set, and
    // was exactly the solid recolour sitting on the artwork that this refusal
    // exists to prevent. **Widening a reader can open a hole in a guard that reads
    // presence rather than effect**, and nothing downstream would have said so —
    // the import is a valid shadow, it is simply invisible and un-findable.
    //
    // A missing primitive still lands here, because its contribution is the zero it
    // was initialised to — so this is the old condition where it was right, and
    // stricter only where it was wrong.
    //
    // **`spread` is deliberately not in the test**, which keeps this exactly the
    // rule it was: a chain whose only content is a `feMorphology` was refused before
    // and is refused now. Whether a spread-only halo should import is a separate
    // question nobody has measured, and answering it here would be a second change
    // hiding inside a bug fix.
    if offset == kurbo::Vec2::ZERO && blur == 0.0 {
        return None;
    }

    // Colour it: the flood fills the region and the composite cuts it to the
    // silhouette.
    let flood = at(i).filter(|p| p.has_tag_name("feFlood"))?;
    let color = flood_color(flood, css, current)?;
    i += 1;
    let cut = at(i).filter(|p| p.has_tag_name("feComposite"))?;
    if cut.attribute("operator") != Some("in") {
        return None;
    }
    i += 1;
    // An inner shadow is confined to the layer's own alpha by a second composite;
    // without it the inverted silhouette would paint the whole region outside.
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
/// ⚠️ **Zero is a value, not a refusal** (§15 D572). `[S7.2-L1-04]`: this used to
/// answer `None` for `dev <= 0.0`, and all three callers propagate that with `?`
/// under D411's all-or-nothing rule — so
/// `<feDropShadow dx="1" dy="1" stdDeviation="0"/>`, a **hard-edged offset shadow**
/// and an ordinary design idiom the model holds *exactly* (`Shadow { blur: 0.0 }`),
/// refused the whole `<filter>` and the layer arrived with nothing. The
/// `is_finite()` half is the defence against a junk file; `> 0.0` was a second,
/// stricter claim that nothing had argued for.
///
/// **Negative is still refused**, which is what SVG says: a negative
/// `stdDeviation` is an error, where zero explicitly disables the primitive.
fn deviation_of(v: &str) -> Option<(f64, bool)> {
    let (dev, uneven) = match numbers(v).as_slice() {
        [s] => (*s, false),
        [x, y] => ((x + y) * 0.5, (x - y).abs() > 1e-9),
        _ => return None,
    };
    (dev.is_finite() && dev >= 0.0).then_some((dev, uneven))
}

/// `flood-color` and `flood-opacity` as one colour.
///
/// ⚠️ **Both are read through [`property`] rather than as attributes**, which is the
/// module head's rule and is not theoretical here: they are *presentation*
/// attributes, so a file that writes its filter primitives through `style=""` — as
/// Inkscape writes everything else — would otherwise give every shadow the default
/// opaque black and say nothing about it.
fn flood_color<'a>(p: roxmltree::Node<'a, '_>, css: &Css<'a>, current: Color) -> Option<Color> {
    let base = match property(p, "flood-color", css) {
        // ⚠️ **`currentColor` is legal here and used to refuse the whole
        // `<filter>`** (§15 D572, `[S7.2-L1-04]`). `parse_paint_color` does not know
        // the keyword — it is not a colour, it is a reference — so the `?` took the
        // shadow, and every other primitive with it. `current` is the referencing
        // element's own `color`, which is what SVG says a `currentColor` inside a
        // filter resolves against, and is the same value `parse_paint` has always
        // used for a `fill`.
        Some("currentColor") => current,
        Some(v) => parse_paint_color(v)?,
        None => Color::BLACK,
    };
    let alpha = property(p, "flood-opacity", css)
        .and_then(parse_len)
        .unwrap_or(1.0);
    Some(base.with_alpha(alpha.clamp(0.0, 1.0) as f32))
}

/// Whether reading `def`'s ink as **alpha** gives the same answer as reading it as
/// **luminance**, which is what SVG's `<mask>` actually means.
///
/// **They agree on opaque white and on nothing else worth relying on.** White has
/// luminance 1 and alpha 1, so a white shape hides nothing under either reading —
/// and the overwhelmingly common mask is exactly that: white shapes on an empty
/// ground, used to say *keep this part*. A **black** shape is the trap: luminance 0
/// hides what it covers, alpha 1 keeps it whole, so the two readings are opposites
/// rather than approximations.
///
/// Conservative by construction: any paint this cannot read as an opaque near-white
/// colour — a gradient, a grey, a partial alpha, a `currentColor` — answers `false`
/// and the import says the mask was approximated. *A mask that is reported as
/// approximate and is not costs a word; the reverse costs a wrong picture with
/// nothing saying so.*
fn luminance_equivalent(def: roxmltree::Node<'_, '_>, css: &Css<'_>) -> bool {
    def.descendants()
        .filter(roxmltree::Node::is_element)
        .filter(|el| !matches!(el.tag_name().name(), "mask" | "clipPath" | "defs"))
        .all(|el| {
            let prop = |name: &str| property(el, name, css);
            let Some(v) = prop("fill") else {
                // Stating nothing means SVG's initial value, which is **black** —
                // the exact case the two readings disagree about most.
                return false;
            };
            match parse_paint_color(v) {
                Some(c) => {
                    let [r, g, b, a] = c.components;
                    // Unlinearised on purpose: this is "is it white", not a colour
                    // conversion, and the threshold is nowhere near a boundary.
                    a >= 0.999 && 0.2126 * r + 0.7152 * g + 0.0722 * b >= 0.99
                }
                None => v.trim() == "none",
            }
        })
}

/// The id inside a `url(#id)`, and `None` for anything else — `none`, a bare colour,
/// a reference to another document.
///
/// ⚠️ **The paren that closes the `url(` is the *first* one, not the last.** SVG lets
/// a paint carry a fallback after the reference — `fill="url(#g) rgb(0, 0, 0)"` is
/// what Inkscape writes on every filled shape — and reading to the last `)` swallows
/// the fallback's own, handing back an "id" of `g) rgb(0, 0, 0`. That resolves to
/// nothing, so the shape loses a gradient it *could* have had: the failure is not in
/// the fallback path but in the ordinary one, which is what made it invisible.
fn reference(value: &str) -> Option<&str> {
    let rest = value.trim().strip_prefix("url(")?;
    let inner = rest[..rest.find(')')?].trim();
    inner
        .trim_matches(|c| c == '"' || c == '\'')
        .strip_prefix('#')
}

/// One `<stop>` as a colour stop, or `None` where it is not one.
///
/// ⚠️ **The offset rejects NaN and leaves the clamp to do the rest** (§15 D448).
/// `f64::clamp` propagates NaN by definition — *"returns NaN if self is NaN"* —
/// so `<stop offset="nan"/>` reached `ColorStop { offset: NaN }`, `serde_json`
/// wrote it as `null`, and the saved document came back
/// *"invalid type: null, expected f32"*: **a paste that never reopens**.
/// `+inf`/`-inf` really were handled by the clamp, which is what made this look
/// covered — a guard that is right about two of its three non-finite inputs.
///
/// ⚠️ **`!is_nan()`, not `is_finite()`, and the difference is a real defect the
/// test caught.** `is_finite` also rejects the infinities, which the clamp is
/// *correct* about: `<stop offset="1e999"/>` means "at the end" and clamps to
/// 1.0, and filtering it out drops it to the `unwrap_or(0.0)` — moving that stop
/// from one end of the ramp to the other. The first version of this filter was
/// `is_finite` and the round-trip assertion passed; the control caught it.
///
/// This is the fifth sink of `[S7.2-L5-01]`'s census and the only one that does
/// not go through [`numbers`], so `numbers`' own `is_finite` filter (§15 D421)
/// did not reach it. Rust's `f64` parser accepts `nan`, `inf` and `infinity`
/// case-insensitively, so the literal costs an attacker nothing.
///
/// **Dropped to 0.0 rather than the stop being refused**, matching the
/// `unwrap_or(0.0)` a missing offset already takes: a gradient with one
/// unreadable offset is still a gradient, and losing the whole stop would change
/// the ramp more than placing it at the start does.
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
    // ⚠️ **Clamped, like its two siblings** (§15 D692). `resolve` clamps
    // `fill-opacity` and `stroke-opacity` into `0..=1` and this line did not, so
    // an out-of-spec `stop-opacity="5"` survived the import, survived save and
    // load, and reached a `0..=100` opacity field as **500**. The offset on the
    // very next line was already clamped; the asymmetry was two lines apart and
    // was not a decision — `color::multiply_alpha` is `add_alpha(opaque, alpha *
    // rhs)` and does no range work of its own.
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
    // One name for "this element's value for", so the cascade lives in `property`
    // and every line below reads as the property it is about.
    let prop = |name: &str| property(el, name, &gradients.css);
    let mut s = inherited.clone();
    if let Some(v) = prop("color").and_then(parse_paint_color) {
        s.current = v;
    }
    // ⚠️ **The parse is guarded, not assigned** (§15 D493, `[S7.1-L1-03]`). These
    // two arms wrote the *result* of `parse_paint` straight into the style, and
    // `None` is `Style`'s spelling for **unpainted** — so a value peniko refuses
    // (`var(--brand)`, which is what a design-tool export writes; anything with a
    // unit it does not know) made the shape invisible **and destroyed the paint it
    // had inherited**: `<g fill='#00ff00'><rect style='fill:var(--x)'/></g>` came
    // back with zero fills, where CSS says an invalid declaration is dropped and
    // the green stands.
    //
    // **Every other arm of this function was already written the safe way** —
    // `stroke-width`, `stroke-dashoffset`, `stroke-miterlimit`, `fill-opacity`,
    // `font-size` and the rest all read `prop(…).and_then(parse_len)`, so a value
    // they cannot read leaves the inherited one alone. `fill` and `stroke` were the
    // two that put the parse inside the assignment.
    //
    // ⚠️ **Three meanings share one `None`, and only the *text* tells them apart.**
    // `none` and `transparent` legitimately mean unpainted. And a `url(#…)` that
    // does not resolve is a **valid declaration with a dead reference**, not an
    // invalid one: §15 D477 measured that case against Chrome and established that
    // the shape comes back *unpainted* — a fallback was written for it and taken
    // back out because both spellings paint zero black pixels.
    //
    // **This guard was written without the `url(` arm and the suite caught it**,
    // on D477's own round-trip test: leaving the inherited paint made an
    // unresolvable image reference come back **opaque black**, since SVG's initial
    // fill is black. That is precisely the failure D477 is named for, reintroduced
    // by a guard one step too broad — the shape session 3 recorded on
    // `[S7.2-L5-01]` and had no name for.
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
        // `bolder`/`lighter` are relative to the inherited weight, which is the one
        // thing a simple read cannot do — and they are vanishingly rare in a file
        // written by a drawing tool.
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
        // ⚠️ **A reference that does not resolve is `none`, not black.** SVG says to
        // use the fallback colour after the `url()` if there is one and to treat the
        // element as unpainted otherwise; painting it black would put a solid square
        // where a subtle gradient was meant to be, which is the most visible way to
        // be wrong.
        if let Some((g, units, xform)) = gradients.get(v) {
            return Some(Paint::Gradient(Box::new(g), units, xform));
        }
        // The fallback starts after the paren that closes the `url(` — the same
        // first-not-last rule [`reference`] is about, and for the same reason: the
        // fallback is very often `rgb(…)`, whose own `)` is the last one in the value.
        let fallback = v
            .strip_prefix("url(")
            .and_then(|rest| rest.find(')').map(|i| rest[i + 1..].trim()))?;
        return parse_paint_color(fallback).map(Paint::Solid);
    }
    parse_paint_color(v).map(Paint::Solid)
}

/// **The whole CSS colour grammar, and none of it is ours** — `peniko::color`
/// parses hex in three lengths, the functional notations and the 148 named colours.
fn parse_paint_color(v: &str) -> Option<Color> {
    peniko::color::parse_color(v.trim())
        .ok()
        .map(|c| c.to_alpha_color::<peniko::color::Srgb>())
}

/// A length, in the one unit the model has.
///
/// ⚠️ **`px` is the only unit read, and the rest are *not* silently accepted.** SVG's
/// `pt`, `mm`, `in` and the rest have fixed ratios to `px` that this could apply, and
/// `%` has none — it is relative to a viewport this may not have. Reading `10mm` as
/// `10` would be wrong by a factor of nearly four, so a unit that is not `px` is
/// refused and the attribute falls back to its default.
fn parse_len(v: &str) -> Option<f64> {
    let v = v.trim();
    let n = v.strip_suffix("px").unwrap_or(v);
    n.trim().parse::<f64>().ok().filter(|f| f.is_finite())
}

/// The numbers in a whitespace- or comma-separated list.
///
/// ⚠️ **Non-finite values are dropped, the way [`parse_len`] already dropped
/// them** — and the asymmetry between these two functions is what let
/// `<path d="M 0 0 L 1e999 10 Z"/>` through. `parse_len` carries
/// `.filter(|f| f.is_finite())`, so `<rect width="1e999">` was harmless and the
/// module read as guarded; this one and `BezPath::from_svg` are the *geometry*
/// paths, and they went round it. `1e999` parses to `f64::INFINITY`,
/// `serde_json` writes that as `null`, and the saved document never opens again.
///
/// **Dropped rather than clamped**, which loses a point from the polygon and is
/// the right trade: there is no coordinate that means "infinitely far" and a
/// finite substitute would put the vertex somewhere the file did not ask for.
/// `Document::apply` refuses the value outright now (`OpError::NonFinite`), so
/// without this filter the *import* would fail rather than the shape being
/// slightly wrong — a whole file lost to one bad number.
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
/// **Left to right, composing on the right**, which is what SVG means by
/// `transform="translate(10) rotate(45)"`: the rotation happens first, in the space
/// the translation establishes. Writing it the other way is the single most likely
/// way to get this function wrong, and `a_transform_list_composes_left_to_right`
/// is the test that says so.
///
/// **Angles are degrees**, unlike kurbo's radians — the other likely slip.
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
            // Rotation about a point, which is three transforms and is why this
            // arm exists rather than being folded into the one above.
            ("rotate", [a, cx, cy]) => {
                Affine::translate((*cx, *cy))
                    * Affine::rotate(a.to_radians())
                    * Affine::translate((-*cx, -*cy))
            }
            ("skewX", [a]) => Affine::new([1.0, 0.0, a.to_radians().tan(), 1.0, 0.0, 0.0]),
            ("skewY", [a]) => Affine::new([1.0, a.to_radians().tan(), 0.0, 1.0, 0.0, 0.0]),
            _ => continue,
        };
        // ⚠️ **A singular transform is dropped, not carried**, and it is the
        // reachable half of the unopenable-document Critical. `matrix(0,0,0,0,0,0)`
        // is finite, so nothing here refused it, and the shape it produced was
        // invisible on canvas but still reachable from the layers panel — so a
        // designer could select it and move it. `build::local_for_world` is
        // `parent_world.inverse() * world`: on a singular parent kurbo divides by
        // a zero determinant, every coefficient comes back `NaN`, and the next
        // autosave wrote a file nothing could ever open.
        //
        // **The identity is the same answer `image.rs` already gives** where it
        // *derives* a degenerate affine — `Framing::identity()` for a frame or a
        // source with no extent (`degenerate_inputs_resolve_to_the_identity_framing`)
        // — and it is the honest one: a matrix that collapses space to a line or a
        // point draws nothing either way, so the shape is no less visible for
        // being placed where it was written.
        //
        // ⚠️ **That is a narrower precedent than "a singular brush transform",
        // which is what this comment used to claim** (§15 D713). `Framing` is
        // computed, not stored; the field with no normalisation anywhere is
        // `GradientBrush::transform`, which the loader accepts singular to this
        // day. `build::affine_is_invertible` is the guard its *readers* ask.
        // 🚨 **`build::affine_is_invertible`, not a determinant test of this
        // function's own** (§15 D762). This read
        // `t.determinant() == 0.0 || !t.as_coeffs().iter().all(|c| c.is_finite())`,
        // which is one of the four spellings D713 counted for one question — and
        // the predicate that answers it is *will the inverse be representable*
        // rather than *is this number small*, which is why it is threshold-free.
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

    /// **An `id` that is not a usable name falls through to the numbering, the
    /// same answer an element with no `id` gets** (§15 D644, `[S2.2-L1-07]`).
    ///
    /// Measured before the fix: `""`, `"   "` and `"a\nb"` were installed
    /// verbatim, so the layers panel showed a row with no label at all, a row
    /// labelled with three spaces, and a name with a line break in a panel that
    /// lays out one line. Every one of them is something both rename fields
    /// already refuse (§15 D54, D127) — import was a third surface agreeing with
    /// neither.
    ///
    /// ⚠️ **The fourth rect is the control and it is the assertion that says
    /// this is a filter rather than a rename**: a perfectly ordinary `id`
    /// survives untouched. Without it, `element_name` returning `None`
    /// unconditionally passes everything above.
    ///
    /// ⚠️ **`is_generated` is asked of the *stem*, so the assertion is on the
    /// leading word.** A generated name here is "Rectangle 1", not "Rectangle".
    ///
    /// Flip: `.and_then(crate::naming::usable)` back to `.map(str::to_string)`.
    /// Red on the first rect, `""` against a generated name.
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

    /// **The transform list composes left to right, and its angles are degrees.**
    ///
    /// The two ways this function is most likely to be wrong, asserted as one point
    /// rather than as a matrix: `translate(10,0) rotate(90)` must send (1,0) to
    /// (10,1) — rotate first, in the space the translate established. Writing the
    /// composition the other way round sends it to (0,11) instead, and reading the
    /// angle as radians sends it very nearly nowhere.
    ///
    /// ⚠️ **Flipped both ways, and the second flip's *value* was predicted wrong.**
    /// Swapping `out *= t` to `out = t * out` fails here with (6.7e-16, 11.0), as
    /// predicted. Dropping `.to_radians()` was predicted to land near (10.02, 0.03)
    /// — "very nearly nowhere" — and actually lands at **(9.55, 0.89)**, because 90
    /// *radians* is 5156°, which is 60.7° after wrapping and is nothing like a small
    /// angle. *A wrong unit does not read as a small error; it reads as a plausible
    /// one*, which is the argument for asserting the point rather than eyeballing the
    /// picture. Neither flip is caught by a test that only checks a translation,
    /// which is why this fixture has both in one string.
    #[test]
    fn a_transform_list_composes_left_to_right_in_degrees() {
        let t = parse_transform("translate(10,0) rotate(90)");
        let p = t * Point::new(1.0, 0.0);
        assert!(
            (p - Point::new(10.0, 1.0)).hypot() < 1e-9,
            "rotate happens inside the translate: {p:?}"
        );
        // A rotation about a point is three transforms, and that point is fixed.
        let about = parse_transform("rotate(180 5 5)");
        let q = about * Point::new(5.0, 5.0);
        assert!((q - Point::new(5.0, 5.0)).hypot() < 1e-9, "{q:?}");
        assert!(((about * Point::new(6.0, 5.0)) - Point::new(4.0, 5.0)).hypot() < 1e-9);
    }

    /// **The primitives land where the markup says**, which is the whole of what an
    /// importer is for and an assertion each of them can fail on its own.
    ///
    /// Each shape's box is deliberately *not* at the origin, because the model's
    /// shapes all start at their own origin and carry placement on the transform — so
    /// an importer that forgot to translate would put all three in the corner and
    /// still produce three shapes of the right size.
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

    /// **A `viewBox` is a transform onto the width and height the file claims.**
    ///
    /// `0 0 48 48` drawn at 24×24 is half scale, so a 48-unit square comes back 24
    /// wide. The size the caller is told is the *declared* one — what the paste will
    /// occupy — rather than the viewBox's, which is the distinction `declared_size`
    /// exists for.
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

    /// **A declared size that is not a size is not a size to either reader**
    /// (`[S7.1-L1-07]`, §15 D511).
    ///
    /// `declared_size` guarded its pair with `w > 0.0 && h > 0.0` and fell through
    /// to the `viewBox`; `view_transform`, ten lines below, read the same two
    /// attributes through the same parser and divided by them unchecked. So the
    /// two disagreed about which declaration they believed, and the transform lost.
    /// Measured before the fix:
    ///
    /// | markup | `Import::size` | root transform |
    /// | --- | --- | --- |
    /// | `width="24" height="0"` | `48×48` | `[0, 0, 0, 0, 12, 0]` |
    /// | `width="-24" height="-24"` | `48×48` | `[-0.5, 0, 0, -0.5, 0, 0]` |
    /// | `width="0" height="0"` | `48×48` | `[0, 0, 0, 0, 0, 0]` |
    ///
    /// ⚠️ **Two of the three are singular**, which is the half that cannot be
    /// recovered from: the whole pasted subtree collapses to a point and every
    /// resize handle is in the same place, so no gesture undoes it. The negative
    /// pair is a drawing rotated through the origin, which a browser renders as
    /// nothing at all.
    ///
    /// The fix is one guarded reader, `declared_pair`, and a non-positive
    /// declaration now means *no box to fit into* — the arm `view_transform`
    /// already had for a missing `width`/`height`. So the drawing pastes at its own
    /// units, which is what the `48×48` assertions below say.
    ///
    /// **Flip run**, `declared_pair`'s `(w > 0.0 && h > 0.0)` dropped: fails on
    /// *"a zero height is no declaration: the size falls through to the viewBox"*
    /// at `24×0` against `48×48`. ⚠️ **The predicted site was the geometry
    /// assertion and it was wrong** — with the guard gone the *size* is wrong too,
    /// and it is checked first. Which is a better result than the prediction:
    /// before the fix the two readers disagreed, and one guarded reader makes them
    /// fail together.
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

        // The control, in the same shape: a real declaration still scales.
        assert_eq!(
            at("24", "24"),
            (
                Some(Size::new(24.0, 24.0)),
                kurbo::Rect::new(0.0, 0.0, 24.0, 24.0)
            ),
            "control: a positive pair is still fitted onto"
        );
    }

    /// **Paint inherits down groups, and an inline `style` beats a presentation
    /// attribute** — CSS's own precedence, and the one rule here that is not obvious
    /// from the markup.
    ///
    /// The fixture makes both testable at once: the outer `<g>` says red, the first
    /// rect says blue as an *attribute* and green in `style`, and the second says
    /// nothing at all and must come out red.
    #[test]
    fn paint_inherits_and_an_inline_style_wins() {
        // `r##` rather than `r#`, because `fill="#ff0000"` contains the `"#` that
        // would end a one-hash raw string.
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <g fill="#ff0000">
                   <rect width="10" height="10" fill="#0000ff" style="fill:#00ff00"/>
                   <rect width="10" height="10"/>
                 </g>
               </svg>"##,
        );
        let ids = shapes(&doc, &out);
        // Read as four channels rather than as one packed integer, which is worth a
        // line: `Rgba8::to_u32` packs little-endian, so green is `0xff00ff00` and a
        // hand-written `0x00ff00ff` is a plausible expectation that is simply wrong.
        let fill = |id: NodeId| match &doc.get(id).unwrap().paint().fills[0].brush {
            Brush::Solid(c) => c.to_rgba8().to_u8_array(),
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

    /// **`fill="none"` is not a colour and not black**, which is the difference
    /// between an outlined icon arriving as an outline and arriving as a silhouette.
    /// The default when nothing says otherwise *is* black — the other half of the
    /// same rule, and the half a reader is likely to doubt.
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

    /// **`fill-rule="evenodd"` survives the trip**, which is what a compound path's
    /// holes depend on — and a property this app only gained on 2026-08-31 (§15
    /// D239). An importer written a day earlier could not have carried it, and every
    /// compound icon would have pasted as a silhouette.
    #[test]
    fn the_even_odd_rule_is_carried() {
        let (doc, out) = imported(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <path d="M0 0h100v100h-100z M30 30h40v40h-40z" fill-rule="evenodd"/>
               </svg>"#,
        );
        let id = shapes(&doc, &out)[0];
        assert_eq!(doc.get(id).unwrap().fill_rule(), FillRule::EvenOdd);
        // The picture, not only the field: the inner square is a hole.
        let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
            panic!("a Path")
        };
        assert!(!FillRule::EvenOdd.contains(path, Point::new(50.0, 50.0)));
        assert!(FillRule::EvenOdd.contains(path, Point::new(10.0, 50.0)));
    }

    /// **What it cannot read is counted, not dropped** — the module's own rule, and
    /// what makes a lossy paste honest. The count is of *names*, so a file with forty
    /// `<text>` elements reports one kind of loss rather than forty.
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

    /// A sink that accepts anything, so a test can drive the `<image>` path without a
    /// real decoder. The id is the byte length, which makes the same payload twice
    /// the *same* picture — the property the dedup test needs.
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

    /// **A picture is a `Rect` with an image fill**, which is how this model holds
    /// every image — there is no `NodeKind::Image` (§5.5a), and that is what makes an
    /// imported picture croppable and re-fillable like any other layer.
    ///
    /// ⚠️ **SVG's default is *contain* and ours is *cover***, which is the one place
    /// the two models disagree by default rather than by instruction. A `<image>`
    /// with no `preserveAspectRatio` means `xMidYMid meet`, so it has to arrive as
    /// `crate::image::ImageFit::Fit` and not as the field's own default.
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
        let Brush::Image(b) = &doc.get(id).unwrap().paint().fills[0].brush else {
            panic!("an image fill")
        };
        assert_eq!(
            b.image.fit,
            crate::image::ImageFit::Fit,
            "SVG's default is contain, which is not this field's default"
        );
    }

    /// **The same picture twice is one table entry and two fills.** The id is a
    /// content hash, so a second `AddImage` would give the transaction an inverse that
    /// removes an entry the first fill is still using.
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

    /// **Without a sink there is nowhere to put a picture**, so an `<image>` is
    /// skipped and counted like anything else — which is what every test above that
    /// passes `None` is relying on, and what a headless caller gets.
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

    /// **A `<defs>` is read by lookup and never walked**, which is the one structural
    /// rule that changes what the user sees: walking it would paste every template in
    /// the file onto the canvas as a visible shape.
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

    /// **`stop-opacity` is clamped into `0..=1`, like `fill-opacity` and
    /// `stroke-opacity` beside it** (§15 D692, `[S23.2-L1-03]`).
    ///
    /// Out-of-spec input, and no common tool emits it — but nothing refused it
    /// either, so `stop-opacity="5"` imported as 5.0, survived save and load,
    /// and reached the app's `0..=100` opacity field as **500**; one bare click
    /// there rescaled every stop in the ramp. `resolve` clamps the other two
    /// opacity properties and `stop_of` clamps the *offset* on the line below
    /// this alpha, so the gap was two lines from two statements of the rule.
    ///
    /// ⚠️ **Both ends are asserted.** A clamp written as `min` passes the high
    /// case and lets the negative through, which is the plausible wrong
    /// version; the low assertion is what has teeth against it.
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)
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
        let Brush::Gradient(g) = &doc.get(id).unwrap().paint().fills[0].brush else {
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

    /// **A bounding-box gradient resolves against the shape it is on**, which is
    /// SVG's *default* `gradientUnits` and therefore the common case rather than the
    /// exotic one.
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
        let Brush::Gradient(g) = &doc.get(id).unwrap().paint().fills[0].brush else {
            panic!("a gradient fill")
        };
        let peniko::GradientKind::Linear(l) = g.gradient.kind else {
            panic!("linear")
        };
        // The rect's own geometry starts at 0 — its `x` is on the transform — so the
        // box the gradient maps onto is 0..50, not 100..150.
        assert!((l.start.x - 0.0).abs() < 1e-9, "{:?}", l.start);
        assert!((l.end.x - 50.0).abs() < 1e-9, "{:?}", l.end);
        assert_eq!(g.gradient.stops.len(), 2);
        assert_eq!(
            g.transform,
            Affine::IDENTITY,
            "a linear mapping is baked into the two points, so nothing rides along"
        );
    }

    /// **A duplicate id is first-wins everywhere, not only where the rule is
    /// written** (§15 D642, `[S7.2-L1-05]`).
    ///
    /// `Gradients::collect` builds three maps. `ids` was written with
    /// `entry().or_insert()` under a comment stating the rule; `clips` and
    /// `by_id` were plain `insert`s, i.e. last-wins. So one document had two
    /// answers for `#g`. Three readers took the first — `Gradients::element`
    /// (behind `<use href>`), `Gradients::filter`, and `gradient_chain`'s `href`
    /// walk — and two took the last: `Gradients::get`, the fill and stroke path
    /// and by far the common one, and `Gradients::clip`, which is `clips` and so
    /// covers a duplicate `<mask id>` as well as a `<clipPath id>`.
    ///
    /// ⚠️ **`Gradients::clip` reads `clips`, not `ids`**, which is worth spelling
    /// out because the obvious guess is that a *clip* lookup goes through the
    /// general id map. It does not, and that is why it was on the broken side.
    ///
    /// ⚠️ **Chrome is the reference and it was measured, not assumed**: the same
    /// file served as `image/svg+xml` paints the rect **red**, and Ondin painted
    /// it blue. A duplicate id is invalid SVG and no real file in the tree has
    /// one; this is here because the code stated the rule and then broke it two
    /// loops later, which is cheap now and expensive to rediscover.
    ///
    /// **The `href` half is the assertion that says the two maps agreed**, not
    /// merely that the fill changed: `#h` inherits its stops from `#g` through
    /// `gradient_chain`, which reads `ids`, so before the fix the *same* document
    /// resolved `#g` to red down one path and blue down the other. Asserting the
    /// fill alone would pass against a fix that flipped `ids` to last-wins
    /// instead, which is the opposite repair and equally consistent.
    ///
    /// Flip: `by_id.entry(id).or_insert(…)` back to `by_id.insert(id, …)`. Red
    /// at the first assertion with `0.0` for red, as predicted.
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

        // ⚠️ **Both read before either is asserted, on purpose.** Asserting
        // inside the loop stops at the first rect, and the finding's sharpest
        // fact is the *pair*: under the old code these two came back
        // `["blue", "red"]` — one document, two answers for `#g`, from the two
        // maps disagreeing. A message naming only rect 0 hides that.
        let seen: Vec<&str> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let Brush::Gradient(g) = &doc.get(*id).unwrap().paint().fills[0].brush else {
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
        let Brush::Gradient(g) = &doc.get(id).unwrap().paint().fills[0].brush else {
            panic!("a gradient fill")
        };
        let peniko::GradientKind::Linear(l) = g.gradient.kind else {
            panic!("linear")
        };
        (g.gradient.clone(), l)
    }

    /// **A gradient inherits through `xlink:href`, per attribute and per stop list**,
    /// which is not an exotic corner: Inkscape writes *every* gradient as a
    /// stop-holding element in `<defs>` plus one positioned element per use. Reading
    /// only the referencing element found stops on nine of the camera drawing's 175
    /// gradients and dropped the other 166 — which is to say the whole picture.
    ///
    /// The fixture puts the *stops* behind the href and the *position* in front of
    /// it, so an importer that inherited one and not the other fails on the half it
    /// missed rather than passing by luck.
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

    /// **A reference chain ends rather than looping**, which XML permits and a
    /// depth-first reader does not survive.
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

    /// **A `userSpaceOnUse` gradient is written in the space the shape's `x`/`y` are
    /// in, and our brush is written in the shape's own** — so the placement has to
    /// come back out of it. Leaving it in was the same bug as never mapping the unit
    /// square, one coordinate system further out, and the symptom was a gradient
    /// sitting entirely off the shape it painted.
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

    /// **`gradientTransform` composes *inside* the `gradientUnits` mapping**, which is
    /// the order the two rules are written in and the one that is not obvious: the
    /// transform maps the gradient's own coordinates into the space the units mapping
    /// starts from.
    ///
    /// `scale(2,1)` over `x2="25"` must reach 50, and the shape is 50 wide so a
    /// dropped transform lands at 25 — half, and visibly so.
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

    /// ⚠️ **A linear gradient's bands are perpendicular to its vector *in its own
    /// space*, and a non-uniform transform does not keep them there.** Mapping the
    /// two endpoints is therefore not the same as mapping the gradient — the bands
    /// come out at the wrong angle, and only the angle is wrong, so the picture looks
    /// plausible and is not.
    ///
    /// The fixture squashes y by 1/4 under a gradient at 45°. The endpoints map to
    /// `(0,0) → (40, 10)`, an 14° slope; the *correct* answer keeps the iso-lines at
    /// 45° in gradient space, which puts the equivalent vector at 76° instead.
    ///
    /// **Flipped**: replacing the covector with `p(l.end)` gives exactly `(40, 10)`
    /// and fails here. ⚠️ **And it fails *only* here** — `a_bounding_box_gradient_is_
    /// mapped_onto_the_shape` stays green under the naive version, because its
    /// gradient is axis-aligned and the correction is a no-op on an axis. So the
    /// oldest test in this file about gradient geometry had no teeth against the one
    /// error a gradient's geometry can have that leaves it looking plausible, and this
    /// fixture is the only thing standing between that and a silent regression.
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

    /// ⚠️ **A radius scales by `sqrt(|det|)`, not by the average of the matrix
    /// diagonal.** For a rotation the diagonal *is* the cosine — at a quarter turn it
    /// is zero — so the old reading collapsed the radius to nothing and the gradient
    /// painted its last stop over the whole shape. The camera drawing's four corner
    /// highlights are white-fading-to-transparent, so "nothing" was the one wrong
    /// answer that is invisible as a wrong answer: the corners just went dark.
    ///
    /// **Flipped**: `(|a| + |d|) / 2` gives a radius of **0** here, so the assertion
    /// fails at the far end of its range rather than near it.
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
        let Brush::Gradient(g) = &doc.get(id).unwrap().paint().fills[0].brush else {
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

    /// **A squashed radial gradient is exact now, and rides on the brush's own
    /// transform** (§15 D412). This test asserted the opposite until 2026-09-03 —
    /// that the ellipse was drawn round and *reported* — which was the honest answer
    /// for a model whose radial gradient was two circles and nothing else.
    ///
    /// ⚠️ **What changed is the model, not the reading**: an ellipse *is* a circle
    /// under a squash, and both backends had taken a brush transform all along for
    /// image framing. So the entry that called this a `peniko` limitation was naming
    /// the wrong thing, and the fix was a field rather than an upstream release.
    ///
    /// The two assertions are a pair on purpose. The **circle is unmapped** — a
    /// `scale(1, 6)` baked into `end_radius` would have made it 60, or 24.5 if
    /// area-preserved, and 10 is the only answer that means "the squash did not go
    /// in here". The **transform carries it**, which is where it did go.
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
        let Brush::Gradient(g) = &doc.get(id).unwrap().paint().fills[0].brush else {
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

    /// **A squash too small to see is carried too, and a similarity still is not**
    /// (§15 D412).
    ///
    /// `Paint::squashed_radial`'s threshold was `1.05` while it chose between
    /// *approximating and saying so* and *approximating silently* — five percent was
    /// invisible, and reporting it on every ordinary file would have been noise. Once
    /// the alternative became **exact**, that deadband was the one case left that was
    /// wrong with nothing saying so, so it is numerical noise now.
    ///
    /// **Flipped, and the two halves came out differently.** Restoring `1.05` fails
    /// the first assertion, which is the point of the change. Dropping the threshold
    /// to `1.0` — no deadband at all — was predicted to fail the second and **does
    /// not**, which is a finding about the arithmetic rather than a weak test: for a
    /// similarity the two singular values come out *bit-identical*, so the ratio is
    /// exactly `1.0` and no margin is needed to keep it out. Tried twice, first with
    /// `matrix(0, 3, -3, 0, 0, 0)` — dismissed as too exact, every operand being a
    /// small integer — and then with `rotate(37) scale(3)`, where the relative error
    /// still lands below the f64 epsilon *at* 1.0 and rounds away. **So `1e-6` is
    /// insurance against a mapping composed through more arithmetic than any fixture
    /// here builds, not a working part**, and the sentence above claiming it is
    /// needed for noise was overstated.
    ///
    /// ⚠️ **The control still earns its place for a different reason than the one it
    /// was written for.** It pins that a similarity is *baked*, which the feature
    /// depends on however the threshold is spelled: were similarities to take the
    /// transform path, every rotated gradient would grow a field it does not need and
    /// rewrite its document on the next save (invariant 9).
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
            let Brush::Gradient(g) = &doc.get(id).unwrap().paint().fills[0].brush else {
                panic!("a gradient fill")
            };
            g.transform
        };
        // 2% — under the old 1.05 and quietly baked until 2026-09-03.
        assert_ne!(
            of("scale(1, 1.02)"),
            Affine::IDENTITY,
            "a squash too small to see is still a squash"
        );
        // A 37° turn at 3×: a similarity, so the two circles hold it exactly and
        // nothing needs to ride along.
        assert_eq!(
            of("rotate(37) scale(3)"),
            Affine::IDENTITY,
            "a similarity is baked, or every rotated gradient grows a field it does \
             not need and rewrites its document on the next save"
        );
    }

    /// **`spreadMethod` is `extend`**, and a reflected gradient is how a brushed-metal
    /// sheen is written — one ramp, mirrored. Padding it instead leaves the outer half
    /// flat at the last stop, which reads as a white band rather than a curve.
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

    /// ⚠️ **A paint's `url()` closes at its *first* paren, and the fallback after it
    /// is the reason.** `fill="url(#g) rgb(0, 0, 0)"` is what Inkscape writes on every
    /// filled shape, and reading to the last `)` hands back an "id" of
    /// `g) rgb(0, 0, 0` — so the failure is in the *ordinary* path, not the fallback
    /// one: a gradient that exists and is named correctly resolves to nothing.
    ///
    /// Both halves are here because they fail apart: the first rect proves the
    /// reference still resolves, the second that the fallback is reached when it does
    /// not, and the third that a fallback with no parens in it never regressed.
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
            matches!(brush(0), Brush::Gradient(_)),
            "the reference resolves despite the fallback beside it"
        );
        assert_eq!(
            brush(1),
            Brush::Solid(Color::from_rgb8(0, 128, 0)),
            "and an unresolvable one falls back to the colour, parens and all"
        );
        assert_eq!(brush(2), Brush::Solid(Color::from_rgb8(0, 255, 0)));
    }

    /// **An unequal `rx`/`ry` is three shapes, not one rounded rect.** SVG's corner is
    /// an ellipse and ours is a circle, so the pair spans a rect, an ellipse and a
    /// path — and the *large* disagreements live at the clamp: `rx="7.5" ry="65"` on a
    /// 15×130 rect is Inkscape's spelling of an ellipse, which the old `min(rx, ry)`
    /// reading drew as a stadium.
    ///
    /// ⚠️ **The kind is asserted rather than the outline**, because that is what the
    /// old reading got wrong while keeping the bounding box exactly right: a stadium
    /// and an ellipse have the same box, so a test on `world_bounds` would have been
    /// green for the bug this is about.
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
        // The path case still occupies the box the markup asked for — the point is
        // that the box was never the thing in doubt.
        assert_eq!(
            world_box(&doc, ids[1]),
            kurbo::Rect::new(0.0, 0.0, 40.0, 130.0)
        );
    }

    /// The first node's effect stack.
    ///
    /// Named apart from the free `effects_of` this module also has, which reads a
    /// `<filter>` element: one asks the *document* what a node ended up with and the
    /// other asks the *markup* what it said, and a test asserting the first while
    /// reading as though it called the second is a test about nothing.
    fn stack_of(doc: &Document, out: &Import) -> Vec<crate::effect::Effect> {
        doc.get(shapes(doc, out)[0]).unwrap().effects().to_vec()
    }

    /// **A `<filter>` holding one `<feGaussianBlur>` is `EffectKind::LayerBlur`**,
    /// and the conversion is the one place SVG's *deviation* and the model's *radius*
    /// meet: `stdDeviation="3"` is a radius of 6, because
    /// `crate::effect::BLUR_DEVIATION` is a half.
    ///
    /// **The simplest of the shapes `effects_of` reads**, and the one that pays for
    /// itself on real files — a lone blur is what Inkscape's *Blur* slider writes, so
    /// a drawing with fifty-six filters can have fifty-six of exactly this.
    ///
    /// ⚠️ **The `style` spelling is here because it is the one that was broken.** The
    /// reader tested `has_attribute("filter")`, and Inkscape writes the whole
    /// presentation block into `style`, so a real drawing's blurs were not read *and
    /// were not reported either* — a silent loss, which is the one thing
    /// `Import::skipped` exists to prevent.
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
                    }] if (radius - 6.0).abs() < 1e-9
                ),
                "{paint} → {fx:?}"
            );
            assert!(out.skipped.is_empty(), "{paint} → {:?}", out.skipped);
        }
    }

    /// **`clip-path` and `mask` are properties too**, which is the sweep the `filter`
    /// bug earned: every attribute read in this file was checked against the list of
    /// presentation properties, and these two were the others. Nothing had reported
    /// them wrong yet — this is the latent case, fixed because the live one showed
    /// what it costs.
    ///
    /// The third assertion is the report's own honesty: `clip-path: none` is the
    /// property's initial value, so an element stating it has no clip to lose and
    /// there is nothing to say about it.
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

    /// **A blur composites on a container**, which is why it is read in
    /// `Builder::chrome` beside opacity and `display` rather than in
    /// `Builder::shape`: SVG puts a `filter` on a `<g>` as readily as on a shape, and
    /// so does the model — that is the whole reason `effects` is a node field rather
    /// than something inside `Paint`.
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

    /// **Everything a `<filter>` can be that this cannot hold is still skipped *and
    /// counted*** — which is the half of this feature that keeps the report honest. A
    /// primitive this has no kind for, a reference to nothing, and ⚠️
    /// `primitiveUnits="objectBoundingBox"`, where `stdDeviation` is a *fraction of
    /// the box* and reading it as a length turns half a shape into half a pixel.
    ///
    /// ⚠️ **The first two cases are the *all-or-nothing* rule, and they are here
    /// because each half of them is separately readable.** A blur followed by a
    /// positioned `feOffset` is a stage-one blur this understands and then a
    /// displacement it does not; a shadow chain followed by a `feTurbulence` is a
    /// whole shadow and then noise. Reading the readable half of either would import
    /// a drawing that is wrong with nothing saying so — worse than the skip, because
    /// a skipped filter is at least *reported*. **This is the assertion that stops a
    /// future primitive being bolted on greedily.**
    ///
    /// **Flipped**: turning `effects_of`'s final `return None` into `i += 1` — walk
    /// past what you do not know — fails on the *first* def's emptiness assertion,
    /// which imports a layer blur for a filter that also displaces the drawing.
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
        // And a `filter` naming nothing at all is the same answer.
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                 <rect width="50" height="20" filter="url(#gone)"/>
               </svg>"##,
        );
        assert!(stack_of(&doc, &out).is_empty());
        assert_eq!(out.skipped, vec!["filter"]);
    }

    /// **`stdDeviation` may name two numbers, and the model's blur is isotropic** — so
    /// an unequal pair is averaged and *said*, which is what `Import::approximated`
    /// is for. An equal pair is exact and stays silent, which is the assertion that
    /// stops the report from crying wolf on `stdDeviation="2 2"`.
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

    /// **The shadow idiom — a silhouette, an offset, a blur, then a flood cut back to
    /// it — is `EffectKind::DropShadow`** (§15 D411). This is what this app's own
    /// writer emits per shadow and what every pre-`feDropShadow` file writes by hand,
    /// and until 2026-09-03 it was skipped whole.
    ///
    /// ⚠️ **The colour is split across two primitives and both halves are load-bearing.**
    /// `feFlood` carries the RGB and `flood-opacity` carries the alpha, because SVG
    /// has no place for a colour with alpha in it here — so a reader that took
    /// `flood-color` alone would give every shadow in every file full opacity, which
    /// is the difference between a shadow and a black shape.
    ///
    /// **Flipped**: dropping the `with_alpha` in `flood_color` fails here *and* in
    /// three other places — the `feDropShadow` test, the `style` test, and the
    /// round-trip suite's `an_effect_stack_round_trips_as_the_same_stack`. Predicted
    /// two of the four; the alpha is read by every door into this, which is the
    /// argument for it being one function rather than a line in each.
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

    /// **An inverted silhouette is an inner shadow, and the dilation's *sign* flips
    /// with it.** The writer picks `dilate` or `erode` from `(spread > 0) != inner`,
    /// because growing the hole an inner shadow casts from is *shrinking* the shadow
    /// — so a reader that took `dilate` to mean "positive" would negate the spread on
    /// every inner shadow ever written, and on nothing else.
    ///
    /// ⚠️ **All four combinations are asserted rather than the two obvious ones.** The
    /// two that share an answer with the naive reading (`dilate` on a drop, `erode`
    /// on an inner) are exactly the ones that keep it green.
    ///
    /// **Flipped**: `if dilate != inner` → `if dilate`, the reading anyone would
    /// write first, fails at `true dilate` with `3.0` against `-3.0` — the third of
    /// the four cases, because the first two are the ones both readings agree on.
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

    /// **`<feDropShadow>` says the whole thing in one primitive**, which is what a
    /// modern tool writes. Its defaults are the spec's — `dx`/`dy` of 2, a deviation
    /// of 2, opaque black — and asserting them is what stops an omitted attribute
    /// silently becoming a zero.
    ///
    /// ⚠️ **It has no spread**, so the model's is zero rather than approximated:
    /// nothing was lost in the reading, because the primitive never carried one.
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

    /// **A `stdDeviation` of zero is a value, not an unreadable file**
    /// (`[S7.2-L1-04]`, §15 D572).
    ///
    /// D411's all-or-nothing rule is right and is not what was wrong: `deviation_of`
    /// answered `None` for a stated zero and all three callers propagate that with
    /// `?`, so a **hard-edged offset shadow** — an ordinary design idiom the model
    /// holds exactly, `Shadow { blur: 0.0 }` — took the whole `<filter>` down with
    /// it and the rectangle arrived with no shadow at all.
    ///
    /// Four inputs and their two controls:
    ///
    /// | filter | before | now |
    /// | --- | --- | --- |
    /// | `feDropShadow stdDeviation="0"` | 0 effects, `skipped:["filter"]` | one shadow, `blur == 0` |
    /// | `feDropShadow stdDeviation="2"` (control) | one shadow | unchanged |
    /// | `feGaussianBlur stdDeviation="0"` | 0 effects, `skipped:["filter"]` | 0 effects, **nothing skipped** |
    /// | `feGaussianBlur stdDeviation="2"` (control) | one blur | unchanged |
    ///
    /// ⚠️ **The third row is a *reporting* fix, not a rendering one.** SVG says a
    /// zero deviation disables the primitive, so "unchanged" was always the right
    /// picture and Ondin drew it — what was wrong is that the paste claimed a filter
    /// had been dropped. That is the same shape as the bare `<feOffset/>` this
    /// reader already treats as the identity, and the assertion to watch is
    /// `skipped.is_empty()` rather than the effect count, which does not move.
    ///
    /// **A negative deviation is still refused**, which SVG says is an error — the
    /// fifth row, and the reason the guard became `>= 0.0` rather than being deleted.
    ///
    /// **Flip run**, `>= 0.0` put back to `> 0.0`: fails on the very first
    /// assertion, `skipped: ["filter"]`. ⚠️ **Predicted at the blur value and wrong
    /// by two assertions** — the reasoning walked to the interesting number and
    /// forgot that a refused filter reports itself before it fails to be a shadow.
    /// Worth writing down because it is the *reported symptom* that bites first
    /// here, which is the order this file's own advice asks for and got by luck
    /// rather than by design.
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

        // The control: the same filter with a deviation reads as it always did.
        let (doc, out) = filtered(r#"<feDropShadow dx="1" dy="1" stdDeviation="2"/>"#);
        assert_eq!(shadow(&stack_of(&doc, &out), 0).0.blur, 4.0);

        // A lone zero blur is the identity — no effect *and* no complaint.
        let (doc, out) = filtered(r#"<feGaussianBlur stdDeviation="0"/>"#);
        assert!(
            out.skipped.is_empty(),
            "a disabled primitive is not a skipped filter: {:?}",
            out.skipped
        );
        assert!(stack_of(&doc, &out).is_empty(), "and it adds nothing");
        // Its control, so the assertion above is not about a reader that has stopped
        // reading blurs altogether.
        let (doc, out) = filtered(r#"<feGaussianBlur stdDeviation="2"/>"#);
        assert_eq!(stack_of(&doc, &out).len(), 1);

        // Negative is an error in SVG and stays a refusal.
        let (_, out) = filtered(r#"<feGaussianBlur stdDeviation="-2"/>"#);
        assert_eq!(out.skipped, vec!["filter".to_string()]);
    }

    /// **A chain that neither moves nor softens is still a recolour, and letting a
    /// zero deviation through nearly opened that hole** (§15 D572).
    ///
    /// `shadow_chain` refuses a silhouette *"neither moved nor softened"*, because
    /// the model has no row for a shape restated in another colour and importing one
    /// puts a solid slab exactly on the artwork. That guard used to read
    /// `!seen_offset && !seen_blur` — *which primitives were present* — and it was
    /// sound only because a zero `stdDeviation` refused the whole filter one function
    /// away. Making the zero legal (`[S7.2-L1-04]`) makes
    /// `feOffset dx="0" dy="0"` + `feGaussianBlur stdDeviation="0"` a chain with two
    /// primitives, both flags set, and no effect at all.
    ///
    /// ⚠️ **Widening a reader can open a hole in a guard that reads presence rather
    /// than effect** — and nothing downstream would object, because the import is a
    /// perfectly valid shadow that happens to be invisible and un-findable. The
    /// guard is on the values now.
    ///
    /// ⚠️ **This test exists because a flip did not bite, and the flip that did not
    /// bite was aimed at the wrong half.** The first attempt kept the presence test
    /// and split the blur flag in two; all 79 `svg_in` tests stayed green *and so did
    /// this one's first fixture*, because `feOffset` sets `seen_offset` whether or
    /// not it moves anything — so the hole was one primitive wider than the change
    /// that opened it, and only writing the fixture found that out.
    ///
    /// **Flip run**, the guard put back to `!seen_offset && !seen_blur`: fails on *"a
    /// chain that neither moves nor softens is not a shadow"* with one effect. Two
    /// controls, because a reader that had stopped reading chains at all would pass
    /// the refusal.
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

        // Control one: it moves. A hard edge from the chain, as `feDropShadow`
        // gives from the shorthand.
        let (doc, out) = chain("4", "0");
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let (s, inner) = shadow(&stack_of(&doc, &out), 0);
        assert!(!inner);
        assert_eq!(s.offset, kurbo::Vec2::new(4.0, 0.0));
        assert_eq!(s.blur, 0.0, "hard-edged, read out of a hand-written chain");

        // Control two: it softens without moving, which was always a shadow.
        let (doc, out) = chain("0", "2");
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let (s, _) = shadow(&stack_of(&doc, &out), 0);
        assert_eq!(s.offset, kurbo::Vec2::ZERO);
        assert_eq!(s.blur, 4.0);
    }

    /// **`flood-color="currentColor"` is legal and used to refuse the whole
    /// `<filter>`** (`[S7.2-L1-04]`, §15 D572).
    ///
    /// `parse_paint_color` does not know the keyword — it is a *reference*, not a
    /// colour — so the `?` in `flood_color` took the shadow and every primitive
    /// beside it. The value comes from the **referencing** element's `color`, which
    /// is what SVG says and what `parse_paint` has always used for a `fill`; a
    /// `<filter>` lives in `<defs>` and inherits nothing of its own, which is why it
    /// had to be threaded rather than read.
    ///
    /// ⚠️ **The colour is asserted, not merely the arrival.** A fix that resolved
    /// `currentColor` to black would pass "a shadow arrives" and be wrong on every
    /// file that uses the keyword for the reason anyone uses it.
    ///
    /// **Flip run**, the `Some("currentColor")` arm removed: fails on the `skipped`
    /// assertion with `["filter"]`, one before *"a currentColor flood is readable"*.
    /// Same correction as the test above — the report lands before the loss does.
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

    /// **The hand-written idiom blurs *before* it offsets, and reads the same.**
    /// `feGaussianBlur in="SourceAlpha"` then `feOffset` is what every file written
    /// before `feDropShadow` existed does; this app's writer offsets first. The two
    /// are the same picture — a shift commutes with a convolution — so insisting on
    /// one order would refuse half the files that mean exactly this.
    ///
    /// ⚠️ **`SourceAlpha` is also the *only* thing marking the chain here**: there is
    /// no silhouette primitive to recognise, so the `in` attribute is what says this
    /// is a shadow rather than a blur of the drawing itself.
    ///
    /// **Flipped**: requiring the offset first (`&& seen_offset` on the blur arm)
    /// fails here on `out.skipped`, not on a wrong shadow — the chain refuses whole,
    /// which is the right failure and the reason the file would be *reported* rather
    /// than quietly missing its shadows.
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

    /// **A filter that resolves to "unchanged" is an empty stack, not a skip.**
    ///
    /// ⚠️ **The bare `<feOffset/>` exists because a filter with *no* primitives
    /// renders its element as nothing at all**, so this app's writer emits one for a
    /// stack whose every entry was invisible or neutral. Refusing it would report a
    /// loss where there is none; reading it as an effect would put a zero-offset
    /// shadow on the node. It has to mean neither.
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

    /// **Document order is stack order**, which is what lets this walk the primitives
    /// linearly instead of sorting a graph: the writer emits stage one in list order
    /// and then the shadow chains in list order, so an inner shadow written *before* a
    /// drop shadow comes back in front of it.
    ///
    /// ⚠️ **The `feMerge` at the end lists them in a different order** — behind, the
    /// layer, in front — and reading *that* for the stack would swap this pair. It is
    /// assembly, not sequence.
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

    /// **A flood's colour is read through `property` like every other presentation
    /// attribute**, which is this module's oldest lesson applied to its newest
    /// reader: `flood-color` and `flood-opacity` *are* presentation attributes, so a
    /// file writing its primitives through `style=""` — which is how Inkscape writes
    /// everything else — would otherwise hand every shadow the default opaque black
    /// and say nothing about it.
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

    /// **`display="none"` becomes a hidden layer rather than nothing at all.** The
    /// markup says the shape exists and is not shown, and the model has exactly that
    /// state — dropping it loses content a user can restore with one click.
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

    /// **The two refusals, and they are the only two.** Anything well-formed with an
    /// `<svg>` root is imported however little of it can be read — refusing a whole
    /// icon over one unsupported element would be the wrong trade.
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

    /// **`looks_like_svg` is a gate, not a parse**, and the paste path asks it of
    /// every clipboard string — so what matters is that it says no to ordinary text
    /// and yes to markup with a declaration in front of it.
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

    /// What may sit between the start of a file and its root element (§15 D671,
    /// `[S16.3-L3-04]`).
    ///
    /// The first two rows are the ones an ordinary drawing hits: a `<!DOCTYPE svg
    /// …>` is how most SVG written before about 2015 opens, and a generator
    /// comment is what Illustrator puts between the declaration and the doctype.
    /// The doctype case already passed; the comment cases did not, above a
    /// sentence claiming they did.
    ///
    /// **Two flips, both predicted correctly.** Deleting the `while let
    /// Some(rest) = head.strip_prefix("<!--")` loop fails on the second assertion,
    /// the plain generator comment. Answering `rest` rather than `""` for a
    /// comment with no `-->` fails on the fourth.
    ///
    /// ⚠️ **The fourth row had to be rewritten to make that second flip bite**, and
    /// it is the more useful half of the pair. It read *"`<!-- never closed, and
    /// then <svg/>`"*, which is refused either way — the text after `<!--` is a
    /// space, so nothing starts with `<svg` whatever the arm returns. The case the
    /// arm is actually for is a comment whose **contents open with the element**,
    /// and the difference is one character.
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

    /// ⚠️ **A `<polygon>` comes back as a `Path`, deliberately, and this is the one
    /// place the round trip is asymmetric.** `NodeKind::Polygon` is a regular *n*-gon
    /// with a radius; the writer emits `<polygon>` for one because the markup fits,
    /// but reading every point list back as one would turn an arbitrary outline into
    /// a regular hexagon. So the geometry round-trips and the *kind* does not, which
    /// is why the round-trip test asserts the first.
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

    /// ⚠️ **The root `<svg>` is a styled element like any other, and this is the
    /// assertion that says so.**
    ///
    /// Every Figma export opens `<svg … fill="none">` and lets each shape state its
    /// own paint. A reader that walks the root's *children* from SVG's initial values
    /// — the obvious spelling, and what this module did until a real export was
    /// imported — gives **black** to every child that states none, so a dashed line
    /// arrives as a filled bar and an outline icon as a silhouette.
    ///
    /// **Found by a probe over a real export, not by reading**, which is the whole
    /// argument for running one: nine hand-written fixtures above are all green
    /// against the bug, because a fixture author who is thinking about fills puts
    /// them on the shapes.
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

    /// **A skipped *attribute* is reported like a skipped element**, because each of
    /// these changes the picture rather than decorating it. `filter` is the only one
    /// of the three left — `clip-path` and `mask` became mask nodes on 2026-08-31 —
    /// and a paste that dropped a blur without saying so would leave someone hunting
    /// for a setting they never lost.
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

    /// **A `clip-path` becomes a wrapper group with a mask node under its content**,
    /// which is the whole mapping: our mask clips the run of siblings *above* it
    /// (§15 D282–D286), so `[clip, content]` in one group is SVG's clip exactly.
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

    /// A duplicate `<clipPath>` id is first-wins too (§15 D642,
    /// `[S7.2-L1-05]`).
    ///
    /// `Gradients::collect`'s `clips` map was a plain `insert` — last-wins —
    /// three lines under the comment stating the opposite rule for `ids`. The
    /// gradient half of the same defect is
    /// `a_duplicate_gradient_id_resolves_to_the_first_definition`, and it is the
    /// half with a measured browser disagreement behind it; this one is the
    /// second map in the same function and had the same spelling.
    ///
    /// ⚠️ **`clips` is filled by `if name == "clipPath" || name == "mask"`, so
    /// the same defect covered a duplicate `<mask id>`** — which neither this
    /// test's name nor the finding's says. The fixture is a `<clipPath>` because
    /// that is the cheaper one to assert a box on; the guard is one line and
    /// does not distinguish them.
    ///
    /// ⚠️ **The two sizes are 40 and 90 rather than 40 and 100**, so a failure
    /// cannot be read as "the clip was ignored and the content's own box came
    /// back" — that answer is 100.
    ///
    /// Flip: `clips.entry(id).or_insert(el)` back to `clips.insert(id, el)`. Red
    /// with the 90-square, as predicted.
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

    /// ⚠️ **The clip is transformed exactly as the thing it clips**, and the two
    /// routes to that reach it from opposite directions — which is why this drives
    /// both in one test.
    ///
    /// `clipPathUnits="userSpaceOnUse"` — the default — resolves the clip in the user
    /// space of the **referencing** element, which includes that element's own
    /// `transform`. A **shape** takes a wrapper, so its clip has to carry the
    /// transform itself; a **container** clips itself, so its clip must carry
    /// *nothing* and inherit the group's. Getting either backwards leaves a clip
    /// hanging at the origin or transformed twice, and the two fixtures below land
    /// on the same numbers by different routes.
    ///
    /// ⚠️ **The flip's predicted site was wrong, and the correction is the lesson.**
    /// Moving the transform from the clip to the wrapper was predicted to leave the
    /// clip behind at 0..40. It does not: the clip is then *correct* at 50..90, and
    /// what breaks is the **content**, which gets the transform twice — from the
    /// wrapper and from itself — and lands at 100..200. So the assertion that bites
    /// is the second one, about the thing the change was not about. *There is a
    /// correct implementation on that side too* — transform on the wrapper, content
    /// built with its own transform suppressed — and this one is chosen only because
    /// suppression would have to be threaded through every arm of `node_ignoring`.
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

    /// **A clipped container clips itself; only a clipped *shape* gets a wrapper.**
    ///
    /// Worth its own assertion because it is what a user sees: a Figma export's
    /// outermost `<g>` carries `clip-path`, so a wrapper would be a permanent extra
    /// level on top of **every** paste. A group's own first child clips the run above
    /// it inside that group, which is the same picture one layer shallower.
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

    /// ⚠️ **An element carrying both a clip and a mask gets one wrapper and one
    /// hoist, never two masks in one parent.**
    ///
    /// Two mask nodes under one parent are two *runs*, not two clips over one run:
    /// the first would clip only the empty stretch before the second, so one of the
    /// two would silently do nothing. The wrapper for the first and the hoist for the
    /// second is what keeps both live — and it is the arrangement a reader would most
    /// plausibly simplify away.
    ///
    /// **Flip**: dropping the "other attribute still pending" half of the hoist test,
    /// so a container always clips itself, fails at the very first count — three
    /// children under the wrapper instead of two, both masks in one parent.
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

    /// **A `<mask>` is an alpha mask, and it is reported when that is not what the
    /// file meant.**
    ///
    /// ⚠️ SVG's mask is a **luminance** mask: black hides, white keeps. Ours reads
    /// alpha, where an opaque black shape *keeps* — the two are opposites rather than
    /// approximations. They agree exactly on opaque white, which is what almost every
    /// real mask is, so the common case is exact and the rest says so.
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

    /// **A reference that resolves to nothing draws unclipped and says so.**
    ///
    /// SVG says such an element is **not rendered at all**. That is the wrong answer
    /// for a paste: a user who pasted a drawing and got an empty canvas has nothing to
    /// work with, where a user who got the drawing and a note has everything. *The
    /// friendlier wrong answer, reported rather than obeyed.*
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

    /// **Inside a `<clipPath>` the rule is `clip-rule`**, a different property with
    /// the same values. An element inside a clip has a `fill-rule` too and it means
    /// nothing there, so reading the wrong one fills the hole in a compound clip.
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

    /// **A real export, end to end**, which is what the probe that found the
    /// root-style bug was doing by hand. A Figma file's shape in miniature: a root
    /// `fill="none"`, an outermost `<g clip-path>`, a rounded-rect clip in `<defs>`,
    /// a compound icon with `clip-rule`, and a stroked line.
    ///
    /// **It asserts the two reports are empty**, which is the claim a per-feature test
    /// cannot make: every piece of this file is one this reader handles, so anything
    /// landing in `skipped` or `approximated` is a regression in what it covers rather
    /// than a wrong picture. *The probe is gone; what it proved is here.*
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

    /// **`<use>` is a deep copy placed by `x`/`y`**, which is the whole of it — and
    /// the fixture is an icon sprite, the shape it actually appears in.
    ///
    /// Three claims a weaker fixture would miss: the copy *is* the referenced shape
    /// (not an empty group), each instance lands at its own `x`, and the definition
    /// inside `<defs>` is still not drawn where it stands.
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

    /// **A `<symbol>` contributes its contents, and is never drawn where it stands.**
    ///
    /// The one place a `<use>` target is treated differently from any other element,
    /// and the difference is visible in the layer count: instantiating the symbol
    /// *itself* would add a level per instance, and drawing it in place would put the
    /// sprite sheet on the canvas.
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

    /// ⚠️ **A `<use>` can reference something that contains it**, which is legal
    /// markup and an infinite recursion. A browser detects the cycle and draws
    /// nothing; a depth cap is the cheap version and cannot be wrong about a real
    /// drawing, since nothing legitimate nests instances sixteen deep.
    ///
    /// **The assertion is that it terminates and reports**, not what it draws — the
    /// partial expansion is arbitrary, and pinning it would pin the cap.
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

    /// **An unresolvable `<use>` is reported rather than silently empty**, which is
    /// the same rule the unresolvable clip follows: a reference into a file that was
    /// not copied along is the commonest way a pasted fragment is incomplete.
    #[test]
    fn an_unresolvable_use_is_reported() {
        let (_doc, out) =
            imported(r##"<svg xmlns="http://www.w3.org/2000/svg"><use href="#gone"/></svg>"##);
        assert_eq!(out.skipped, vec!["use (unresolved)"]);
        assert_eq!(out.shapes, 0);
    }

    /// **An Illustrator export in miniature**, which is the shape this whole feature
    /// exists for: every shape carries `class="cls-N"` and a `<style>` block decides
    /// what the classes mean.
    ///
    /// ⚠️ **The load-bearing rule is that a class beats a presentation attribute.**
    /// It is the one piece of CSS precedence that is not obvious — a presentation
    /// attribute is *weaker* than any rule, however unspecific — and getting it
    /// backwards leaves an Illustrator file looking exactly as it did before the
    /// stylesheet existed, which is to say wrong and plausible.
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
        let fill = |id: NodeId| match &doc.get(id).unwrap().paint().fills[0].brush {
            Brush::Solid(c) => c.to_rgba8().to_u8_array(),
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

    /// **An XML comment inside `<style>` does not truncate the sheet** —
    /// `[S7.2-L1-03]`, §15 D544.
    ///
    /// `roxmltree::Node::text()` returns the first child *and only when that
    /// child is a text node*, so a comment anywhere in `<style>` ended the sheet
    /// there and a leading comment yielded nothing parseable at all. **The second
    /// case is the damaging one**: `<style><!-- … --></style>` is the legacy
    /// idiom, and one leading comment sent every classed shape in the drawing to
    /// black.
    ///
    /// ⚠️ **Black is not "no fill" — it is SVG's initial `fill`** — so the loss
    /// is invisible in exactly the way this importer's report is meant to
    /// prevent: the shape is there, painted the wrong colour, and `skipped` and
    /// `approximated` are **both empty**. That is why the emptiness of both lists
    /// is asserted here rather than taken for granted.
    ///
    /// **Every row was confirmed against Chrome** reading the same markup as
    /// `image/svg+xml` over HTTP, through `getComputedStyle(el).fill` — which is
    /// the point of the finding: both sides of a hand-written fixture agree about
    /// what SVG looks like, so only a real renderer disagrees with you.
    ///
    /// ⚠️ **The whitespace-then-comment row is not the same case as the leading
    /// comment**, and it is the one a reader would drop as redundant: it fails
    /// through a *different* branch — `text()` finds a text node, which is the
    /// newline, and returns that — so it would stay green against a fix that only
    /// handled "the first child is a comment".
    ///
    /// **CDATA is the control that pins the cause**, because roxmltree merges
    /// text and CDATA nodes: it was never affected, so the bug is comments
    /// specifically and not interleaving in general.
    ///
    /// ⚠️ **Flipped** back to `el.text()`: fails on the first row's `.b`, black
    /// against green. With that row disabled it fails on the leading-comment row,
    /// and with that one disabled too, on the whitespace-then-comment row — three
    /// separate branches, and the two controls stay green throughout.
    ///
    /// (Plain backticks rather than `[links]`, §15 D319's convention.)
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
            let fill = |id: NodeId| match &doc.get(id).unwrap().paint().fills[0].brush {
                Brush::Solid(c) => c.to_rgba8().to_u8_array(),
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

        // Controls. A trailing comment and a plain two-rule sheet always worked,
        // and CDATA is what says the cause is comments rather than interleaving.
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
        // And the initial fill is black, which is why the failure was invisible.
        assert_eq!(
            case(".a{fill:#ff0000}").1,
            BLACK,
            "an unmatched class is black"
        );
    }

    /// **Specificity, and the tiebreak.** An id beats a class beats a type, and two
    /// rules of equal weight are settled by document order — the later one wins.
    ///
    /// Worth its own test because the ordering is the half a lookup gets wrong
    /// silently: a matcher that returned the *first* match would be right about every
    /// file with one rule per element, which is most of them.
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
        let fill = |id: NodeId| match &doc.get(id).unwrap().paint().fills[0].brush {
            Brush::Solid(c) => c.to_rgba8().to_u8_array(),
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

    /// **Descendant and child combinators, and compound selectors** (§15 D806).
    ///
    /// `roadmap.md` held this as *"a CSS selector needing a combinator, which is a
    /// cascade engine rather than a lookup"*. It is a cascade engine now, of the
    /// narrow kind: type, class, id and `*`, compounded, and joined by whitespace
    /// or `>`.
    ///
    /// ⚠️ **The child combinator is asserted against a *grandchild* that must not
    /// match**, and the descendant against the same shape that must. Without the
    /// pair, a `>` implemented as a descendant walk passes — which is the whole
    /// difference between the two and the easiest thing to get wrong.
    ///
    /// ⚠️ **Specificity is asserted by the rules *losing*, not by arithmetic.**
    /// `.outer .a` has two classes and beats the bare `.a` written after it; the
    /// later-wins tiebreak is what would otherwise explain the same result, so the
    /// higher-specificity rule is deliberately written **first**.
    ///
    /// ⚠️ **Flip-check, run, twice, and both land on the same assertion.** Making
    /// `Combinator::Child` walk all ancestors like `Descendant` is red on the
    /// grandchild, cyan against black — the predicted site. Dropping the `chain`
    /// check from `Sel::matches` so only the subject is read is red on the
    /// grandchild too, with the same two colours.
    ///
    /// ⚠️ **That both flips land there says the grandchild is carrying this
    /// test**, and that nothing else here would notice either mutation: the
    /// descendant rect is inside `.outer` so a chain-less match still gives it
    /// green, and the compound has no chain to drop. Whether the assertions after
    /// the grandchild stay green is **reasoned, not measured** — it panics before
    /// them. **A single shape that must not be reached is the whole of what
    /// separates a combinator from a lookup.**
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
        // ⚠️ `shapes` walks every descendant, so the two `<g>`s are in this list
        // too — the rects are 1, 2, 4 and 5, not 0..4.
        let ids = shapes(&doc, &out);
        let fill = |id| {
            let Brush::Solid(c) = &doc.get(id).unwrap().paint().fills[0].brush else {
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

    /// 🚨 **A kilobyte of SVG used to hang the import here** (§15 D837).
    ///
    /// The selector is a long run of `g` that *does* match, led by a compound
    /// that never can. Right-to-left matching gets all the way along the run and
    /// then fails on `nope`, and the old recursive matcher re-derived that from
    /// every increasing choice of ancestors — `C(nesting, chain)`. Thirty levels
    /// was 12.6 s; a 534-byte version of this file did not finish in 120 s.
    ///
    /// ⚠️ **The assertion is that it *returns*, and there is deliberately no
    /// wall-clock in it.** A timing assertion measures the machine, which is
    /// what §15 D829 spent a session learning; an exponential matcher does not
    /// come back at all, so the suite's own timeout is both the sharper
    /// instrument and the honest one. **The flip is running this test against
    /// the recursive version, and it was run: it does not finish in 120 s**,
    /// against 0.27 s for the whole `svg_in` suite — 95 tests — with the table.
    /// That is the whole check, and it is why the fixture is sized to be
    /// hopeless rather than merely slow: a fixture tuned to "slow" would be a
    /// wall-clock assertion wearing a disguise.
    ///
    /// ⚠️ **And it must reach the matcher**, so the leading compound is `nope`
    /// rather than something absent from the sheet: `declaration` checks the
    /// property's presence and the specificity *first*, and a rule that loses
    /// either never walks the tree. The fixture asserts the shape came out
    /// black, which is what says the rule was evaluated and lost rather than
    /// skipped.
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
        let Brush::Solid(c) = &doc.get(rect).unwrap().paint().fills[0].brush else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [0, 0, 0, 255],
            "the rule is reached and loses on `nope`, which is what makes this \
             fixture exercise the matcher rather than the presence check"
        );
    }

    /// **A stylesheet is bounded by its own length, and says so** (§15 D838).
    ///
    /// `Css::declaration` walks every rule for every element for every property,
    /// so 50,000 rules over 1,000 shapes froze a paste for 24.5 s from a file
    /// that is shallow and small by both of this module's other bounds.
    ///
    /// ⚠️ **Both halves, because "capped" and "still works" are two claims.**
    /// The sheet is reported through the same channel a selector this cannot
    /// read uses, *and* the rules read before the cap still paint — a cap that
    /// dropped the whole sheet would pass an assertion about the report alone
    /// while losing every style in the file.
    ///
    /// Flip: the cap's branch disabled. Red on the report at `[]`, the
    /// predicted site, and green on the fill — the pair working as intended,
    /// since the first rule applies either way.
    ///
    /// 🚨 **The obvious flip — raising `MAX_CSS_RULES` past the fixture — is
    /// vacuous here, and it was written down as the flip before it was run.**
    /// The fixture is `MAX_CSS_RULES + 10` rules, so raising the constant raises
    /// the fixture with it and the cap trips either way: at 100,000 the test
    /// still passed, having quietly built a hundred thousand rules and taken
    /// 1.63 s to do it. **A fixture defined in terms of the constant under test
    /// cannot falsify that constant** — which is worth more than the cap is,
    /// because the same shape is available to every threshold test in this
    /// module, and the passing run looks exactly like a working one.
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
        let Brush::Solid(c) = &doc.get(ids[0]).unwrap().paint().fills[0].brush else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [0, 255, 0, 255],
            "and the rules read before the cap still apply — the first one is \
             written first for exactly this assertion"
        );
    }

    /// **The backtracking a descendant chain needs, in the one shape where a
    /// greedy walk gives a different answer** (§15 D837).
    ///
    /// 🚨 **`match_chain`'s own doc could not be used to write this test**, and
    /// §15 D806 carried the same sentence — both are corrected now, and both
    /// justified the recursion with
    /// `.a .b .c`, where greedy and backtracking **necessarily agree**, because
    /// a descendant chain's ancestor sets are nested — having found the nearest
    /// `.b`, any `.a` above a further `.b` is also above that one. The case that
    /// separates them needs a `>` to the *left* of a descendant.
    ///
    /// Here `.a > .b .c` must match: the nearest `.b` above the target has a
    /// plain `<g>` as its parent, not `.a`, so a matcher that commits to the
    /// nearest `.b` fails, while the second `.b` further up does have `.a` as
    /// its direct parent. **Flipped against a greedy walk — the descendant arm
    /// taking the first matching ancestor and not reconsidering — and it is red
    /// here**, black against green, with every other selector test green.
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
        let Brush::Solid(c) = &doc.get(rect).unwrap().paint().fills[0].brush else {
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

    /// **A selector list keeps its readable members when another member is one
    /// this importer does not support — which is what a browser does** (§15 D860,
    /// `[X7-L1-03]`).
    ///
    /// 🚨 **The finding said the opposite and a browser says this.** It held that
    /// `.a, rect:hover{fill:red}` should drop the whole rule, since *"every browser
    /// drops the whole rule"* for an invalid member — and `rect:hover` is not
    /// invalid, it is valid CSS this importer does not read. Measured in Chromium,
    /// served over HTTP per CLAUDE.md's recipe: that rule paints the rect
    /// **red**, and it is only a *syntactically invalid* member (`rect:::bad`) or
    /// an unknown pseudo-class (`rect:nonsense-pseudo`) that makes Chromium drop
    /// the rule and paint it black. So the importer already agrees with the browser
    /// on the realistic case, and the finding's sketched fix — drop the rule when
    /// any member is unreadable — would have made it disagree there.
    ///
    /// ⚠️ **What is left is the narrow case, kept by decision**: a list with an
    /// *invalid* member applies its readable members here and not in a browser.
    /// Telling invalid from unsupported needs a CSS validator this importer does
    /// not have, and exporters do not write invalid selectors. The member it could
    /// not read is still reported.
    ///
    /// **Flip-check, run** against the finding's own sketch — drop the whole rule
    /// when any member is unreadable: red, the rect black (`[0, 0, 0, 255]`),
    /// which is the answer the browser does *not* give for this file.
    #[test]
    fn a_selector_list_keeps_its_readable_members_as_a_browser_does() {
        let (doc, out) = imported(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
                  <style>.a, rect:hover{fill:#ff0000}</style>
                  <rect class="a" width="10" height="10"/>
                </svg>"##,
        );
        let rect = shapes(&doc, &out)[0];
        let Brush::Solid(c) = &doc.get(rect).unwrap().paint().fills[0].brush else {
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

    /// **`*` matches any element, which nothing asserted** (§15 D837).
    ///
    /// ⚠️ Making the universal compound a literal tag name — i.e. matching only
    /// an element called `*`, which is the shape somebody writes by forgetting
    /// the case — passed the whole suite. Here it is red: the rect is not named
    /// `*` and must still be reached through one.
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
        let Brush::Solid(c) = &doc.get(rect).unwrap().paint().fills[0].brush else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [0, 255, 0, 255],
            "`*` stands for the intermediate `<g>`, so the chain matches"
        );
    }

    /// **What a compound or a combinator refuses** (§15 D806).
    ///
    /// The parser's job is as much to say *no* as to match: a selector it
    /// half-understood would paint the wrong shapes and report nothing, which is
    /// the one thing this module's contract forbids.
    ///
    /// ⚠️ **The malformed cases matter as much as the unsupported ones.**
    /// `.a > > .b` read leniently becomes `.a > .b` — a **wider** rule than the
    /// one written — and a trailing or leading `>` has no compound to attach to.
    /// Each is refused rather than repaired.
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

    /// ⚠️ **A selector this cannot read is ignored and reported once for the sheet**,
    /// not once per element it should have matched. "Some rules were too complex" is a
    /// fact a reader can act on; "nine shapes are the wrong colour" is a symptom they
    /// would have to trace back themselves.
    ///
    /// A CSS comment costs the rule it is in, for a reason worth stating: every value
    /// here is a *slice* of the document, so stripping comments would mean copying the
    /// sheet into a string this parser does not outlive.
    ///
    /// ⚠️ **This test used `g > rect` as its unreadable example until 2026-09-19**,
    /// when §15 D806 made the child combinator readable and it went red — which is
    /// the test doing its job. The example is a **sibling** combinator now, and
    /// what is still out of reach is stated beside the parser rather than only
    /// here: `+` and `~` need sibling order, `:pseudo` needs state, `[attr]` needs
    /// attribute matching.
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
        let Brush::Solid(c) = &doc.get(id).unwrap().paint().fills[0].brush else {
            panic!("a solid fill")
        };
        assert_eq!(
            c.to_rgba8().to_u8_array(),
            [0, 255, 0, 255],
            "and the rules it *could* read still applied"
        );
    }

    /// ⚠️ **SVG places text by its baseline and this model places it by its box**,
    /// which is the whole of the arithmetic in `text_node` and the one thing that
    /// cannot be approximated from the markup.
    ///
    /// The assertion is that the box sits **above** the baseline the file names, by
    /// roughly the ascent — asserted as a band rather than a number, because the exact
    /// ascent is the bundled family's and would make this a test of Inter's metrics.
    /// **Flip**: dropping the `- ascent` — putting the box's *top* at `y`, which is
    /// the obvious wrong version — lands the box at y0 = 100 exactly and fails here,
    /// a whole line low. It leaves the anchor test green, which is worth knowing:
    /// these two tests are about the two axes and neither covers the other.
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

    /// **`text-anchor` is folded into the placement**, because the model has no
    /// anchor: an auto-width box *is* its text, so `middle` is the box shifted half
    /// its own width and there is nothing left to store.
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

    /// **A `<text>`'s own opacity, visibility and filter reach *every* run it splits
    /// into, not just the first** (`[S7.1-L1-05]`, §15 D573).
    ///
    /// A positioned `<tspan>` becomes a node of its own, and `chrome` was handed that
    /// `<tspan>` — which carries none of the three properties. Measured before the
    /// fix: node 0 got `SetOpacity 0.3` and `SetVisible false`, node 1 got **neither**
    /// — full opacity, visible — and `skipped` was empty, so a diagram pasted from a
    /// file that hides a layer arrived with half of it on screen and nothing said
    /// why. This is the treatment the run's *transform* already had for the same
    /// reason (`text_node`'s `outer`); the chrome was not moved with it.
    ///
    /// ⚠️ **The `<tspan>`'s own opacity must survive too, which is why this composes
    /// rather than replacing.** SVG nests opacity multiplicatively — the second
    /// assertion is `0.3 × 0.5` — and a fix spelled *"read the `<text>` instead"*
    /// passes the visibility half and silently drops every per-run fade.
    ///
    /// **Flip run**, `Some(text_el)` in `text_node` put back to `None`: fails on *"the
    /// `<text>`'s opacity reaches the second run"*, `1.0` against `0.3`. ⚠️ **Predicted
    /// at the *visibility* assertion and wrong for the fourth time this session** —
    /// the two are one `if` apart in the fixture and opacity is simply written first.
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

        // The `<tspan>`'s own opacity nests inside the `<text>`'s rather than being
        // replaced by it.
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

    /// **A nested `<tspan>` is a run like any other, at whatever depth**
    /// (`[S7.1-L1-04]`, §15 D575).
    ///
    /// The importer read exactly one level, so a tspan inside a tspan had its whole
    /// style dropped — no colour, no size, no weight — with `skipped` and
    /// `approximated` both empty. ⚠️ **The *text* arrived**, because the collection
    /// underneath was already `descendants()`, and that is precisely what made the
    /// loss silent: the content was right, only the styling was gone, so nothing
    /// looked broken. `fn text`'s own doc says *"nothing on a `<tspan>` is lost
    /// silently any more"* and §15 D398 closed *"the importer's last silent loss"*;
    /// neither carves out nesting.
    ///
    /// **The flat spelling is the control and is asserted in the same run**, because
    /// "three spans" is only meaningful against a reader that produces three for the
    /// markup that always worked.
    ///
    /// ⚠️ **The third case is the one reading would not predict.** Interleaved text
    /// — `x<tspan …>y</tspan>z` inside a tspan — has `z` belonging to the **outer**
    /// tspan, which a per-element walk cannot express at all: it would either give
    /// `z` the inner style or drop it. One piece per *text node* is what buys that,
    /// and it is asserted so a later simplification back to per-element has something
    /// to fail on.
    ///
    /// **Flip run**, `text_pieces` reverted to the old shape — one piece per
    /// element, its text collected with `descendants()`: fails on *"a nested tspan
    /// carries its own style"* with `Spans([])` against the flat spelling's three
    /// spans. **The finding's own measurement, reproduced by the flip**, which is the
    /// strongest form this check takes.
    ///
    /// ⚠️ **The first attempt at that flip was a different bug and said so
    /// immediately.** Dropping the recursion and collecting `children()` instead of
    /// `descendants()` loses the nested *text* as well, so the run failed on *"the
    /// content was never the half that was lost"* at `"a"` against `"ab"` — a fixture
    /// assertion, three lines above the interesting one. Faithful in colour and
    /// wrong in mechanism: it proved a reader that drops nested text is broken, which
    /// nobody doubted. **Write the flip as the code that actually shipped**, which
    /// here means keeping `descendants()`.
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

        // A nested *positioned* tspan places a second run, where the one-level
        // reader gave one node for markup SVG puts in two places.
        let (_, _, made) = spans_of(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20">a<tspan><tspan x="0" y="60">b</tspan></tspan></text>
               </svg>"#,
        );
        assert_eq!(made, 2, "a nested positioned tspan is still a break");
    }

    /// **Text after a nested `<tspan>` belongs to the tspan it is *in*, not to the
    /// one that just closed** (§15 D575).
    ///
    /// The case that decides the shape of `text_pieces`: pieces are one per **text
    /// node**, not one per element, so an element's own characters can be split
    /// around a child's. A per-element walk has nowhere to put `z` — it would have to
    /// give it the inner tspan's style or lose it — and no amount of recursion fixes
    /// that, which is why this is asserted rather than left to the reader.
    ///
    /// **Flip run**, `text_pieces` emitting one piece per element with its text
    /// collected by `descendants()` — the old shape: fails on *"y reads the inner
    /// one's — the control"*, 12 against 40.
    ///
    /// ⚠️ **Predicted at the tail assertion and wrong, and the correction is the
    /// useful part.** Under that flip `x`, `y` and `z` are one piece carrying the
    /// *outer* tspan's style, so the tail assertion is **green** and only the control
    /// bites. So this test's third assertion is not falsified by the per-element
    /// flip at all; what it pins is the *ordering* a per-text-node walk gives, and
    /// nothing simpler than the whole shape produces the wrong answer for it. Left
    /// standing anyway, because a future simplification that got the sizes right and
    /// the order wrong would have nothing else to fail on — but recorded here so
    /// nobody reads it as the assertion with teeth. **The control is the one with
    /// teeth**, which is the opposite of what writing it suggested.
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

        // The size each character resolves to, read through `Spans::resolve` — the
        // same door the shaper uses — rather than by reading the span list back.
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

    /// ⚠️ **A `<tspan>` with its own position starts a new layer; one without joins
    /// the text.** The rule that tells two exporters apart: this app writes one
    /// `<text>` per line with a `<tspan>` per *style run* (§15 D81), and Illustrator
    /// writes one `<text>` with a `<tspan x y>` per *line*. Reading the element
    /// rather than the position would split ours and merge theirs.
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

    /// **An unpositioned `<tspan fill>` becomes a character span over its own
    /// bytes**, which is the round trip closing for this app's own writer: §15 D81
    /// emits one `<tspan>` per style run and §15 D154 is the per-run colour it
    /// emits them for, so reading them back as `CharSpans` is reading our own
    /// spelling rather than inventing one.
    ///
    /// Three things are pinned here and each fails differently:
    ///
    /// - the range is the run's bytes and not the whole node — the assertion is
    ///   `4..7` and not "some span exists";
    /// - a **trailing space** inside the coloured run is trimmed off the content,
    ///   so the range has to be clamped to what survived. ⚠️ Dropping the `min` in
    ///   `text_node` leaves a span ending at 8 over a 7-byte string, and the second
    ///   case is the one that catches it;
    /// - a `<tspan>` **repeating** the node's own colour mints nothing.
    ///
    /// ⚠️ **That third case is about `ink`, not about `Spans::normalize`, and the
    /// flip is what said so.** The obvious reading is that a repeated colour makes a
    /// span which normalize then drops as equal to the default — so reverting
    /// `text_node`'s `TextStyle::set` to a raw field assignment (which leaves the
    /// default at `f32` and every span canonicalized to `u8`) ought to strand an
    /// invisible span. **It leaves every assertion here green**: `text` compares the
    /// two runs through `ink`, at `to_rgba8` precision, and never pushes the range at
    /// all. Normalize is never reached. The last assertion below is the one that
    /// bites on that flip, and it is about the *stored* alpha rather than about
    /// spans: `fill-opacity="0.5"` is 128/255 once quantized, which is the only value
    /// the opacity control can reproduce (`typography::canonical_color`).
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
        // The fixture is only about spans if it is one node holding all three runs.
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

        // A trailing space inside the coloured run: the content loses it, so the
        // span must lose it too rather than running one byte past the end.
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

        // The same colour twice is not a span — it is the default said again.
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
        // The stored alpha is quantized, not the 0.5 the file spelled: 0.5 is a
        // value no control here can express, and a span set from the picker would
        // never coalesce with it. This is the assertion the `TextStyle::set` flip
        // fails — every other one above it stays green.
        let alpha = default.expect("an ink").components[3];
        assert!(
            (f64::from(alpha) - 128.0 / 255.0).abs() < 1e-6,
            "fill-opacity quantized to 1/255, got {alpha}"
        );
    }

    /// **A `<tspan>`'s font scope becomes spans too — size, weight, italic and
    /// family** (§15 D398). Colour landed first (D396) and left these four dropped
    /// *silently*, which was the one thing this module's header promised never
    /// happens.
    ///
    /// The four are compared independently, which is the case worth a fixture: a
    /// `<tspan>` that states a size and inherits its weight must produce one span,
    /// not a whole style. The assertion is therefore on the exact **set** of kinds.
    ///
    /// ⚠️ **The family is not a comparison like the other three.** A `font-family` is
    /// a preference *order*, so it goes through `Builder::pick_family` — the same
    /// path the node's own default takes, deliberately, since two copies of that
    /// choice are how a node and its runs would come to substitute differently. A run
    /// naming only families this machine lacks gets **no span and a report**, and the
    /// third case pins that: a span there would silently re-point the run at
    /// whatever the list's head happened to be.
    ///
    /// Flips, all three biting. Dropping the `Weight` push fails the first case with
    /// `[(4, 7, Size)]` against both. Making `Builder::pick_family` take the list's
    /// head regardless of availability fails the third **and two older tests with
    /// it** — `an_unavailable_family_is_substituted_and_said_out_loud` and
    /// `font_properties_inherit_and_reach_a_positioned_tspan` — which is the return
    /// on the node and its runs sharing one picker instead of owning two.
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

        // A run that states a size and a weight and inherits everything else: two
        // spans over the same bytes, not four and not one.
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
        // ⚠️ **20, not 16.** `Style::default().font_size` is 16 — SVG's initial
        // `medium`, which this model shares — so asserting 16 here would pass against
        // an importer that never read the `<text>`'s `font-size` at all, and the span
        // below would still be minted off a `run_style` of 16. A non-default number is
        // what makes this line discriminate.
        assert_eq!(style.font_size, 20.0, "the node keeps the <text>'s size");
        let mut list = kinds(&doc, id);
        list.sort_by_key(|e| format!("{:?}", e.2));
        assert_eq!(
            list,
            [(4, 7, CharAttrKind::Size), (4, 7, CharAttrKind::Weight),],
            "exactly the two the run stated, over exactly its bytes"
        );

        // Italic on its own, to prove the three are independent rather than one
        // bundle that happens to be keyed by the first of them.
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

        // ⚠️ A family nothing here has: reported, and *no* span — the run keeps the
        // node's face rather than being re-pointed at a name that resolves to
        // nothing.
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

    /// **A span covering every byte is hoisted onto the node** (§15 D398), because a
    /// default is what it is.
    ///
    /// ⚠️ **This is not tidiness, and this app's own output is the case.** §15 D81
    /// writes one `<tspan>` per style run with the font properties **on the tspan**,
    /// so before the hoist, re-importing our own single-run line produced a node
    /// saying weight 400 with a `Weight(500)` span lying over the whole string. It
    /// renders identically, which is exactly why nothing would ever have surfaced it —
    /// it was found by `svg_roundtrip`'s colour test counting **two** spans where it
    /// expected one.
    ///
    /// The pair below is the discriminating one: the same markup with the run
    /// covering *part* of the content keeps its span, so this pins a hoist rather
    /// than "font-weight spans get eaten".
    ///
    /// Flips: dropping the `clear` leaves the span in place equal to its own default
    /// (fails `spans.is_empty()`); dropping the whole block fails one line earlier, on
    /// the node's weight reading 400.
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

        // Full coverage: one run, and it is the whole string.
        let (weight, spans) = node(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" font-weight="400"><tspan font-weight="700">ab</tspan></text>
               </svg>"#,
        );
        assert_eq!(weight, 700, "the run's weight became the node's");
        assert!(spans.is_empty(), "and left nothing behind: {spans:?}");

        // Two runs that agree: `normalize` merges them before this sees them, so they
        // reach the hoist as one span over everything.
        let (weight, spans) = node(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" font-weight="400"><tspan font-weight="700">a</tspan><tspan font-weight="700">b</tspan></text>
               </svg>"#,
        );
        assert_eq!(weight, 700, "two runs agreeing are still one default");
        assert!(spans.is_empty(), "{spans:?}");

        // Partial coverage: the control, and the reason this is a hoist rather than a
        // rule about weights.
        let (weight, spans) = node(
            r#"<svg xmlns="http://www.w3.org/2000/svg">
                 <text x="0" y="20" font-weight="400">a<tspan font-weight="700">b</tspan></text>
               </svg>"#,
        );
        assert_eq!(weight, 400, "the node keeps its own weight");
        assert_eq!(spans.len(), 1, "and the run keeps its span: {spans:?}");
        assert_eq!((spans[0].start, spans[0].end), (1, 2));
    }

    /// **A size the model will not hold is clamped on the node *and* on the span, so
    /// the two still agree** (§15 D398).
    ///
    /// ⚠️ **This is the trap the colour work walked into from the other side.**
    /// `TextStyle::set` clamps a size into `MIN_FONT_SIZE..=MAX_FONT_SIZE` and a span
    /// goes through that clamp whatever the node does — so a node default assigned
    /// *raw* is a default a span can never equal once the file states something out
    /// of range, and `Spans::normalize` keeps an override of a value identical to the
    /// one under it.
    ///
    /// Flip: build `ts` with the struct literal it used to have
    /// (`font_size: style.font_size`) and this fails — **on the first assertion,
    /// `left: 0.1, right: 1.0`, and the span assertion is never reached**. The
    /// prediction written here first was that it would fail on the *span*, which is
    /// the interesting half; the node's own size gives way one line earlier, and the
    /// stranded span is real but unwitnessed. Left in this order deliberately: the
    /// two assertions are one claim, and a reader who only sees the first should
    /// still be looking at a node whose size the model agrees to hold.
    ///
    /// Both numbers here are under `MIN_FONT_SIZE`, which is what makes them equal
    /// *after* clamping and different before — two sizes that are merely small
    /// would test nothing.
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

    /// **A per-run *gradient* is still a loss and still says so.** `CharAttr::Color`
    /// holds a colour, so the one fill a `<tspan>` can state that a span cannot carry
    /// is a paint server — and the report is what stops that being silent.
    ///
    /// ⚠️ **Stated, not merely resolved.** A gradient the run *inherits* from the
    /// `<text>` is the run's own paint and loses nothing by joining it; reporting on
    /// the resolved style alone would call every unstyled `<tspan>` under a
    /// gradient-filled `<text>` an approximation.
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

        // ⚠️ **A gradient on the `<text>` itself is a *node*-level loss, and it is
        // reported once rather than twice.** The run inherits it and loses nothing by
        // joining — no `per-run` report, no span — while the node cannot hold it at
        // all and says so. Asserting only "something was reported" would pass against
        // the version that reports the run as well, which is the double-count the
        // `positioned` shortcut in `text` exists to avoid.
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

        // ⚠️ **A *positioned* `<tspan>` with a gradient — the input that had no
        // fixture, and whose absence made a live line look inert.** It becomes a node
        // of its own, so the loss is that node's and the report comes from
        // `text_node`; the `positioned` shortcut in `text` is what stops the run arm
        // reporting the same gradient a second time. Exactly one, from two layers.
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

        // ⚠️ **The `style=""` spelling, which is the module's own standing trap.**
        // Inkscape writes the whole presentation block there, so reading the run's
        // fill with `has_attribute` would report nothing and drop the gradient
        // silently — the exact shape that cost this module its blurs. Reading it
        // through `property` is what keeps the two spellings one question.
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

    /// **XML whitespace collapses**, which is not optional for pretty-printed markup:
    /// a `<text>` written across three indented lines carries newlines and tabs that
    /// are layout in the *file* and nothing in the drawing.
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

    /// **A no-break space survives the import** — §15 D660, `[S7.1-L1-08]`.
    ///
    /// 🚨 **`push_collapsed` asked `char::is_whitespace`, which is Unicode's
    /// `White_Space` property and twenty-three characters**, while its own doc
    /// comment said *"XML whitespace collapsing"* — XML's `S` production is four,
    /// and CSS's `white-space: normal` collapses the same four. So the code was
    /// wider than both specifications and than the sentence above it, and four of
    /// the nineteen extras are exactly the characters an author reaches for
    /// *because* they are not ordinary spaces. A pasted price, measurement or date
    /// came back breakable and nothing was reported.
    ///
    /// **Four separators and two controls**, and the controls are what make this a
    /// test about the *predicate* rather than about NBSP. `\u{feff}` was always
    /// preserved — it carries no `White_Space` property — so it says nothing on
    /// its own; the ordinary newline-and-indent case above still has to collapse,
    /// or the fix would have been "stop collapsing", which is the other wrong
    /// answer and is the one a pretty-printed file punishes.
    ///
    /// ⚠️ **The leading ideographic space is the sharpest of the four**, and it
    /// tests a different line: `text_node`'s two trims, not `push_collapsed`. A
    /// `str::trim` on the wider set removed it *outright* rather than collapsing
    /// it to a space, so the two had to move together — and `lead`, which re-bases
    /// every span's byte range onto the trimmed string (§15 D396), is why they
    /// must not drift.
    ///
    /// ⚠️ **Flip run, twice, and the two flips fail different assertions — which
    /// is what says the fix really is two lines rather than one.** Restoring
    /// `c.is_whitespace()` in `push_collapsed` fails the **NBSP** assertion, being
    /// first, at `"10 km"`. Restoring the wide `trim()`/`trim_start()` in
    /// `text_node` leaves that green and fails the **ideographic** one at `"a"`
    /// against `"\u{3000}a"`. Neither flip touches the two controls.
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

        // The controls. One says the rule was never narrower than
        // `char::is_whitespace` to begin with; the other says it is still wide
        // enough to do its job.
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

    /// **A family this machine does not have is substituted and reported**, rather
    /// than stored as a name that shapes as something else. A `font-family` list is a
    /// preference order, so the first *available* one wins — taking the head
    /// regardless would shape every foreign file in a family the document has no
    /// bytes for.
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

    /// ⚠️ **The font properties inherit, and the two ways they reach a run are the
    /// same two the Wikipedia test card uses in one file** — which is how this bug was
    /// found and why the fixture is shaped like that file rather than minimally.
    ///
    /// **Three routes to a run's font size, and the fixture drives all three**: stated
    /// on the `<text>`, stated on a positioned **`<tspan>`**, and inherited from an
    /// enclosing `<g>` that the first two override.
    ///
    /// ⚠️ **A fixture with only the first two proves nothing about inheritance, which
    /// is how the first draft of this test was wrong.** Flipping `ts.font_size` back to
    /// `property(el, "font-size", …)` left it green, because a positioned `<tspan>`
    /// *is* the element by then and it states the size itself — so the flip that was
    /// supposed to prove the fix could not tell the two halves apart. The `<g>` and
    /// the third `<text>` are what discriminate them, and with them the same flip
    /// fails on the inherited box.
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
        // Offset by the difference in their anchors, which is the drop shadow the
        // test card is drawing.
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

    /// ⚠️ **A `<!DOCTYPE>` is not a parse error, and this is the most expensive thing
    /// in this module to get wrong.**
    ///
    /// roxmltree refuses a DTD by default, and every SVG 1.1 file written by Inkscape,
    /// Illustrator or anything of that era opens with one — Wikipedia's own test card
    /// does. The failure is **whole-file**: not a missing element but `SvgError::NotXml`,
    /// so the paste falls through and the user gets their markup as a text layer with
    /// nothing saying why.
    ///
    /// **No fixture written by hand has a doctype in it**, which is exactly why
    /// thirty-eight green tests said nothing about it. It was found by running the
    /// importer over the real file — while looking for something else.
    ///
    /// **Flip**: `allow_dtd: false` gives `Err(NotXml)` here.
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

    /// A bomb of nested `<g>`s is reported, not survived by luck — and the tree
    /// it leaves behind is one the loader will read back.
    ///
    /// ⚠️ **Two separate things go wrong past the bound and the test asserts
    /// both.** The walk `children → node → node_ignoring → children` is
    /// recursive and aborted a debug build at depth 200 with
    /// `STATUS_STACK_OVERFLOW` — not a panic, so nothing catches it and the
    /// session goes with the process. And the *document* the walk builds must
    /// stay inside `crate::io::MAX_TREE_DEPTH`, or the import would apply
    /// cleanly, save cleanly and then refuse to reopen.
    ///
    /// **The shallow half is the control**, and it is the assertion that would
    /// catch a fix that simply refused deep files: 30 levels is an ordinary
    /// export and must come through with its shape and no report at all.
    ///
    /// ⚠️ **`shapes()` cannot be used here** — the module's own helper walks the
    /// result recursively, so a test that used it would be measuring the
    /// helper's stack rather than the importer's. The depth below is counted
    /// iteratively for the same reason.
    ///
    /// **Flip-check, run, and it does not fail — it takes the harness with it.**
    /// With `MAX_SVG_NESTING` raised to 100,000 this test does not report an
    /// assertion; `cargo test -p ondin-core --lib` exits `0xc00000fd`,
    /// `STATUS_STACK_OVERFLOW`, on the 200-level fixture, in a debug build. That
    /// is the whole argument for the bound: **there is no failure to catch**, no
    /// message and no stack trace, which is why the 200 here is not a round
    /// number but the depth the abort was measured at.
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

        // The bomb: reported, and bounded. ⚠️ **90 and not the 200 the doc below
        // measured at**, because 200 is refused whole by `MAX_XML_DEPTH` now,
        // before the builder is reached (§15 D851) — and this test is about the
        // builder. 90 sits between the two bounds, which is the only band where
        // the builder's own skip can still be seen.
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

    /// **`<use>`'s depth cap bounds nesting and not branching**, so the budget is
    /// what stops a wide bomb.
    ///
    /// A `<defs>` of L groups each instantiating the next *twice*, plus one
    /// `<use>` at the root, expands 2^L from a file growing ~50 bytes per level.
    /// Measured before `MAX_SVG_NODES` existed, release, through `import`: L=10
    /// is 635 bytes and 1,024 shapes, L=12 is 739 and 4,096, **L=14 is 843 bytes
    /// and 16,384 shapes in 7.99 s** — and `use_depth`'s cap of 16 admits 65,536.
    /// Three `<use>` per level instead of two is 3^16 ≈ 43 million, which is an
    /// allocation failure, which is an **abort**.
    ///
    /// ⚠️ **The budget counts *nodes*, not shapes, and the two are far apart
    /// here**: L=14 now stops at 9,997 shapes, because each `<use>` level also
    /// creates a group and the groups spend most of the 50,000. That is the right
    /// denomination — a group costs what a shape costs — but it means the number
    /// in `Import::shapes` after a refusal is not the budget.
    ///
    /// ⚠️ **The control is the half that matters.** A budget low enough to stop
    /// the bomb and low enough to break real artwork would look identical from the
    /// bomb's side alone, so L=10's 1,024 shapes are asserted to come through
    /// whole and unreported.
    ///
    /// ⚠️ **Flipped** by setting `MAX_SVG_NODES` to `usize::MAX`: fails on the
    /// `skipped` assertion with `[]` — predicted there and observed there — and
    /// the control stays green, which is what says the control is about the
    /// budget's *value* rather than about its existence.
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
        // The control first: an ordinary amount of instancing comes through whole.
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

    /// **A gradient stop whose offset is `nan` still saves and reloads.**
    ///
    /// The whole round trip, because the loss is at the far end of it: import →
    /// `Document::apply` → `io::save` → `io::load`. A `NaN` offset reaches
    /// `ColorStop`, `serde_json` writes it as `null`, and the file comes back
    /// *"invalid type: null, expected f32"* — the paste opens fine and the
    /// document is dead the next time it is asked for.
    ///
    /// ⚠️ **`f64::clamp` is why this looked guarded**: it is right about `+inf`
    /// and `-inf` — `<stop offset="1e999"/>` really does clamp to 1.0 — and
    /// propagates NaN by definition. A guard correct on two of its three
    /// non-finite inputs reads as a guard.
    ///
    /// ⚠️ **The controls are the point of the test.** The two infinities are
    /// asserted to come through *clamped* rather than dropped, so this stays a
    /// test about NaN and does not silently become a test about `is_finite`
    /// rejecting everything — which is what a fix one step too broad would do,
    /// and which would move every `1e999` stop from 1.0 to 0.0.
    ///
    /// ⚠️ **Flipped** by removing `.filter(|f| !f.is_nan())` from `stop_of` —
    /// the predicate that is *there*, not the `is_finite` that was tried and
    /// rejected, since in a test whose whole subject is that those two are not
    /// interchangeable one wrong word sends the next reader round the loop.
    /// Fails on the `io::load` assertion with serde's *"invalid type: null,
    /// expected f32"* — the loss, and the predicted site. Asserting the offset
    /// directly would have failed one line earlier on `NaN != 0.0`, which is the
    /// mechanism and says nothing about what the user lost.
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

        // Controls: the infinities are clamped, not dropped, and were correct
        // before this fix. If either of these moves, the filter went too wide.
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

    /// **A descending stop list is clamped up on import, which is SVG's own rule.**
    ///
    /// `[S11.1-L1-02]`, the importer's half of §15 D455. SVG: *"each gradient
    /// offset value is required to be equal to or greater than the previous"*,
    /// and a smaller one is corrected up — so `[0, 0.8, 0.2, 1]` means
    /// `[0, 0.8, 0.8, 1]`, with the third stop collapsing onto the second. Stored
    /// verbatim, that ramp rendered as a **flat fill of the first stop's colour**
    /// in all 100 columns, which `ondin-export`'s
    /// `a_gradient_with_descending_stops_draws_the_ramp_svg_says_it_means`
    /// measures.
    ///
    /// ⚠️ **This is the door that makes the *model* right, and it is not the same
    /// guard as the render boundary's.** `color::brush_to_backend` applies the
    /// identical rule so that a hand-edited `.ondin` and every file imported
    /// before today draw correctly; this is what makes a re-export round-trip and
    /// what the layers panel and the paint editor read. Neither covers the other,
    /// and the pixel test above is green with *this* call deleted.
    ///
    /// ⚠️ **It is a clamp and not a sort**, and `panels::paint::with_stops`'
    /// `sort_by` is deliberately left alone: dragging a stop handle past its
    /// neighbour means *"put it there"*, while a file that says `0.2` after `0.8`
    /// is a file to be read. The two disagree on exactly this input — a sort gives
    /// `[0, 0.2, 0.8, 1]`, a third picture — which is why the control below
    /// asserts the clamped answer rather than merely "monotonic".
    ///
    /// ⚠️ **Flipped** by removing the `make_stops_monotonic` call in
    /// `Gradients::collect`: fails on stop 2, `0.2` against `0.8`.
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

        // The control: an ascending list is untouched, so nothing here reorders
        // or collapses a legitimate ramp.
        let ok = svg.replace("offset='0.2'", "offset='0.9'");
        assert_eq!(all_stop_offsets(&imported(&ok).0), vec![0.0, 0.8, 0.9, 1.0]);
    }

    /// **An unreadable `fill` leaves the inherited paint standing and says so**
    /// (§15 D493, `[S7.1-L1-03]`).
    ///
    /// Two independent halves, and the test carries both because either alone
    /// leaves a real defect. `!important` was handed to `parse_paint` verbatim, so
    /// `fill:#ff0000 !important` — which is what a stylesheet with any priority in
    /// it writes — imported the shape with **zero** fills, invisible, with both
    /// report lists empty. And the two paint arms wrote the *result* of the parse
    /// into the style, where `None` is `Style`'s spelling for **unpainted**, so any
    /// value peniko refuses destroyed the inherited paint too: `var(--brand)`
    /// inside a green group came back with no fill at all, where CSS drops the
    /// invalid declaration and the green stands.
    ///
    /// ⚠️ **Every other arm of `resolve` was already written the safe way.**
    /// `stroke-width`, `stroke-dashoffset`, `stroke-miterlimit`, `fill-opacity`,
    /// `font-size` and the rest all read `prop(…).and_then(parse_len)`. `fill` and
    /// `stroke` were the two that put the parse inside the assignment.
    ///
    /// ⚠️ **`fill='none'` is the control and it is what stops this over-reaching.**
    /// `parse_paint` answers `None` for it too, and there it means exactly what it
    /// says. The two are told apart by the *text*, which is the only place the two
    /// meanings are still distinguishable once the parse has run.
    ///
    /// ⚠️ **Two flips, run, one per half.** Removing the `!important` strip fails
    /// the first assertion at `[0, 0, 0]` against `[255, 0, 0]` — **black, not
    /// empty**, because the other half now leaves the inherited paint, and SVG's
    /// initial fill is black. Returning the `fill` arm to a plain assignment fails
    /// on *"the green survived"*. Neither half masks the other; each fails its own
    /// assertion.
    ///
    /// ⚠️ **This test was first inserted below, anchored on the *next* function's
    /// name, and took that function's fifty-eight-line doc comment with it** —
    /// `CLAUDE.md`'s "anchor above, never below" trap, committed by the session
    /// that had written up its eighth instance hours earlier. `cargo` caught this
    /// one only because the displaced `#[test]` landed on a doc comment and became
    /// a duplicate attribute; had the neighbour been an ordinary `fn`, every gate
    /// would have been green.
    #[test]
    fn a_paint_this_reader_cannot_parse_leaves_the_inherited_one_and_says_so() {
        let red = |doc: &Document, id: NodeId| match &doc.get(id).unwrap().paint().fills[..] {
            [f] => match &f.brush {
                Brush::Solid(c) => c.to_rgba8().to_u8_array(),
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

        // A value this reader genuinely cannot read leaves the inherited paint
        // standing — CSS's own rule for an invalid declaration — and says so.
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

        // The control, and it is the one that stops this over-reaching: `none` is
        // a value `parse_paint` also answers `None` for, and it means *unpainted*.
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

    /// **A gradient's payload is bounded, and it is the third quantity the
    /// importer bounds.**
    ///
    /// `[S7.2-L5-02]` (§15 D459). Every shape naming a gradient gets its **own**
    /// copy of the whole stop list, and not a transient one: the copies are in the
    /// `Vec<Operation>` the paste applies, so they end up in the `Document` and
    /// then in the JSON `io::save` writes. *S* stops × *K* shapes × 24 bytes,
    /// exactly quadratic — measured in release at 452 KB of source holding
    /// **864 MB**, with a solid-fill control at the same shape counts growing
    /// linearly beside it.
    ///
    /// ⚠️ **Neither existing bound sees it, and that is the point of the entry.**
    /// `roxmltree`'s `nodes_limit` bounds the *XML* at 500 000 nodes: the 452 KB
    /// fixture is 12 001 elements, 2.4% of its budget, and the limit as written
    /// permits 250 000 × 250 000. D446's `MAX_SVG_NODES` bounds what is *emitted*
    /// at 50 000, against this fixture's ~6 001,
    /// which here is unremarkable. What explodes is the payload **per** node.
    ///
    /// ⚠️ **Asserted as the stop count in the document, not as a wall clock.** The
    /// loss is memory that is kept, so the honest assertion is on what the
    /// transaction holds; a timing bound would pass on a fast machine with the cap
    /// removed at this fixture size, and the size that would not pass is one this
    /// test cannot afford to build. The count is what is quadratic.
    ///
    /// ⚠️ **And it is reported**, which is `[S7.2-L5-02]`'s G12 half: a gradient
    /// that was resampled is *there and slightly wrong*, so it is `approximated`
    /// rather than `skipped`, and it is said once for the file rather than once
    /// per shape.
    ///
    /// ⚠️ **Flipped** by deleting the resample block in `Gradients::collect`: fails
    /// on the stop-count assertion with 600 against 256, and the `approximated`
    /// assertion behind it would have failed too. The control — a gradient under
    /// the cap, kept exactly, with nothing reported — stays green under the flip,
    /// which is what says the cap does not touch an ordinary file.
    ///
    /// ⚠️ **And the before-and-after was measured under the same flip**, on the
    /// finding's own *S* stops × *S* rects fixture, timing `import` alone in
    /// release with a solid-fill control beside it:
    ///
    /// | S | source | import, before | after | control | stops held, before → after |
    /// | --- | --- | --- | --- | --- | --- |
    /// | 1 000 | 104 KB | 9.6 ms | 3.9 ms | 1.2 ms | 22 MB → 5 MB |
    /// | 2 000 | 210 KB | 34.1 ms | 7.0 ms | 2.5 ms | 91 MB → 11 MB |
    /// | 4 000 | 421 KB | 127 ms | 13.8 ms | 5.0 ms | 366 MB → 23 MB |
    /// | 6 000 | 633 KB | **284 ms** | **20.9 ms** | 7.6 ms | **823 MB → 35 MB** |
    ///
    /// The "before" column reproduces `[S7.2-L5-02]`'s own measurements (9.4 /
    /// 35.0 / 133 / 288 ms) closely enough to say the finding re-runs, so the two
    /// halves of the table are comparable rather than two machines' numbers. What
    /// changed is the **shape**: ×4 per doubling becomes ×2, matching the control.
    ///
    /// ⚠️ **The control is the reason this is not also a timing test.** Run through
    /// `Document::apply` rather than `import` alone, the *solid-fill* control is
    /// itself quadratic — 53 / 206 / 826 / 1 869 ms at the same four sizes, within
    /// 5% of the gradient case — because `op_create` resolves an unnamed node's
    /// name through `child_names(parent, &[])`, a scan of every sibling. That is a
    /// separate, larger finding, and this fixture measured it independently
    /// without meaning to. A wall clock over the pair would be asserting on it.
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
        // The ramp's *shape* survives, which is what resampling buys over
        // truncating: the ends are the ends.
        let offs = all_stop_offsets(&doc);
        assert!(
            offs[0] == 0.0 && (offs[MAX_GRADIENT_STOPS - 1] - 1.0).abs() < 1e-6,
            "the resample must span the whole ramp, got {}..{}",
            offs[0],
            offs[MAX_GRADIENT_STOPS - 1]
        );

        // **The control.** A gradient under the cap is kept exactly and nothing is
        // reported, so this is a bound on generated files rather than a tax on
        // ordinary ones.
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
                    if let crate::Brush::Gradient(g) = &f.brush {
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
                    if let crate::Brush::Gradient(g) = &f.brush
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

    /// **A `clip-path` that references its own `<clipPath>` terminates.**
    ///
    /// The recursion is `masked → children → node → node_ignoring → masked`: the
    /// def's children are read with `ignore = &[]`, so a child's own `clip-path`
    /// resolves to the `<clipPath>` currently being expanded. `<use>` and gradient
    /// `href` each carry their own cycle guard with their own named test
    /// (`a_recursive_use_stops_rather_than_recursing_forever`,
    /// `a_gradient_href_cycle_stops_rather_than_recursing_forever`); this third
    /// reference kind had neither, and a stack overflow is an **abort, not a
    /// panic** — `import`'s `Result` never returns and nothing can catch it.
    ///
    /// ⚠️ **No new guard was needed and that is the finding.** `[S7.1-L5-01]`
    /// reproduced this as a 218-byte paste aborting the release build with
    /// `STATUS_STACK_OVERFLOW`, and proposed a `depth` counter on `Builder`. That
    /// counter was then written for a *different* finding — `[A3-L5-02]`'s
    /// `MAX_SVG_NESTING`, on `children`, which is the one choke point both routes
    /// pass through — and closed this one on the way past. **Both forms are
    /// asserted here rather than assumed**, because "the fix for A ought to cover
    /// B" is exactly the claim that deserves a test rather than a sentence.
    ///
    /// ⚠️ **Flipped** by raising `MAX_SVG_NESTING` to 100,000: the test binary dies
    /// with `has overflowed its stack`, exit code `0xc00000fd`,
    /// `STATUS_STACK_OVERFLOW` — the reported symptom in the reporter's own words.
    /// Note what that means for the failure *site*: there is none. The process is
    /// gone before any assertion runs, so this test can only ever fail by aborting,
    /// and a reader debugging it should expect no assertion message at all.
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

    /// Run `f` on a thread with the **1 MiB** stack the shipped binary reserves
    /// for its main thread (read from `ondin.exe`'s PE header; nothing sets
    /// `/STACK`) — which is where `paste_svg` runs. A test thread's default is
    /// larger, so a test on it would pass a depth the app cannot survive.
    fn on_the_main_threads_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(f)
            .expect("spawn")
            .join()
            .expect("the import returned rather than overflowing")
    }

    /// **Nesting the parser cannot survive is refused before it is asked** (§15
    /// D851, `[X7-L1-01]`).
    ///
    /// 🚨 **This test could not be written as an assertion before the guard**,
    /// and that is the argument for the guard: 2,000 nested `<g>` — 34 KB —
    /// aborted the process with `STATUS_STACK_OVERFLOW` inside `roxmltree`, and a
    /// test that overflows takes its whole binary with it.
    ///
    /// ⚠️ **Flip-check, run**: the `nests_deeper_than` refusal removed — the test
    /// binary dies with `STATUS_STACK_OVERFLOW` rather than failing, which is the
    /// reported symptom, and the only way this failure can be observed at all.
    #[test]
    fn markup_nested_past_the_parsers_depth_is_refused_before_the_parser() {
        let answer = on_the_main_threads_stack(|| {
            let mut ids = IdSource::new(1);
            let root = ids.mint();
            import(&nested(2_000), &mut ids, root, 0, None).map(|_| ())
        });
        assert_eq!(answer, Err(SvgError::TooDeep));
    }

    /// **At the bound, the document still imports — on the main thread's stack,
    /// in whichever profile runs this** — with what lies past the builder's own
    /// `MAX_SVG_NESTING` skipped and reported as it always was.
    ///
    /// This is the half that says the bound is *survivable* and not only a
    /// number: release was measured surviving 1,200 levels on 1 MiB, and the debug
    /// profile, whose frames are larger, is the one this is most likely to run in.
    ///
    /// ⚠️ **Flip-check, run**: `MAX_XML_DEPTH` lowered below the fixture fails at
    /// *"at the bound it imports"*, `Err(TooDeep)`.
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

    /// **What the depth scan steps over, and the one thing it counts that a tag
    /// count cannot see** (§15 D851).
    ///
    /// A `<` inside a comment, a CDATA section, a processing instruction or a
    /// quoted attribute value is not a tag, and a self-closed element does not
    /// nest — each would refuse a file that parses at depth one. Twenty thousand
    /// *siblings* are the control that the scan measures depth rather than size.
    /// ⚠️ **And an entity**: a doctype can declare one whose value is markup, and
    /// `roxmltree` expands references ten deep, so a shallow document can parse
    /// deep. The deepest markup in a doctype string is counted ten times.
    ///
    /// ⚠️ **Flip-check, run**: the `ENTITY_DEPTH * entity_depth` term dropped fails
    /// at *"an entity holding markup counts ten times"*.
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
