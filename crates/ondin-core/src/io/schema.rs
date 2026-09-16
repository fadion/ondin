//! Versioned on-disk DTO (§5.11).
//!
//! Decoupled from the in-memory `Document`/`Node` so internal refactors don't
//! break saved files: the tree structure is stored as a flat, id-sorted list
//! with string ids, independent of the in-memory `FxHashMap` layout. Node ids
//! and references are strings (`"actor-hex:seq"`); the local transform is stored
//! as its six affine coefficients. The kind/paint value types reuse the model's
//! serde representation (a change there is a schema-version bump).

use crate::document::{DEFAULT_CANVAS_BACKGROUND, Document};
use crate::effect::Effect;
use crate::export::ExportSpec;
use crate::guide::{Guide, GuideAxis, GuideId};
use crate::id::NodeId;
use crate::image::{ImageEntry, ImageFormat, ImageId, ImageSource};
use crate::io::{CURRENT_SCHEMA_VERSION, IoError, MAX_TREE_DEPTH};
use crate::meta::DocumentMeta;
use crate::node::{FillRule, MaskMode, Node, NodeKind, Paint, Pivot};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use kurbo::Affine;
use peniko::Color;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct DocumentDto {
    pub schema_version: u32,
    /// The document's library identity — typed name, project, creation stamp
    /// (§5.11a, [`crate::meta`]).
    ///
    /// **Additive in the strongest form, like `NodeDto::pivot`**: every field is
    /// optional *and* the whole block is skipped when nothing is set, so a
    /// document that has never been through the library is written byte-for-byte
    /// as it was before this field existed and no version bump is owed.
    ///
    /// ⚠️ **At the top on purpose, beside `schema_version`, and the reason is
    /// reading rather than diffing.** The dashboard scans a folder of documents
    /// to list them, and all it wants from each is this block — while the thing
    /// it would have to read past to reach it is the node array and then an
    /// image table that can be megabytes. A first draft put it last, on the
    /// diff argument that keeps `images` last; that argument is real for a table
    /// that must not move, and worth nothing for four short lines whose whole
    /// purpose is to be read without the rest of the file.
    #[serde(default, skip_serializing_if = "DocumentMeta::is_empty")]
    pub meta: DocumentMeta,
    pub root: String,
    /// Sorted by node id on save for byte-stable output (invariant 9).
    pub nodes: Vec<NodeDto>,
    /// The ground behind and around the frames (§15 D18). Defaulted rather than
    /// schema-versioned, on the same grounds as `clip` below: it is purely
    /// additive, and a file written before the ground was saved was drawn on
    /// the default, which is exactly what the default reproduces. See §5.11.
    #[serde(default = "default_ground")]
    pub canvas_background: Color,
    /// Ruler guides, sorted by id on save (invariant 9). Additive in the same
    /// way as `canvas_background`: a file written before guides existed had
    /// none, which is what the empty default reproduces.
    #[serde(default)]
    pub guides: Vec<GuideDto>,
    /// The image table, sorted by id on save (invariant 9).
    ///
    /// **Last, and that is a decision about diffs rather than about serde.** An
    /// embedded image is by far the largest thing in the file and the least
    /// likely to change, so keeping it below the tree means a geometry edit
    /// produces a diff nowhere near it and the image lines stay byte-identical
    /// between saves. `skip_serializing_if` follows `GuideDto::owner`: a document
    /// with no images is written byte-for-byte as it was before this field
    /// existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageDto>,
}

/// One image table entry on disk.
///
/// **The bytes are base64 rather than serde's own array-of-numbers.** `Vec<u8>`
/// through serde_json is `[137,80,78,71,…]` — four to six text bytes per byte,
/// pretty-printed one number per line. Base64 is 1.33× the binary and one line,
/// which is the difference between a 300 KB photograph costing 400 KB in the file
/// and costing well over a megabyte across a hundred thousand lines.
#[derive(Serialize, Deserialize)]
pub(crate) struct ImageDto {
    pub id: String,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub source: ImageSourceDto,
}

/// Embedded bytes or a path — an enum rather than two optional fields, so the
/// file cannot say both or neither and nothing has to validate that it didn't.
#[derive(Serialize, Deserialize)]
pub(crate) enum ImageSourceDto {
    /// The encoded file, base64.
    Embedded {
        data: String,
    },
    Linked {
        path: String,
    },
}

/// What a file written before the ground was saved meant: the design's
/// near-black.
fn default_ground() -> Color {
    DEFAULT_CANVAS_BACKGROUND
}

/// A guide on disk. The id is the same `"<actor-hex>:<seq>"` string a node uses
/// — guides are minted from the one id source — and `color` is absent for a
/// guide left on the shared default, so retuning that default moves every such
/// guide in every saved file.
#[derive(Serialize, Deserialize)]
pub(crate) struct GuideDto {
    pub id: String,
    pub axis: GuideAxis,
    pub position: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    /// The frame this guide is scoped to, absent for a canvas-wide one (§5.5).
    ///
    /// Purely additive, so no version bump: every guide in every file written
    /// before scoping existed ran the whole canvas, which is exactly what a
    /// missing key means. `skip_serializing_if` follows `NodeDto::pivot` rather
    /// than the weaker `default =` shape, so a document with no scoped guide is
    /// written byte-for-byte as it was before the field existed (invariant 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct NodeDto {
    pub id: String,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub kind: NodeKind,
    /// Local transform as `[a, b, c, d, e, f]` affine coefficients.
    pub transform: [f64; 6],
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    /// Whether a resize of the node holds its aspect ratio (§5.3). Defaulted
    /// rather than schema-versioned: it is purely additive, and every file
    /// written while the lock was session state meant "unlocked", which is what
    /// `false` reproduces. See §5.11.
    #[serde(default)]
    pub proportions_locked: bool,
    pub opacity: f32,
    /// Whether the node clips its children (§5.3). Defaulted rather than
    /// schema-versioned: it is purely additive, and a v2 file's frames all
    /// clipped, which is what `true` reproduces. See §5.11.
    #[serde(default = "clips_by_default")]
    pub clip: bool,
    /// Whether the node masks the siblings above it (§5.3). Purely additive, so
    /// no version bump — and unlike `clip` it defaults to *off*, because a file
    /// written before masks existed had none: `false` is what every one of those
    /// nodes meant, and no default function is needed to say so.
    /// `skip_serializing_if` keeps the key out of every document with no mask in
    /// it, so the field changed no existing file's bytes (invariant 9). See
    /// §5.11.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mask: bool,
    /// *How* the node masks (§5.3). Additive on `mask`'s own terms and skipped at
    /// its default, so a document whose masks are all shape masks — which is
    /// every document written before the mode existed — carries no key and no
    /// bytes change (invariant 9).
    #[serde(default, skip_serializing_if = "is_shape_mask")]
    pub mask_mode: MaskMode,
    /// How the node's geometry decides its interior (§15 D239). Additive on the
    /// same terms as `mask_mode` and skipped at its default, so every document
    /// written before the rule existed — all of which meant non-zero — carries no
    /// key and no bytes change (invariant 9).
    #[serde(default, skip_serializing_if = "is_nonzero_fill")]
    pub fill_rule: FillRule,
    pub paint: Paint,
    /// The node's transform origin (§5.3), absent for one still on its box's
    /// centre. Purely additive, so no version bump: a file written before pivots
    /// existed has no entry, and `None` is exactly the centre those files' nodes
    /// rotated and mirrored about. `skip_serializing_if` keeps it out of every
    /// document that never moved one, so adding the field changed no existing
    /// file's bytes (invariant 9). See §5.11.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<Pivot>,
    /// The files this layer produces (§7), absent for one nobody has set up to
    /// export. Purely additive, so no version bump, and on the same terms as
    /// `pivot` above: a file written before export settings existed has no
    /// entry, and an empty list is exactly what those layers meant.
    /// `skip_serializing_if` keeps the key out of every document that never
    /// added one, so the field changed no existing file's bytes (invariant 9).
    /// See §5.11.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<ExportSpec>,
    /// The effect stack (§5.3a), absent for a layer carrying none. Purely
    /// additive on the same terms as `exports` above — a file written before
    /// effects existed has no entry, and an empty stack is exactly what those
    /// layers meant, so no version bump and no existing file's bytes change
    /// (invariant 9).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    /// The layout grids drawn over this layer (`crate::layout`), absent for one
    /// carrying none. Additive on `exports`' terms exactly — no version bump, no
    /// existing file's bytes changed (invariant 9).
    ///
    /// ⚠️ **Not normalized away on a kind that cannot be a frame**, unlike `clip`
    /// and `mask` above. Those two are booleans that would *change behaviour* on
    /// the wrong kind, so a stale one has to be neutralised; a grid list is read
    /// by one panel and one overlay, both of which ask the kind first, so the
    /// worst a stray one costs is bytes — and dropping it would silently delete
    /// something a later version might have a use for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grids: Vec<crate::layout::LayoutGrid>,
}

/// What a file written before `clip` existed meant: frames clipped.
fn clips_by_default() -> bool {
    true
}

/// `skip_serializing_if` for a flag whose "off" is the absence of the key.
fn is_false(b: &bool) -> bool {
    !*b
}

/// The mask mode every file written before the mode existed meant.
fn is_shape_mask(m: &MaskMode) -> bool {
    *m == MaskMode::Shape
}

fn is_nonzero_fill(r: &FillRule) -> bool {
    *r == FillRule::NonZero
}

impl DocumentDto {
    /// Project a document to its DTO, nodes sorted by id (deterministic bytes).
    pub(crate) fn from_document(doc: &Document) -> Self {
        let mut nodes: Vec<NodeDto> = doc
            .nodes_iter()
            .map(|n| NodeDto {
                id: n.id().to_wire(),
                parent: n.parent().map(NodeId::to_wire),
                children: n.children().iter().map(|c| c.to_wire()).collect(),
                kind: n.kind().clone(),
                transform: n.transform().as_coeffs(),
                name: n.name().to_string(),
                visible: n.visible(),
                locked: n.locked(),
                proportions_locked: n.proportions_locked(),
                opacity: n.opacity(),
                // ⚠️ **Normalized on the way *out* as well as on the way in**
                // (§15 D663, `[S1.1-L1-06]`), which is what stops a document's
                // bytes moving on its first open-and-save with no edit made.
                // `op_set_clip` deliberately accepts the flag on a kind that
                // cannot honour it — §15 D282: *"an inert `clip` is a switch
                // nothing reads"*, so dragging a shape through a frame and back
                // keeps its setting — and `into_document` zeroes it on load. Both
                // halves are right on their own and the pair was not: a `.ondin`
                // holding `"clip": true` on a `Group` loaded as `false` and saved
                // back one byte **longer**, silently, for every such group —
                // `clip` carries a named serde default and no
                // `skip_serializing_if`, so the key is always written and
                // `"clip":true` (4) becomes `"clip":false` (5). ⚠️ This comment
                // said *shorter* until `arch-scribe` read the DTO; the direction
                // is not what makes it a defect and is exactly why it went
                // unchecked.
                //
                // Fixed here rather than at either end. The loader's
                // normalization **stays** — it is the guard against a hand-edited
                // or foreign file — and this is the projection saying what the
                // format's `clip` actually means, which is *"clips its
                // children"*. The model is untouched, so D282's property is
                // untouched.
                clip: n.clip() && n.kind().clips_children(),
                mask: n.mask(),
                mask_mode: n.mask_mode(),
                // **The stored field, not `fill_rule()`** — that accessor answers
                // the *effective* rule and would write `EvenOdd` into every
                // `Exclude`, changing the bytes of files that are already correct
                // (§15 D239, invariant 9).
                fill_rule: n.fill_rule,
                paint: n.paint().clone(),
                pivot: n.pivot(),
                exports: n.exports().to_vec(),
                effects: n.effects().to_vec(),
                grids: n.grids().to_vec(),
            })
            .collect();
        // Stable order: sort by (actor, seq) via the parsed id.
        nodes.sort_by_key(|n| NodeId::from_wire(&n.id).expect("wire form we just produced"));
        let mut guides: Vec<GuideDto> = doc
            .guides()
            .iter()
            .map(|g| GuideDto {
                id: g.id.to_wire(),
                axis: g.axis,
                position: g.position,
                color: g.color,
                owner: g.owner.map(NodeId::to_wire),
            })
            .collect();
        // The in-memory list is append-ordered, so it depends on the order the
        // guides were drawn and on what has been undone. Sort as the nodes are.
        guides.sort_by_key(|g| GuideId::from_wire(&g.id).expect("wire form we just produced"));
        let mut images: Vec<ImageDto> = doc
            .images()
            .map(|(id, entry)| ImageDto {
                id: id.0.clone(),
                format: entry.format,
                width: entry.width,
                height: entry.height,
                source: match &entry.source {
                    ImageSource::Embedded(bytes) => ImageSourceDto::Embedded {
                        data: BASE64.encode(bytes),
                    },
                    ImageSource::Linked(path) => ImageSourceDto::Linked { path: path.clone() },
                },
            })
            .collect();
        // A `FxHashMap` has no order at all, so this sort is not tidiness — it is
        // the whole of invariant 9 for the table.
        images.sort_by(|a, b| a.id.cmp(&b.id));
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            root: doc.root().to_wire(),
            nodes,
            canvas_background: doc.canvas_background(),
            guides,
            images,
            meta: doc.meta().clone(),
        }
    }

    /// Rebuild a document from a (migrated) DTO, verifying tree integrity.
    pub(crate) fn into_document(self) -> Result<Document, IoError> {
        let root = parse_id(&self.root)?;
        // Opaque, always — the same rule the editor applies when setting it.
        let ground = self.canvas_background.with_alpha(1.0);

        let mut nodes: FxHashMap<NodeId, Node> = FxHashMap::default();
        for dto in self.nodes {
            let id = parse_id(&dto.id)?;
            let parent = dto.parent.as_deref().map(parse_id).transpose()?;
            let children = dto
                .children
                .iter()
                .map(|c| parse_id(c))
                .collect::<Result<Vec<_>, _>>()?;
            // The flag is kept inert on kinds with no frame to clip to, so a
            // pre-`clip` file (where the default says "yes") does not arrive
            // with every group and rectangle claiming to clip.
            let clip = dto.clip && dto.kind.clips_children();
            // Kept inert on the kinds that cannot be one, the same way `clip` is
            // above. Nothing writes a mask onto a frame or the root, so this
            // guards against a hand-edited or future file rather than against
            // our own default — but a mask flag on a frame would be a switch the
            // inspector cannot show and the walk would have to decide about
            // anyway.
            let mask = dto.mask && dto.kind.can_mask();
            let node = Node {
                id,
                parent,
                children,
                kind: normalize_kind(dto.kind),
                transform: Affine::new(dto.transform),
                name: dto.name,
                visible: dto.visible,
                locked: dto.locked,
                proportions_locked: dto.proportions_locked,
                // ⚠️ **Clamped, not refused**, which is §5.11's own asymmetry
                // rather than a new one: a text span reaching past its content is
                // clamped in `normalize_kind` for exactly this reason, that a
                // recoverable file should stay openable. A hand-edited or
                // third-party `"opacity": 5.0` used to load into a state
                // `op_set_opacity` and `op_insert_subtree` both refuse with
                // `BadOpacity` — the two doors disagreeing about one field. The
                // range is `crate::build::valid_opacity`'s, so there is one
                // answer rather than two spellings of it.
                //
                // `f32::clamp` returns `NaN` for a `NaN` input, so the finite
                // test is not decoration — though a `NaN` cannot actually arrive
                // here, since serde would have refused `null` for an `f32` one
                // step earlier. Written as a fact about the function rather than
                // as a fact about today's DTO.
                opacity: if dto.opacity.is_finite() {
                    dto.opacity.clamp(0.0, 1.0)
                } else {
                    1.0
                },
                clip,
                mask,
                // Not gated on `mask`: the mode is remembered while the mask is
                // off, which is the whole reason it is a field beside the flag.
                mask_mode: dto.mask_mode,
                fill_rule: dto.fill_rule,
                paint: dto.paint,
                pivot: dto.pivot,
                // ⚠️ **Dropped rather than refused, which is this file's own rule
                // for a value that is decoration** (§15 D492). `op_set_exports`
                // rejects a scale or a quality that cannot be one; a file written
                // by an older build, another tool, or a hand edit can still carry
                // one, and refusing the whole document over an export preset would
                // lose the artwork to save the recipe. An unusable spec is not
                // repaired either — `Width(0)` has no defensible non-zero reading —
                // so it goes, and the panel shows one row fewer.
                exports: dto
                    .exports
                    .into_iter()
                    .filter(crate::document::export_spec_is_usable)
                    .collect(),
                effects: dto.effects,
                grids: dto.grids,
            };
            if nodes.insert(id, node).is_some() {
                return Err(IoError::Integrity(format!("duplicate node id {id:?}")));
            }
        }

        // Guides are checked for a parseable id, a finite position and a live
        // owner. There is no tree for one to corrupt, and a guide off the side of
        // the artwork is a perfectly ordinary thing to have — but a NaN would sit
        // in every distance comparison the hit test makes and never match.
        //
        // **The owner is the one reference a guide has**, and a dangling one is
        // not survivable by ignoring it: a scoped guide's `position` is read in
        // its owner's space, so with no owner it has no place, and every drawing
        // and snapping site would need a fallback for a state the model refuses
        // to produce (`Document::check_guide_owner`). Rejected here rather than
        // silently promoted to a global guide, which would move the line to a
        // coordinate the user never chose.
        let mut seen_guides: FxHashSet<GuideId> = FxHashSet::default();
        let mut guides = Vec::with_capacity(self.guides.len());
        for dto in self.guides {
            let id = GuideId::from_wire(&dto.id)
                .ok_or_else(|| IoError::Integrity(format!("malformed guide id {:?}", dto.id)))?;
            if !seen_guides.insert(id) {
                return Err(IoError::Integrity(format!("duplicate guide id {id:?}")));
            }
            if !dto.position.is_finite() {
                return Err(IoError::Integrity(format!(
                    "guide {id:?} has a non-finite position"
                )));
            }
            let owner = match dto.owner.as_deref() {
                None => None,
                Some(wire) => {
                    let owner = parse_id(wire)?;
                    let kind = nodes.get(&owner).map(Node::kind);
                    if !matches!(kind, Some(NodeKind::Artboard { .. })) {
                        return Err(IoError::Integrity(format!(
                            "guide {id:?} is scoped to {owner:?}, which is not a frame in this document"
                        )));
                    }
                    Some(owner)
                }
            };
            guides.push(Guide {
                id,
                axis: dto.axis,
                position: dto.position,
                color: dto.color,
                owner,
            });
        }

        // **A dangling reference is not checked here, and that is deliberate.** A
        // fill naming an id the table does not carry is the missing-image state
        // the renderer has to draw anyway — a linked file that moved — so
        // refusing to load would turn a picture that needs relinking into a
        // document that cannot be opened at all.
        let mut images: FxHashMap<ImageId, ImageEntry> = FxHashMap::default();
        for dto in self.images {
            let id = ImageId(dto.id);
            if images.contains_key(&id) {
                return Err(IoError::Integrity(format!("duplicate image id {id}")));
            }
            if dto.width == 0 || dto.height == 0 {
                return Err(IoError::Integrity(format!(
                    "image {id} has an empty intrinsic size ({}×{})",
                    dto.width, dto.height
                )));
            }
            let source = match dto.source {
                // `.into()` is the `Vec<u8>` → `Arc<[u8]>` move the table's own type
                // now asks for (§15 D301). It reallocates once, here, at load — the
                // one place in the app where a copy of the bytes is unavoidable
                // anyway, since base64 decoding produces the `Vec`.
                ImageSourceDto::Embedded { data } => ImageSource::Embedded(
                    BASE64
                        .decode(&data)
                        .map_err(|e| IoError::Integrity(format!("image {id}: bad base64 ({e})")))?
                        .into(),
                ),
                ImageSourceDto::Linked { path } => ImageSource::Linked(path),
            };
            images.insert(
                id,
                ImageEntry {
                    source,
                    format: dto.format,
                    width: dto.width,
                    height: dto.height,
                },
            );
        }

        verify_integrity(&nodes, root)?;
        Ok(Document::from_parts(
            CURRENT_SCHEMA_VERSION,
            nodes,
            root,
            ground,
            guides,
            images,
            // ⚠️ **Taken as it was written, with nothing filled in.** A loader
            // that minted a missing id would make `load` non-deterministic —
            // reading the same bytes twice would give two documents that
            // disagree — so a pre-library file arrives with an empty block and
            // the app mints its identity the first time the library saves it
            // (`crate::meta::DocumentMeta::id`).
            //
            // ⚠️ **`sanitized` takes nothing *away* that the app could not have
            // put there, and it is the boundary the whole class is closed at.**
            // The id is joined as a path component by three separate writers; a
            // check at each of them is three chances to miss the fourth. See
            // [`DocumentMeta::is_wellformed_id`] — and note this is still
            // deterministic, because it is a function of the bytes alone.
            self.meta.sanitized(),
        ))
    }
}

/// Re-establish the invariants a derived `Deserialize` cannot.
///
/// `Spans` guarantees sorted, non-overlapping, non-default, in-range spans (see
/// `typography.rs`) — for the character list and the paragraph one alike, since
/// they are one type — and every one of those is established by its own
/// `set`/`normalize` rather than by the type's shape, so a file is the one place
/// a span list can arrive having skipped them. Normalizing here rather than
/// rejecting is deliberate: a span reaching past the end of its content is a
/// hand-edit or an older writer's rounding, not a corrupt tree, and clamping it
/// loses nothing a designer would notice. The tree checks below are the ones that
/// must refuse.
fn normalize_kind(kind: NodeKind) -> NodeKind {
    match kind {
        NodeKind::Text {
            content,
            style,
            spans,
            para_spans,
            paragraph,
            block,
            sizing,
            on_path,
            on_path_flip,
            on_path_offset,
        } => {
            let spans = spans.clamped(&content, &style);
            let para_spans = para_spans.clamped(&content, &paragraph);
            // **Not normalized, and there is nothing here it could be clamped
            // against.** A rail is geometry in the node's own space, exactly like
            // `NodeKind::Path`'s own `path`, which this function also leaves alone;
            // an empty or zero-length one is refused by `text::PathWarp::of` at
            // layout time rather than rewritten on the way in, so a file that
            // carries one opens as ordinary horizontal type instead of failing.
            NodeKind::Text {
                content,
                style,
                spans,
                para_spans,
                paragraph,
                block,
                sizing,
                on_path,
                // Carried, not normalized — and inert rather than wrong when a
                // hand-written file sets either with no `on_path` beside it, which
                // is the whole reason they are fields of their own. An offset
                // outside `0..1` is not clamped either: `PathWarp` takes it modulo
                // the rail on a closed one and past the end on an open one, both of
                // which are answers rather than errors.
                on_path_flip,
                on_path_offset,
            }
        }
        other => other,
    }
}

fn parse_id(s: &str) -> Result<NodeId, IoError> {
    NodeId::from_wire(s).ok_or_else(|| IoError::Integrity(format!("malformed node id {s:?}")))
}

/// Full tree integrity (§5.11, invariant 8). Rejects anything that would make
/// the in-memory tree walks (`subtree_ids`, `paint_path`, `resolve`) misbehave,
/// rather than letting a corrupt file surface as a hang or panic later:
///
/// 1. the root exists, is of kind `Root`, and has no parent;
/// 2. every non-root node has a parent that exists;
/// 3. every listed child exists and points back at its parent;
/// 4. no node is listed as a child more than once (anywhere in the document) —
///    a duplicate entry would make one node appear twice in a tree walk;
/// 5. every node is reachable from the root by exactly one path;
/// 6. no node is deeper than [`MAX_TREE_DEPTH`].
///
/// (4) + (5) together rule out cycles and orphan components. Without them a
/// hand-edited file could satisfy parent/child consistency and still loop the
/// tree walks forever, since those walk `children` without a visited set.
///
/// ⚠️ **There is deliberately no finiteness check here, and it took a measurement
/// to know that.** The review asked for one — a file already damaged by a build
/// that let a `NaN` through should be *reported* rather than dying inside serde.
/// It cannot be, because **no JSON input can produce a non-finite `f64` in the
/// first place**: `serde_json` writes one as `null`, refuses `null` where an
/// `f64` is wanted, and refuses an overflowing literal like `1e999` with
/// *"number out of range"*. Both die at `from_value`, one function above this,
/// so a check here is unreachable by construction rather than merely untested.
/// The whole of the protection is therefore at the operation
/// (`OpError::NonFinite`), which is where invariant 8 puts it anyway. What such
/// a file *does* get is serde's own message, which names neither the node nor
/// the field; improving that would mean a custom deserializer for every float
/// on the DTO, and is a decision rather than a fix.
///
/// ⚠️ **(6) is the one that is about the *stack* rather than about the tree, and
/// it was missing.** Every check above bounds a shape; none of them bounded a
/// depth, and the walks §5.11 delegates safety to — `subtree_ids`, `paint_path`,
/// `resolve` — are recursive. A ~2,000-deep document of nested groups (about
/// 900 KB, a one-line generator or an imported SVG of nested `<g>`s) passed all
/// five and then killed the process with `STATUS_STACK_OVERFLOW` inside
/// `Resolved::rebuild`. **A stack overflow is not a panic**: `catch_unwind`
/// cannot see it, no `Failed` memo records it, and every unsaved edit in the
/// session goes with it. And it is reachable without the user doing anything —
/// `library::cover::rasterize` loads and rebuilds *every* document the dashboard
/// grid draws, and the dashboard is the landing screen, so one such file in the
/// synced base folder crashes the app at launch with nothing on screen naming
/// the file.
fn verify_integrity(nodes: &FxHashMap<NodeId, Node>, root: NodeId) -> Result<(), IoError> {
    let root_node = nodes
        .get(&root)
        .ok_or_else(|| IoError::Integrity("root node not present".into()))?;
    if !matches!(root_node.kind(), NodeKind::Root) {
        return Err(IoError::Integrity("root node is not of kind Root".into()));
    }
    if root_node.parent().is_some() {
        return Err(IoError::Integrity("root node has a parent".into()));
    }

    // Which node claims each id as a child; also catches duplicate listings.
    let mut claimed: FxHashMap<NodeId, NodeId> = FxHashMap::default();

    for node in nodes.values() {
        match node.parent() {
            None if node.id() != root => {
                return Err(IoError::Integrity(format!(
                    "non-root node {:?} has no parent",
                    node.id()
                )));
            }
            Some(p) if !nodes.contains_key(&p) => {
                return Err(IoError::Integrity(format!(
                    "node {:?} references missing parent {p:?}",
                    node.id()
                )));
            }
            _ => {}
        }
        for child in node.children() {
            let Some(child_node) = nodes.get(child) else {
                return Err(IoError::Integrity(format!(
                    "node {:?} references missing child {child:?}",
                    node.id()
                )));
            };
            if child_node.parent() != Some(node.id()) {
                return Err(IoError::Integrity(format!(
                    "child {child:?} does not point back to parent {:?}",
                    node.id()
                )));
            }
            if let Some(other) = claimed.insert(*child, node.id()) {
                return Err(IoError::Integrity(format!(
                    "node {child:?} is listed as a child twice (by {other:?} and {:?})",
                    node.id()
                )));
            }
            // Same structural rules a file built through operations obeys.
            crate::document::check_child_kind(node.kind(), child_node.kind()).map_err(|e| {
                IoError::Integrity(format!(
                    "node {child:?} may not be a child of {:?}: {e}",
                    node.id()
                ))
            })?;
        }
    }

    // Reachability, and depth with it. The visited guard makes this terminate on
    // any input, so a malformed file is rejected instead of hanging the loader.
    //
    // **The depth rides along on the walk that was already here**, which is why
    // check (6) costs nothing: this loop visits every node exactly once and the
    // only new state is a `u32` per stack entry. It is an explicit stack rather
    // than recursion, so *this* walk was never the one at risk — the walks it
    // certifies are.
    let mut visited: FxHashSet<NodeId> = FxHashSet::default();
    let mut stack = vec![(root, 0usize)];
    visited.insert(root);
    while let Some((id, depth)) = stack.pop() {
        if depth > MAX_TREE_DEPTH {
            return Err(IoError::Integrity(format!(
                "the tree is nested deeper than {MAX_TREE_DEPTH} (at {id:?}); \
                 the recursive walks this document would be resolved by cannot \
                 carry that"
            )));
        }
        for child in nodes[&id].children() {
            if visited.insert(*child) {
                stack.push((*child, depth + 1));
            }
        }
    }
    if visited.len() != nodes.len() {
        let orphan = nodes.keys().find(|id| !visited.contains(id));
        return Err(IoError::Integrity(format!(
            "{} node(s) are not reachable from the root (e.g. {:?})",
            nodes.len() - visited.len(),
            orphan.copied().unwrap_or(root)
        )));
    }

    Ok(())
}
