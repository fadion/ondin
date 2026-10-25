//! Versioned on-disk DTO (§5.11).
//!
//! Decoupled from the in-memory `Document`/`Node` so internal refactors don't
//! break saved files: the tree structure is stored as a flat, id-sorted list
//! with string ids, independent of the in-memory `FxHashMap` layout. Node ids
//! and references are strings (`"actor-hex:seq"`); the local transform is stored
//! as its six affine coefficients. The kind/paint value types reuse the model's
//! serde representation (a change there is a schema-version bump).

use crate::document::Document;
use crate::effect::Effect;
use crate::export::ExportSpec;
use crate::guide::{Guide, GuideAxis, GuideId};
use crate::id::NodeId;
use crate::image::{ImageEntry, ImageFormat, ImageId, ImageSource};
use crate::io::{CURRENT_SCHEMA_VERSION, IoError, MAX_TREE_DEPTH};
use crate::item::Keyed;
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
    /// Additive: every field is optional and an empty block is skipped, so an
    /// older document is written unchanged and no version bump is owed.
    ///
    /// At the top, beside `schema_version`, so the dashboard can read it without
    /// reading past the nodes and the image table.
    #[serde(default, skip_serializing_if = "DocumentMeta::is_empty")]
    pub meta: DocumentMeta,
    /// The pages in order (`crate::page`), each naming its root in `nodes`. New
    /// in v10, replacing the one `root` and the document's `canvas_background`.
    pub pages: Vec<PageDto>,
    /// The vault's root — the parentless root holding vendored kit mains (§15
    /// D1012) — absent in a document that has used no kit. Its nodes are in
    /// `nodes` with the rest, which is why the reachability check walks from it
    /// as well as from every page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<String>,
    /// Sorted by node id on save for byte-stable output (invariant 9).
    pub nodes: Vec<NodeDto>,
    /// Ruler guides, sorted by id on save (invariant 9). A file without the key
    /// loads with none.
    #[serde(default)]
    pub guides: Vec<GuideDto>,
    /// The variables and the mode list (§5.3f, §15 D1039), absent in a document
    /// that has neither. Above the images for their diff reason.
    #[serde(default, skip_serializing_if = "VariablesDto::is_empty")]
    pub variables: VariablesDto,
    /// The styles (§15 D1062), sorted by id on save (invariant 9), absent in a
    /// document with none. New in v8.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<crate::style::Style>,
    /// The image table, sorted by id on save (invariant 9).
    ///
    /// Near the end for diffs: an embedded image is the largest and least-changing
    /// part of the file, so a geometry edit's diff stays away from it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageDto>,
    /// What a kit last published (§5.3e, §15 D1012), absent in every document that
    /// has not. Last, for `images`' diff reason: it can hold a copy of every main.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<PublishedDto>,
}

/// A kit's published state on disk: a whole document, in this same form, and
/// the frame each published main sat in.
///
/// A migration that reshapes a document must reshape this one too: the ladder
/// (`io::migrate`) walks the outer value and does not descend into the nested
/// `document`.
#[derive(Serialize, Deserialize)]
pub(crate) struct PublishedDto {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupDto>,
    pub document: Box<DocumentDto>,
    /// The kit's name at the publish (§15 D1014).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The variables table on disk: the modes in their order (the first is the
/// default) and the variables sorted by id (invariant 9), cells keyed by mode id.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct VariablesDto {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<crate::variable::Mode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<crate::variable::Variable>,
    /// The kits' modes and mappings, by kit id (§15 D1064); a `BTreeMap`, so in
    /// order. New in v8.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub kits: std::collections::BTreeMap<String, crate::variable::KitModes>,
}

impl VariablesDto {
    fn is_empty(&self) -> bool {
        self.modes.is_empty() && self.vars.is_empty() && self.kits.is_empty()
    }

    fn from_table(t: &crate::variable::Variables) -> Self {
        // `iter` is in id order already — a `BTreeMap` — so this is the sort.
        Self {
            modes: t.modes().to_vec(),
            vars: t.iter().cloned().collect(),
            kits: t.kits().clone(),
        }
    }

    /// The table, refusing a repeated variable id; its rules are checked with the
    /// nodes' bindings once the tree is built (`variable::check`).
    fn into_table(self) -> Result<crate::variable::Variables, IoError> {
        let mut vars = std::collections::BTreeMap::new();
        for v in self.vars {
            let id = v.id;
            if vars.insert(id, v).is_some() {
                return Err(IoError::Integrity(format!("duplicate variable id {id:?}")));
            }
        }
        Ok(crate::variable::Variables {
            modes: self.modes,
            vars,
            kits: self.kits,
        })
    }
}

/// One published main or set and the top-level frame it sat in (§15 D1011 (4)).
#[derive(Serialize, Deserialize)]
pub(crate) struct GroupDto {
    pub top: String,
    pub frame: String,
}

/// One image table entry on disk.
///
/// The bytes are base64 rather than serde's array of numbers, which costs four to
/// six text bytes per byte and one line each.
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

/// A page on disk: its root's id, and the page-table fields its root node does
/// not carry (its name and mode are the node's).
#[derive(Serialize, Deserialize)]
pub(crate) struct PageDto {
    pub id: String,
    pub ground: Color,
    #[serde(default, skip_serializing_if = "is_false")]
    pub draft: bool,
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
    /// The frame this guide is scoped to, or the page it runs across (§5.5).
    /// Required from v10, where a canvas-wide guide names its page.
    pub owner: String,
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
    /// Whether a resize holds the aspect ratio (§5.3). Defaulted, not versioned:
    /// earlier files meant "unlocked", which `false` reproduces.
    #[serde(default)]
    pub proportions_locked: bool,
    pub opacity: f32,
    /// Whether the node clips its children (§5.3). Defaulted, not versioned: a v2
    /// file's frames all clipped, which `true` reproduces.
    #[serde(default = "clips_by_default")]
    pub clip: bool,
    /// Whether the node masks the siblings above it (§5.3). Additive; unlike
    /// `clip` it defaults to off, and is skipped when off (invariant 9).
    #[serde(default, skip_serializing_if = "is_false")]
    pub mask: bool,
    /// How the node masks (§5.3). Additive and skipped at its default: every
    /// earlier file's masks are shape masks (invariant 9).
    #[serde(default, skip_serializing_if = "is_shape_mask")]
    pub mask_mode: MaskMode,
    /// How the node's geometry decides its interior (§15 D239). Additive and
    /// skipped at its default: every earlier file meant non-zero (invariant 9).
    #[serde(default, skip_serializing_if = "is_nonzero_fill")]
    pub fill_rule: FillRule,
    pub paint: Paint,
    /// The node's transform origin (§5.3), absent for one on its box's centre,
    /// which is what earlier files meant. Additive and skipped when `None`
    /// (invariant 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<Pivot>,
    /// The files this layer produces (§7), absent when empty. Additive and
    /// skipped when empty (invariant 9).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<Keyed<ExportSpec>>,
    /// The effect stack (§5.3a), absent when empty. Additive, as `exports`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Keyed<Effect>>,
    /// The layout grids drawn over this layer (`crate::layout`), absent when
    /// empty. Additive, as `exports`.
    ///
    /// Not normalized away on a kind that cannot be a frame, unlike `clip` and
    /// `mask`: those would change behaviour on the wrong kind, while a stray grid
    /// list is only ignored and dropping it would delete data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grids: Vec<Keyed<crate::layout::LayoutGrid>>,
    /// The layer's CSS insets inside its frame (`crate::container`), absent when
    /// unset. Additive (§5.11, §15 D868); kept on a layer where they are inert,
    /// for `grids`' reason.
    #[serde(default, skip_serializing_if = "crate::container::Insets::is_unset")]
    pub insets: crate::container::Insets,
    /// The layer's layout (`crate::container::Display`), absent for a container
    /// with none. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<crate::container::Display>,
    /// The layer's layout-item properties, absent at CSS's defaults.
    #[serde(
        default,
        skip_serializing_if = "crate::container::LayoutItem::is_default"
    )]
    pub item: crate::container::LayoutItem,
    /// Whether the layer is a main component (§15 D978), absent when not. Arrived
    /// with schema v5 so an older build refuses a file holding components rather
    /// than silently dropping them (§5.3d).
    #[serde(default, skip_serializing_if = "is_false")]
    pub component: bool,
    /// The node this one was copied from, as a wire id (§15 D978), absent for a
    /// layer outside any instance. Checked on load like a guide's owner: a link to
    /// nothing is refused, not dropped (`crate::component::check`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The main a nested copy's contents are swapped to, as a wire id (§15 D983),
    /// absent on every layer not swapped. Part of v5. Checked on load with the
    /// links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap: Option<String>,
    /// A component set's variant properties (§15 D982), absent on every other
    /// layer. Part of v5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<crate::variant::VariantSet>,
    /// A variant's values, absent on everything that is not one (§15 D982).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variant: Vec<String>,
    /// A main's or a set's component properties, absent when it defines none
    /// (§15 D982). Bound layers are wire ids, checked on load with the links.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<Keyed<crate::variant::Property>>,
    /// A main's or a set's description, absent when it has none. New in v9.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Where a vault node was vendored from (§15 D1012), absent on every node
    /// outside the vault — which `crate::kit::check` holds on load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<OriginDto>,
    /// The mode a frame sets (§5.3f), absent to inherit. New in v7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<crate::variable::ModeId>,
    /// A rectangle's corner bindings (§15 D1039), absent when none is bound.
    /// New in v7.
    #[serde(default, skip_serializing_if = "no_corner_vars")]
    pub corner_vars: crate::variable::CornerVars,
    /// A container's gap and padding bindings (§15 D1058), absent when none is
    /// bound. New in v8.
    #[serde(default, skip_serializing_if = "no_spacing_vars")]
    pub spacing_vars: crate::variable::SpacingVars,
    /// The boolean variable a layer's visibility is bound to (§15 D1059),
    /// absent when unbound. New in v8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_var: Option<crate::variable::VarId>,
    /// The number variables a layer's width and height are bound to (§15
    /// D1061), absent when neither is. New in v8.
    #[serde(default, skip_serializing_if = "no_size_vars")]
    pub size_vars: crate::variable::SizeVars,
    /// The variables a text's content, family and type numbers are bound to,
    /// absent when none is; up to the last bound slot, so a file with the first
    /// two slots only loads. New in v8 with two slots; the number slots in v9.
    #[serde(
        default,
        skip_serializing_if = "no_text_vars",
        with = "crate::variable::text_vars_serde"
    )]
    pub text_vars: crate::variable::TextVars,
    /// The number variable a layer's opacity is bound to (§15 D1072), absent
    /// when unbound. New in v8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity_var: Option<crate::variable::VarId>,
    /// The styles a layer's fills, strokes and effects are bound to (§15
    /// D1062), and a text layer's type (§15 D1067), absent when none is. New in
    /// v8, as `size_vars` is.
    #[serde(default, skip_serializing_if = "crate::style::StyleRefs::is_empty")]
    pub styles: crate::style::StyleRefs,
}

fn no_size_vars(v: &crate::variable::SizeVars) -> bool {
    v.iter().all(Option::is_none)
}

fn no_text_vars(v: &crate::variable::TextVars) -> bool {
    v.iter().all(Option::is_none)
}

fn no_corner_vars(v: &crate::variable::CornerVars) -> bool {
    v.iter().all(Option::is_none)
}

fn no_spacing_vars(v: &crate::variable::SpacingVars) -> bool {
    v.iter().all(Option::is_none)
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

impl NodeDto {
    /// Project one node to its DTO.
    ///
    /// Shared with the clipboard (`crate::io::clip`, §15 D823): a copied subtree is
    /// the same wire form as a saved one, so there is one projection and one set
    /// of normalization rules.
    pub(crate) fn from_node(n: &Node) -> Self {
        Self {
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
            // Normalized on the way out as well as in (§15 D663): `op_set_clip`
            // accepts the flag on a kind that cannot honour it and `into_node`
            // zeroes it on load, so without this a document's bytes move on
            // open-and-save. The loader's normalization stays as the guard
            // against a hand-edited file.
            clip: n.clip() && n.kind().clips_children(),
            mask: n.mask(),
            mask_mode: n.mask_mode(),
            // The stored field, not `fill_rule()`, which answers the effective
            // rule and would write `EvenOdd` into every `Exclude` (invariant 9).
            fill_rule: n.fill_rule,
            paint: n.paint().clone(),
            pivot: n.pivot(),
            exports: n.exports().to_vec(),
            effects: n.effects().to_vec(),
            grids: n.grids().to_vec(),
            insets: *n.insets(),
            display: n.display().cloned(),
            item: *n.item(),
            component: n.component(),
            link: n.link().map(NodeId::to_wire),
            swap: n.swap().map(NodeId::to_wire),
            set: n.set().cloned(),
            variant: n.variant().to_vec(),
            props: n.props().to_vec(),
            description: n.description().to_string(),
            origin: n.origin().map(|o| OriginDto {
                kit: o.kit.clone(),
                node: o.node.to_wire(),
                version: o.version,
                name: o.name.clone(),
            }),
            mode: n.mode(),
            corner_vars: n.corner_vars(),
            spacing_vars: n.spacing_vars(),
            visible_var: n.visible_var(),
            size_vars: n.size_vars(),
            text_vars: n.text_vars(),
            opacity_var: n.opacity_var(),
            styles: n.styles(),
        }
    }

    /// Rebuild one node from its DTO, normalizing the flags a foreign or
    /// hand-edited file can state on a kind that cannot honour them.
    ///
    /// Every rule here is about the format, which is why the clipboard shares it.
    pub(crate) fn into_node(self) -> Result<Node, IoError> {
        let id = parse_id(&self.id)?;
        let parent = self.parent.as_deref().map(parse_id).transpose()?;
        let link = self.link.as_deref().map(parse_id).transpose()?;
        let swap = self.swap.as_deref().map(parse_id).transpose()?;
        let children = self
            .children
            .iter()
            .map(|c| parse_id(c))
            .collect::<Result<Vec<_>, _>>()?;
        // Inert on kinds with no frame to clip to, so a pre-`clip` file (default
        // "yes") does not arrive with every group claiming to clip.
        let clip = self.clip && self.kind.clips_children();
        // Inert on kinds that cannot be a mask, like `clip`; guards against a
        // hand-edited or future file.
        let mask = self.mask && self.kind.can_mask();
        Ok(Node {
            id,
            parent,
            children,
            kind: normalize_kind(self.kind),
            transform: Affine::new(self.transform),
            name: self.name,
            visible: self.visible,
            locked: self.locked,
            proportions_locked: self.proportions_locked,
            // Clamped, not refused (§5.11): a recoverable file stays openable, and
            // the range is `crate::build::valid_opacity`'s so the operations and
            // the loader agree. `f32::clamp` passes `NaN` through, so the finite
            // test stays even though serde refuses `null` for an `f32`.
            opacity: if self.opacity.is_finite() {
                self.opacity.clamp(0.0, 1.0)
            } else {
                1.0
            },
            clip,
            mask,
            // Not gated on `mask`: the mode is remembered while the mask is off.
            mask_mode: self.mask_mode,
            fill_rule: self.fill_rule,
            paint: self.paint,
            pivot: self.pivot,
            // Dropped rather than refused (§15 D492): refusing the document over
            // an export preset would lose the artwork to save the recipe, and an
            // unusable spec such as `Width(0)` has no defensible repair.
            exports: self
                .exports
                .into_iter()
                .filter(|k| crate::document::export_spec_is_usable(k))
                .collect(),
            effects: self
                .effects
                .into_iter()
                .map(|mut k| {
                    crate::effect::clamp_blur(&mut k.value);
                    k
                })
                .collect(),
            grids: self.grids,
            insets: self.insets,
            display: self.display,
            item: self.item,
            component: self.component,
            link,
            swap,
            set: self.set,
            variant: self.variant,
            // Re-keyed rather than refused: earlier builds saved sets with a
            // repeated property id, and refusing would lose the artwork. It is a
            // function of the list, so the same bytes give one document.
            props: crate::item::rekey_repeats(self.props),
            description: self.description,
            origin: self
                .origin
                .map(|o| -> Result<_, IoError> {
                    Ok(crate::kit::Origin {
                        kit: o.kit,
                        node: parse_id(&o.node)?,
                        version: o.version,
                        name: o.name,
                    })
                })
                .transpose()?,
            mode: self.mode,
            corner_vars: self.corner_vars,
            spacing_vars: self.spacing_vars,
            visible_var: self.visible_var,
            size_vars: self.size_vars,
            text_vars: self.text_vars,
            opacity_var: self.opacity_var,
            styles: self.styles,
        })
    }
}

/// A vendored node's provenance on disk (§15 D1012): the kit's `meta.id`, the
/// node there as a wire id, and the published version.
#[derive(Serialize, Deserialize)]
pub(crate) struct OriginDto {
    pub kit: String,
    pub node: String,
    pub version: u32,
    /// The kit's name, on a vault main's top node only (§15 D1014).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ImageDto {
    /// Project one image table entry to its DTO.
    pub(crate) fn from_entry(id: &ImageId, entry: &ImageEntry) -> Self {
        Self {
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
        }
    }

    /// Rebuild one image table entry, refusing an empty intrinsic size and bad
    /// base64 — the two states nothing downstream has a reading for.
    pub(crate) fn into_entry(self) -> Result<(ImageId, ImageEntry), IoError> {
        let id = ImageId(self.id);
        if self.width == 0 || self.height == 0 {
            return Err(IoError::Integrity(format!(
                "image {id} has an empty intrinsic size ({}×{})",
                self.width, self.height
            )));
        }
        let source = match self.source {
            // `.into()` is the `Vec<u8>` → `Arc<[u8]>` move; the one copy at load.
            ImageSourceDto::Embedded { data } => ImageSource::Embedded(
                BASE64
                    .decode(&data)
                    .map_err(|e| IoError::Integrity(format!("image {id}: bad base64 ({e})")))?
                    .into(),
            ),
            ImageSourceDto::Linked { path } => ImageSource::Linked(path),
        };
        Ok((
            id,
            ImageEntry {
                source,
                format: self.format,
                width: self.width,
                height: self.height,
            },
        ))
    }
}

impl DocumentDto {
    /// Project a document to its DTO, nodes sorted by id (deterministic bytes).
    pub(crate) fn from_document(doc: &Document) -> Self {
        let mut nodes: Vec<NodeDto> = doc.nodes_iter().map(NodeDto::from_node).collect();
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
                owner: g.owner.to_wire(),
            })
            .collect();
        // The in-memory list is append-ordered, so it depends on the order the
        // guides were drawn and on what has been undone. Sort as the nodes are.
        guides.sort_by_key(|g| GuideId::from_wire(&g.id).expect("wire form we just produced"));
        let mut images: Vec<ImageDto> = doc
            .images()
            .map(|(id, entry)| ImageDto::from_entry(id, entry))
            .collect();
        // A `FxHashMap` has no order, so this sort is invariant 9 for the table.
        images.sort_by(|a, b| a.id.cmp(&b.id));
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            pages: doc
                .pages()
                .iter()
                .map(|p| PageDto {
                    id: p.id.to_wire(),
                    ground: p.ground,
                    draft: p.draft,
                })
                .collect(),
            vault: doc.vault().map(NodeId::to_wire),
            nodes,
            guides,
            variables: VariablesDto::from_table(doc.variables()),
            styles: doc.styles().iter().cloned().collect(),
            images,
            meta: doc.meta().clone(),
            published: doc.published().map(|p| PublishedDto {
                groups: p
                    .groups()
                    .iter()
                    .map(|(t, f)| GroupDto {
                        top: t.to_wire(),
                        frame: f.clone(),
                    })
                    .collect(),
                document: Box::new(Self::from_document(p.document())),
                name: p.name().map(str::to_string),
            }),
        }
    }

    /// Rebuild a document from a (migrated) DTO, verifying tree integrity.
    pub(crate) fn into_document(self) -> Result<Document, IoError> {
        let vault = self.vault.as_deref().map(parse_id).transpose()?;
        let mut pages: Vec<crate::page::Page> = Vec::with_capacity(self.pages.len());
        for dto in &self.pages {
            let id = parse_id(&dto.id)?;
            if pages.iter().any(|p| p.id == id) || Some(id) == vault {
                return Err(IoError::Integrity(format!(
                    "page {id:?} is listed twice, or is the vault"
                )));
            }
            pages.push(crate::page::Page {
                id,
                // Opaque, always — the same rule the editor applies when setting it.
                ground: dto.ground.with_alpha(1.0),
                draft: dto.draft,
            });
        }
        if pages.is_empty() {
            return Err(IoError::Integrity("the document has no page".into()));
        }
        let page_ids: Vec<NodeId> = pages.iter().map(|p| p.id).collect();

        let mut nodes: FxHashMap<NodeId, Node> = FxHashMap::default();
        for dto in self.nodes {
            let node = dto.into_node()?;
            let id = node.id();
            if nodes.insert(id, node).is_some() {
                return Err(IoError::Integrity(format!("duplicate node id {id:?}")));
            }
        }

        // Guides are checked for a parseable id, a finite position and a live
        // owner. A dangling owner is rejected rather than promoted to a global
        // guide, which would move the line to a coordinate the user never chose
        // (`Document::check_guide_owner`).
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
            let owner = parse_id(&dto.owner)?;
            let kind = nodes.get(&owner).map(Node::kind);
            if !page_ids.contains(&owner) && !matches!(kind, Some(NodeKind::Artboard { .. })) {
                return Err(IoError::Integrity(format!(
                    "guide {id:?} is scoped to {owner:?}, which is not a frame or a page in this document"
                )));
            }
            guides.push(Guide {
                id,
                axis: dto.axis,
                position: dto.position,
                color: dto.color,
                owner,
            });
        }

        // A dangling image reference is deliberately not checked: it is the
        // missing-image state the renderer draws anyway, and refusing would make a
        // picture needing relinking into an unopenable document.
        let mut images: FxHashMap<ImageId, ImageEntry> = FxHashMap::default();
        for dto in self.images {
            let (id, entry) = dto.into_entry()?;
            if images.insert(id.clone(), entry).is_some() {
                return Err(IoError::Integrity(format!("duplicate image id {id}")));
            }
        }

        verify_integrity(&nodes, &page_ids, vault)?;

        // The variables rules (§5.3f, §15 D1039), `apply`'s post-condition. Refused,
        // not repaired: dropping a dangling binding would hand the layer a literal
        // nobody chose.
        let variables = self.variables.into_table()?;
        let in_vault = |id: NodeId| {
            let mut cursor = Some(id);
            for _ in 0..=nodes.len() {
                match cursor {
                    Some(c) if Some(c) == vault => return true,
                    Some(c) => cursor = nodes.get(&c).and_then(|n| n.parent()),
                    None => return false,
                }
            }
            false
        };
        crate::variable::check(&variables, nodes.values(), in_vault)
            .map_err(|rule| IoError::Integrity(rule.to_string()))?;

        // The style rules (§15 D1062), refused for the same reason. A layer whose
        // list differs from its style's loads as written; the next
        // `style::settle` puts it back unless that edit wrote the slot.
        let mut styles = Vec::with_capacity(self.styles.len());
        for mut s in self.styles {
            if styles.iter().any(|t: &crate::style::Style| t.id == s.id) {
                return Err(IoError::Integrity(format!("duplicate style id {:?}", s.id)));
            }
            // Clamped as a layer's are, or `style::settle` could never apply it.
            if let crate::style::StyleValue::Effects(v) = &mut s.value {
                for k in v.iter_mut() {
                    crate::effect::clamp_blur(&mut k.value);
                }
            }
            styles.push(s);
        }
        let styles = crate::style::Styles::from_list(styles);
        crate::style::check(&styles, &variables, nodes.values(), in_vault)
            .map_err(|rule| IoError::Integrity(rule.to_string()))?;

        // The published state is a document of its own, held to the same rules,
        // one level deep only: refusing a nested one bounds the recursion.
        let published = match self.published {
            None => None,
            Some(p) => {
                if p.document.published.is_some() {
                    return Err(IoError::Integrity(
                        "a published kit state holds a published state of its own".into(),
                    ));
                }
                let doc = p.document.into_document()?;
                let mut groups = Vec::with_capacity(p.groups.len());
                for g in p.groups {
                    groups.push((parse_id(&g.top)?, g.frame));
                }
                Some(crate::kit::Published::from_parts(doc, groups, p.name))
            }
        };
        Ok(Document::from_parts(
            CURRENT_SCHEMA_VERSION,
            nodes,
            pages,
            vault,
            guides,
            images,
            variables,
            styles,
            // Taken as written, with nothing minted: a loader that filled in a
            // missing id would make `load` non-deterministic, so the library mints
            // it at first save (`crate::meta::DocumentMeta::id`). `sanitized` is
            // the boundary where an unsafe id (joined as a path component by
            // several writers) is closed off; see
            // [`DocumentMeta::is_wellformed_id`].
            self.meta.sanitized(),
            published,
        ))
    }
}

/// Re-establish the invariants a derived `Deserialize` cannot.
///
/// `Spans` guarantees sorted, non-overlapping, non-default, in-range spans (see
/// `typography.rs`) through its own `set`/`normalize`, not its shape, so a file is
/// where a list can arrive unnormalized. Clamping rather than rejecting is
/// deliberate: an overlong span is a hand-edit, not a corrupt tree.
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
            // The rail is not normalized: it is geometry like `NodeKind::Path`'s
            // `path`, and `text::PathWarp::of` refuses a degenerate one at layout,
            // so the file opens as horizontal type.
            NodeKind::Text {
                content,
                style,
                spans,
                para_spans,
                paragraph,
                block,
                sizing,
                on_path,
                // Carried, not normalized: inert without `on_path`, and an offset
                // outside `0..1` is an answer to `PathWarp` (modulo on a closed
                // rail, past the end on an open one), not an error.
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
/// 1. every page's root exists, is of kind `Root`, and has no parent;
/// 2. every node but a page's root and the vault has a parent that exists;
/// 3. every listed child exists and points back at its parent;
/// 4. no node is listed as a child more than once (anywhere in the document) —
///    a duplicate entry would make one node appear twice in a tree walk;
/// 5. every node is reachable from a page or the vault by exactly one path;
/// 6. no node is deeper than [`MAX_TREE_DEPTH`];
/// 7. no frame sits anywhere under a boolean or a mask (§15 D876).
///
/// (4) + (5) together rule out cycles and orphan components. Without them a
/// hand-edited file could satisfy parent/child consistency and still loop the
/// tree walks forever, since those walk `children` without a visited set.
///
/// There is deliberately no finiteness check: no JSON input can produce a
/// non-finite `f64` (serde_json refuses `null` and `1e999` at `from_value`), so
/// the protection is at the operation (`OpError::NonFinite`).
///
/// (6) bounds the stack, not the shape: the recursive walks (`subtree_ids`,
/// `paint_path`, `resolve`) overflow the stack on a deeply nested file, which
/// `catch_unwind` cannot see, and the dashboard's cover rasterizer loads every
/// document at launch.
fn verify_integrity(
    nodes: &FxHashMap<NodeId, Node>,
    pages: &[NodeId],
    vault: Option<NodeId>,
) -> Result<(), IoError> {
    for page in pages {
        let root_node = nodes
            .get(page)
            .ok_or_else(|| IoError::Integrity(format!("page {page:?}'s root is not present")))?;
        if !matches!(root_node.kind(), NodeKind::Root) {
            return Err(IoError::Integrity(format!(
                "page {page:?}'s root is not of kind Root"
            )));
        }
        if root_node.parent().is_some() {
            return Err(IoError::Integrity(format!(
                "page {page:?}'s root has a parent"
            )));
        }
    }

    // Which node claims each id as a child; also catches duplicate listings.
    let mut claimed: FxHashMap<NodeId, NodeId> = FxHashMap::default();

    for node in nodes.values() {
        // The rule `Document::apply` checks after every transaction (§15 D980),
        // at the other door: a hand-edited file repeating an item id in one list.
        node.check_item_ids()
            .map_err(|e| IoError::Integrity(e.to_string()))?;
        match node.parent() {
            None if !pages.contains(&node.id()) && Some(node.id()) != vault => {
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
    // Checks (6) and (7) ride along on this explicit-stack walk. (7) is an
    // ancestor rule the pairwise `check_child_kind` cannot see; each entry's flag
    // is "something above here bars frames". The walk starts from every page
    // and the vault (§15 D1012); the vault's shape is `kit::check`'s, below.
    let mut visited: FxHashSet<NodeId> = FxHashSet::default();
    let mut stack: Vec<(NodeId, usize, bool)> = pages.iter().map(|p| (*p, 0, false)).collect();
    visited.extend(pages.iter().copied());
    if let Some(v) = vault {
        if pages.contains(&v) || !nodes.contains_key(&v) {
            return Err(IoError::Integrity(format!(
                "node {v:?} {}",
                crate::kit::VaultRule::Shape
            )));
        }
        stack.push((v, 0, false));
        visited.insert(v);
    }
    while let Some((id, depth, barred)) = stack.pop() {
        if depth > MAX_TREE_DEPTH {
            return Err(IoError::Integrity(format!(
                "the tree is nested deeper than {MAX_TREE_DEPTH} (at {id:?}); \
                 the recursive walks this document would be resolved by cannot \
                 carry that"
            )));
        }
        let node = &nodes[&id];
        if barred && matches!(node.kind(), NodeKind::Artboard { .. }) {
            return Err(IoError::Integrity(format!(
                "frame {id:?} sits inside a boolean or a mask: {}",
                crate::op::OpError::ArtboardPlacement
            )));
        }
        let barred = barred || crate::document::bars_frames(node);
        for child in node.children() {
            if visited.insert(*child) {
                stack.push((*child, depth + 1, barred));
            }
        }
    }
    if visited.len() != nodes.len() {
        let orphan = nodes.keys().find(|id| !visited.contains(id));
        return Err(IoError::Integrity(format!(
            "{} node(s) are not reachable from a page (e.g. {:?})",
            nodes.len() - visited.len(),
            orphan.copied().unwrap_or(pages[0])
        )));
    }

    // The component rules (§15 D978), after the tree is known to be a tree: the
    // ancestor walks in `component::check` assume parents resolve and end. A link
    // to nothing is refused, since dropping it would silently detach an instance.
    crate::component::check(nodes)
        .map_err(|(id, rule)| IoError::Integrity(format!("node {id:?} {rule}")))?;
    // And the vault's (§15 D1012), `Document::apply`'s post-condition at this door.
    crate::kit::check(nodes, vault)
        .map_err(|(id, rule)| IoError::Integrity(format!("node {id:?} {rule}")))?;

    Ok(())
}
