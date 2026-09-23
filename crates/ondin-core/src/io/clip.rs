//! The clipboard's wire form: captured subtrees as text, so layers cross
//! between two `ondin` windows (§15 D823).
//!
//! **Not a second format.** A copied subtree is written with the *document*
//! schema's [`NodeDto`] and read back through the same [`NodeDto::into_node`] a
//! `.ondin` file goes through, so every normalization the loader applies — an
//! inert `clip` on a kind that cannot clip, a clamped opacity, an unusable
//! export spec dropped — applies to a pasted layer too, with nothing to keep in
//! step. What this module adds is the *envelope*: several roots instead of one,
//! no canvas ground, no guides, and the box the copy came from.
//!
//! **Text, because text is the only channel there is.** `arboard` offers the
//! system clipboard as text or as an image and nothing else, so a payload that
//! is to survive leaving this process has to be a string. The string opens with
//! a human heading — the layer names, which is what [`write()`]'s caller was
//! already putting there for `egui_winit`'s benefit (§15 D17) — then
//! [`FENCE`], then the JSON. Anyone pasting an Ondin copy into a text editor
//! still sees the names on the first line, which is the property that stand-in
//! was for and the reason the payload did not simply replace it.
//!
//! **The version is checked for equality, not migrated**, which is the one place
//! this deliberately differs from [`crate::io::load`]. A file is an archive and
//! has to open in a build written years later; a clipboard is a handoff between
//! two processes running right now, and the honest answer to "this came from a
//! build whose schema I do not know" is to say so rather than to guess. The
//! migration ladder stays where the archives are.

use crate::image::{ImageEntry, ImageId};
use crate::io::schema::{ImageDto, NodeDto};
use crate::io::{CURRENT_SCHEMA_VERSION, IoError};
use crate::node::Node;
use kurbo::Rect;
use serde::{Deserialize, Serialize};

/// The line that separates the human heading from the payload.
///
/// 🚨 **A marker, and no longer a landmark** (§15 D833). This doc used to read
/// *"found with `rsplit_once` rather than `split_once`, so a layer actually
/// named this does not truncate the heading and take the payload with it"* —
/// and `rsplit_once` was what **caused** that failure rather than preventing
/// it. The fence is written before the JSON, so a layer named this, a text
/// layer containing it, or a linked image path carrying it all put a second
/// copy *after* the real one; splitting on the last occurrence then handed
/// `serde_json` a fragment of a string literal and the copy would not cross.
/// Neither split direction survives a fence inside the payload, because the
/// payload is where the user's own text lives.
///
/// **So the fence answers only *is this ours*, and [`read`] locates the payload
/// by line instead** — which works because the JSON is written with
/// `to_string` rather than a pretty printer and JSON escapes a raw newline, so
/// the payload is exactly one line and is the last one. That is a property of
/// [`write()`] and it is load-bearing: pretty-printing the payload would break
/// the reader.
pub const FENCE: &str = "--- ondin clipboard ---";

/// What a clipboard copy holds once it is off the wire.
///
/// The app's own `Clip` is this minus [`Self::from`], which it keeps in a
/// separate field for a reason of its own — see `OndinApp::clipboard_from`. They
/// are one payload here because the wire form must not be able to carry half of
/// a copy.
pub struct Payload {
    /// One per copied layer, ids still the originals'. `build::insert_subtrees`
    /// mints fresh ones through `document::remap_subtree`, so the ids arriving
    /// from another window are a template rather than a claim on this document.
    pub subtrees: Vec<Vec<Node>>,
    /// The image table entries those subtrees key into. A node carries the key
    /// and the document carries the bytes, so a picture that crossed without
    /// these would arrive as the missing-picture placeholder in silence.
    pub images: Vec<(ImageId, ImageEntry)>,
    /// The world box the copy was taken from, for *Paste here* to aim with.
    /// Absent when the source could not measure one.
    pub from: Option<Rect>,
}

/// The envelope, which is everything a document has that a clip does not need.
#[derive(Serialize, Deserialize)]
struct ClipDto {
    /// [`CURRENT_SCHEMA_VERSION`], because the nodes below *are* the document
    /// schema's nodes. Refused unless it matches exactly — see the module doc.
    schema_version: u32,
    /// One list per copied layer. Flat within a subtree, exactly as
    /// `Document::capture_subtree` produces it: the first entry is that
    /// subtree's root and its `parent` still names the node it was copied out
    /// of, which is what lets a same-document paste land back in its own group.
    subtrees: Vec<Vec<NodeDto>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    images: Vec<ImageDto>,
    /// `[x0, y0, x1, y1]`, absent when the copy had no measurable box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<[f64; 4]>,
}

/// Write a copy as clipboard text: `heading`, [`FENCE`], then the payload.
///
/// `heading` is whatever the caller wants a foreign application to see — the app
/// passes the layer names. It is **not** read back: [`read`] takes the last
/// line and nothing before it, so the heading can say anything — including a
/// newline, or the fence itself — without becoming part of the format.
pub fn write(
    subtrees: &[Vec<Node>],
    images: &[(ImageId, ImageEntry)],
    from: Option<Rect>,
    heading: &str,
) -> Result<String, IoError> {
    let dto = ClipDto {
        schema_version: CURRENT_SCHEMA_VERSION,
        subtrees: subtrees
            .iter()
            .map(|t| t.iter().map(NodeDto::from_node).collect())
            .collect(),
        images: images
            .iter()
            .map(|(id, entry)| ImageDto::from_entry(id, entry))
            .collect(),
        from: from.map(|r| [r.x0, r.y0, r.x1, r.y1]),
    };
    // **Not pretty-printed, unlike `io::save`.** That printer is there for
    // git diffs; nothing diffs a clipboard, and the whitespace is pure size on
    // a payload that has to fit through the OS.
    //
    // 🚨 **And it is now load-bearing rather than a size choice** (§15 D833).
    // `read` locates the payload as the final line, which is well defined only
    // because this is `to_string` and because JSON escapes a raw newline as
    // `\n` — so the whole DTO, including a text layer's multi-line content, is
    // one line. Switching to `to_string_pretty` here would break every paste.
    let json = serde_json::to_string(&dto)?;
    Ok(format!("{heading}\n{FENCE}\n{json}"))
}

/// Read clipboard text back, if it is ours at all.
///
/// **Three answers, not two.** `None` means the text carries no fence and is
/// therefore somebody else's — a sentence, some markup, a path — and the caller
/// should go on to its other arms. `Some(Err(_))` means the text *is* an Ondin
/// copy and could not be read, which is a thing to tell the user about rather
/// than to fall through on: falling through would paste the payload itself as a
/// text layer, which is the one outcome nobody wants.
pub fn read(text: &str) -> Option<Result<Payload, IoError>> {
    // **The fence decides the first answer and the last line carries the
    // payload** (§15 D833). Asking `contains` rather than splitting is what
    // keeps the middle answer reachable: a truncated copy that still shows the
    // fence is ours and unreadable, which the user is told about, rather than
    // foreign text that falls through and gets pasted as a layer.
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
    for dto_nodes in dto.subtrees {
        if dto_nodes.is_empty() {
            return Err(IoError::Integrity("empty subtree on the clipboard".into()));
        }
        let nodes = dto_nodes
            .into_iter()
            .map(NodeDto::into_node)
            .collect::<Result<Vec<Node>, _>>()?;
        // **Checked here rather than left to `remap_subtree`**, which answers
        // `None` for a malformed template and whose caller then skips it — so a
        // payload with one bad subtree in five would paste four layers and say
        // it pasted four, with nothing anywhere naming the one that vanished.
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
        // **The field doc's own rule, which nothing checked** (§15 D835).
        // `ClipDto::subtrees` says in the format's voice that the first entry
        // is that subtree's root, and two paste decisions read it that way:
        // `paste_clipboard_at` takes both the destination parent and the
        // "just above what it came from" z-slot from `template.first()`, while
        // `remap_subtree` and `insert_subtrees` locate the **real** root by
        // predicate. So a reordered payload had the placement computed from one
        // node and applied to another — landing a paste inside a group the user
        // did not choose, in the same-document-in-two-windows case where
        // foreign ids do resolve (§10).
        //
        // Refused rather than reordered: the rule is the format's, so a payload
        // breaking it is malformed rather than merely inconvenient, and
        // silently reordering would leave `capture_subtree`'s guarantee
        // untested on both sides.
        if *root_at != 0 {
            return Err(IoError::Integrity(format!(
                "a clipboard subtree lists its root at {root_at} and a subtree \
                 begins with its root"
            )));
        }
        subtrees.push(nodes);
    }
    let mut images = Vec::with_capacity(dto.images.len());
    let mut seen: Vec<ImageId> = Vec::new();
    for dto_image in dto.images {
        let (id, entry) = dto_image.into_entry()?;
        // 🚨 **A linked picture is refused, and this is the v1 non-goal's first
        // gate** (§15 D852, `[R3-L5-01]`, D819). `into_entry` builds
        // `ImageSource::Linked(path)` from whatever string the payload carries,
        // with no check of any kind, and the SVG export writes that string out
        // verbatim as an `href` — so OS-clipboard text could put an
        // attacker-chosen URL into the user's document, keep it through save,
        // and publish it in every export, while the canvas showed only a
        // missing-picture placeholder. D819 had declared linked images out of v1
        // on the ground that *"nothing can make one"*; this door, added in the
        // same range, could. **The maintainer ruled the non-goal stands**
        // (2026-09-23), so a payload carrying one is refused whole rather than
        // pasted with a placeholder: the refusal says why, and a picture that
        // silently stops drawing does not.
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
    // 🚨 **Only the entries a node actually keys into** (§15 D834). The
    // outbound side has always been precise — `copy_selection` computes the set
    // as `build::image_ids_in` over the copied nodes — and the inbound side
    // imposed no such rule, which made this the asymmetry. `missing_image_ops`
    // adds **every** carried entry the document lacks (its filter is
    // `!doc.has_image(id)` and nothing else), `io::save` writes `doc.images()`
    // whole, and the table is never collected: *"an entry is added and removed
    // by explicit operations only"*. So a payload could plant arbitrary bytes,
    // or an attacker-chosen `Linked` URL, in the victim's document at rest —
    // invisible, because nothing in the UI lists an image no layer shows.
    //
    // **Dropped rather than refused**, which is D492's rule for a value that is
    // decoration and matches D179's *"a dangling reference draws a placeholder
    // rather than failing the load"*: a legitimate payload that over-carries
    // should still paste. No legitimate copy changes, because `copy_selection`
    // already produces exactly the set this keeps.
    let referenced: Vec<ImageId> = subtrees
        .iter()
        .flat_map(|nodes| crate::build::image_ids_in(nodes))
        .collect();
    images.retain(|(id, _)| referenced.contains(id));
    Ok(Payload {
        subtrees,
        images,
        // A non-finite box would sit in every comparison `paste_aim` makes and
        // match none of them, so it is dropped rather than carried — the copy
        // still pastes, at the fallback the app uses for a copy that never had
        // a box at all.
        from: dto
            .from
            .map(|[x0, y0, x1, y1]| Rect::new(x0, y0, x1, y1))
            .filter(|r| {
                r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite()
            }),
    })
}
