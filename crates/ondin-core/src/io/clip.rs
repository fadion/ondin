//! The clipboard's wire form: captured subtrees as text, so layers cross
//! between two `ondin` windows (§15 D823).
//!
//! Not a second format: a subtree is written with the document schema's
//! [`NodeDto`] and read back through [`NodeDto::into_node`], so every loader
//! normalization applies to a pasted layer too. This module adds the envelope:
//! several roots, no canvas ground or guides, and the box the copy came from.
//!
//! The payload is text, the only channel that leaves the process (`arboard`
//! offers text or an image): a human heading (the layer names, for foreign
//! editors), then [`FENCE`], then the JSON.
//!
//! The version is checked for equality, not migrated, unlike [`crate::io::load`]:
//! a clipboard is a handoff between two running processes, and a schema this
//! build does not know is reported rather than guessed at.

use crate::image::{ImageEntry, ImageId};
use crate::io::schema::{ImageDto, NodeDto};
use crate::io::{CURRENT_SCHEMA_VERSION, IoError};
use crate::node::Node;
use kurbo::Rect;
use serde::{Deserialize, Serialize};

/// The line that separates the human heading from the payload.
///
/// The fence answers only "is this ours" (§15 D833); [`read`] locates the payload
/// as the last line. Splitting on the fence in either direction fails, since a
/// layer name or text containing it sits inside the payload. That works because
/// [`write()`] uses `to_string` and JSON escapes newlines, so the payload is
/// exactly one line; pretty-printing it would break the reader.
pub const FENCE: &str = "--- ondin clipboard ---";

/// What a clipboard copy holds once it is off the wire.
///
/// The app's own `Clip` is this minus [`Self::from`], which it keeps separately
/// (see `OndinApp::clipboard_from`); here they are one payload so the wire form
/// cannot carry half a copy.
pub struct Payload {
    /// One per copied layer, ids still the originals'; `build::insert_subtrees`
    /// mints fresh ones through `document::remap_subtree`, so they are a template.
    pub subtrees: Vec<Vec<Node>>,
    /// The vault mains the copy reaches, captured whole with their origins
    /// (`crate::kit::carried`, §15 D1012); empty unless the copy holds a kit
    /// instance. The paste vendors them (`crate::kit::adopt`) so it lands linked.
    pub kits: Vec<Vec<Node>>,
    /// The image table entries those subtrees key into; without them a picture
    /// would arrive as the missing-picture placeholder in silence.
    pub images: Vec<(ImageId, ImageEntry)>,
    /// The variables the subtrees read, with a detached twin of each layer that
    /// reads one (`crate::variable::carried`, §15 D1039), so a paste rebinds by
    /// name or falls back to the value drawn at the copy.
    pub vars: crate::variable::Carried,
    /// The world box the copy was taken from, for *Paste here* to aim with.
    /// Absent when the source could not measure one.
    pub from: Option<Rect>,
}

/// [`crate::variable::Carried`] on the wire.
#[derive(Default, Serialize, Deserialize)]
struct CarriedDto {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    vars: Vec<crate::variable::CarriedVar>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    modes: Vec<crate::variable::Mode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    detached: Vec<NodeDto>,
    /// The styles the layers are bound to (§15 D1062).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    styles: Vec<crate::style::CarriedStyle>,
    /// The kits' variables and styles the copy reads (§15 D1064).
    #[serde(default, skip_serializing_if = "crate::tokens::Tokens::is_empty")]
    tokens: crate::tokens::Tokens,
}

impl CarriedDto {
    fn is_empty(&self) -> bool {
        self.vars.is_empty()
            && self.modes.is_empty()
            && self.detached.is_empty()
            && self.styles.is_empty()
            && self.tokens.is_empty()
    }
}

/// The wire envelope.
#[derive(Serialize, Deserialize)]
struct ClipDto {
    /// [`CURRENT_SCHEMA_VERSION`]; refused unless it matches exactly.
    schema_version: u32,
    /// One list per copied layer, flat as `Document::capture_subtree` produces
    /// it: the first entry is the root, and its `parent` still names the node it
    /// was copied from, so a same-document paste lands back in its own group.
    subtrees: Vec<Vec<NodeDto>>,
    /// The carried kit mains, each a subtree in `subtrees`' form. Absent for a
    /// copy holding no kit instance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    kits: Vec<Vec<NodeDto>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    images: Vec<ImageDto>,
    /// Absent for a copy whose layers read no variable.
    #[serde(default, skip_serializing_if = "CarriedDto::is_empty")]
    vars: CarriedDto,
    /// `[x0, y0, x1, y1]`, absent when the copy had no measurable box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<[f64; 4]>,
}

/// Write a copy as clipboard text: `heading`, [`FENCE`], then the payload.
///
/// `heading` is what a foreign application sees (the app passes the layer
/// names). It is not read back, so it can contain a newline or the fence.
pub fn write(
    subtrees: &[Vec<Node>],
    images: &[(ImageId, ImageEntry)],
    from: Option<Rect>,
    heading: &str,
) -> Result<String, IoError> {
    write_carrying(subtrees, &[], images, &Default::default(), from, heading)
}

/// [`write()`], carrying the kit mains the copy reaches (`crate::kit::carried`,
/// §15 D1012) and what its layers' variables are (`crate::variable::carried`,
/// §15 D1039).
pub fn write_carrying(
    subtrees: &[Vec<Node>],
    kits: &[Vec<Node>],
    images: &[(ImageId, ImageEntry)],
    vars: &crate::variable::Carried,
    from: Option<Rect>,
    heading: &str,
) -> Result<String, IoError> {
    let dto_of = |t: &Vec<Node>| t.iter().map(NodeDto::from_node).collect();
    let dto = ClipDto {
        schema_version: CURRENT_SCHEMA_VERSION,
        subtrees: subtrees.iter().map(dto_of).collect(),
        kits: kits.iter().map(dto_of).collect(),
        images: images
            .iter()
            .map(|(id, entry)| ImageDto::from_entry(id, entry))
            .collect(),
        vars: CarriedDto {
            vars: vars.vars.clone(),
            modes: vars.modes.clone(),
            detached: vars.detached.iter().map(NodeDto::from_node).collect(),
            styles: vars.styles.clone(),
            tokens: vars.tokens.clone(),
        },
        from: from.map(|r| [r.x0, r.y0, r.x1, r.y1]),
    };
    // Not pretty-printed: `read` takes the payload as the final line, which holds
    // only because `to_string` plus JSON's `\n` escape keep the DTO on one line.
    // `to_string_pretty` would break every paste (§15 D833).
    let json = serde_json::to_string(&dto)?;
    Ok(format!("{heading}\n{FENCE}\n{json}"))
}

/// Read clipboard text back, if it is ours at all.
///
/// `None` means no fence, so the text is somebody else's and the caller goes on
/// to its other arms. `Some(Err(_))` means it is an Ondin copy that could not be
/// read, to be reported: falling through would paste the payload as a text layer.
pub fn read(text: &str) -> Option<Result<Payload, IoError>> {
    // `contains` rather than splitting keeps `Some(Err)` reachable: a truncated
    // copy that still shows the fence is ours and unreadable (§15 D833).
    if !text.contains(FENCE) {
        return None;
    }
    let json = text.rsplit_once('\n').map_or("", |(_, last)| last);
    Some(parse(json.trim()))
}

/// [`read`]'s second half, split out so the error paths are one expression each.
fn parse(json: &str) -> Result<Payload, IoError> {
    let dto: ClipDto = serde_json::from_str(json)?;
    if dto.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(IoError::UnsupportedVersion(u64::from(dto.schema_version)));
    }
    let mut subtrees = Vec::with_capacity(dto.subtrees.len());
    let mut kits = Vec::with_capacity(dto.kits.len());
    // The carried kit mains are subtrees in the same form and held to the same
    // two rules (§15 D1012); only where they land differs.
    let tagged = dto.subtrees.into_iter().map(|t| (false, t));
    for (carried, dto_nodes) in tagged.chain(dto.kits.into_iter().map(|t| (true, t))) {
        if dto_nodes.is_empty() {
            return Err(IoError::Integrity("empty subtree on the clipboard".into()));
        }
        let nodes = dto_nodes
            .into_iter()
            .map(NodeDto::into_node)
            .collect::<Result<Vec<Node>, _>>()?;
        // Checked here because `remap_subtree` answers `None` for a malformed
        // template and its caller skips it, so one bad subtree would vanish unnamed.
        let roots: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.parent()
                    .is_none_or(|p| !nodes.iter().any(|m| m.id() == p))
            })
            .map(|(i, _)| i)
            .collect();
        let [root_at] = roots.as_slice() else {
            return Err(IoError::Integrity(format!(
                "a clipboard subtree has {} roots, and a subtree has exactly one",
                roots.len()
            )));
        };
        // The root must be first (§15 D835): `paste_clipboard_at` takes the
        // destination parent and z-slot from `template.first()`, while
        // `remap_subtree` and `insert_subtrees` find the real root by predicate,
        // so a reordered payload would place a paste inside a group the user did
        // not choose. Refused rather than reordered; the order is the format's rule.
        if *root_at != 0 {
            return Err(IoError::Integrity(format!(
                "a clipboard subtree lists its root at {root_at} and a subtree \
                 begins with its root"
            )));
        }
        if carried {
            kits.push(nodes);
        } else {
            subtrees.push(nodes);
        }
    }
    let mut images = Vec::with_capacity(dto.images.len());
    let mut seen: Vec<ImageId> = Vec::new();
    for dto_image in dto.images {
        let (id, entry) = dto_image.into_entry()?;
        // A linked picture is refused (§15 D852, D819): `into_entry` builds
        // `ImageSource::Linked(path)` from any payload string and the SVG export
        // writes it verbatim as an `href`, so clipboard text could plant an
        // attacker-chosen URL in a document and every export. Refused whole, not
        // pasted as a placeholder, so the refusal says why.
        if entry.is_linked() {
            return Err(IoError::Integrity(format!(
                "image {id} is linked to a file rather than embedded, and linked \
                 pictures cannot be pasted"
            )));
        }
        if seen.contains(&id) {
            return Err(IoError::Integrity(format!("duplicate image id {id}")));
        }
        seen.push(id.clone());
        images.push((id, entry));
    }
    // Keep only the entries a node actually keys into (§15 D834), as
    // `copy_selection` does on the way out. `missing_image_ops` adds every carried
    // entry the document lacks and the image table is never collected, so a payload
    // could otherwise plant invisible bytes or a `Linked` URL in a document at rest.
    // Dropped rather than refused: an over-carrying payload should still paste.
    let referenced: Vec<ImageId> = subtrees
        .iter()
        .chain(&kits)
        .flat_map(|nodes| crate::build::image_ids_in(nodes))
        .collect();
    images.retain(|(id, _)| referenced.contains(id));
    let vars = crate::variable::Carried {
        vars: dto.vars.vars,
        modes: dto.vars.modes,
        detached: dto
            .vars
            .detached
            .into_iter()
            .map(NodeDto::into_node)
            .collect::<Result<_, _>>()?,
        styles: dto.vars.styles,
        tokens: dto.vars.tokens,
    };
    Ok(Payload {
        subtrees,
        kits,
        images,
        vars,
        // A non-finite box would match no comparison in `paste_aim`; it is dropped
        // and the copy pastes at the fallback for a copy with no box.
        from: dto
            .from
            .map(|[x0, y0, x1, y1]| Rect::new(x0, y0, x1, y1))
            .filter(|r| {
                r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite()
            }),
    })
}
