//! ondin-export — SVG, PNG, JPEG, and the AI-handoff JSON snapshot (§7).
//!
//! SVG is a pure function of core state (also a model-completeness gate). PNG and
//! JPEG go through the CPU renderer (no GPU) and differ only in the encoder at the
//! end. The snapshot is the stable public projection MCP read tools return,
//! versioned independently of the save format.
//!
//! All the writers take a *set of subtrees* as well as the whole page — `svg_of`,
//! `png_of` — because "export the selection" is a question a viewport cannot
//! answer: a box drawn round some layers also contains whatever overlaps it.
//!
//! [`mod@plan`] sits above them and is the one place a saved [`ExportSpec`] is turned
//! into numbers and filenames, so the app's panel, the CLI and the zip cannot come
//! to disagree about what a layer's export settings mean.
//!
//! [`ExportSpec`]: ondin_core::ExportSpec

// ⚠️ **`#![allow(dead_code)]` was here and is gone** (§15 D732, `[A8-L3-06]`). It
// switched the lint off for the **whole crate**, private items included — which is
// stronger than the `pub`-in-a-library blind spot `CLAUDE.md` records, because
// there the lint *cannot* fire and here it was *forbidden* to. A private helper
// that lost its last caller, and its now-false doc comment with it, would have sat
// here with all six gates green.
//
// **It was hiding nothing.** Removing it produced zero warnings under
// `-p --all-targets`, under `--workspace --all-targets` and under a plain `check`
// — so the cost was a disabled gate and the benefit was none. The one allow in
// this crate that *is* load-bearing is `tests/common/mod.rs`'s, which says why.
//
// ⚠️ **An inner attribute is invisible to the obvious census.** `grep '#\[allow('`
// cannot match `#![allow(` — the `!` sits between the `#` and the `[` — so the
// most far-reaching kind of allow there is was the kind nothing counted.
//
// The rustdoc gate, and why one of the three lints is allowed: see `ondin_core`'s
// crate doc (§15 D296).
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]

pub mod jpeg;
pub mod plan;
pub mod png;
pub mod snapshot;
pub mod svg;

pub use plan::{PlanOptions, PlannedFile, plan};
pub use snapshot::{Snapshot, snapshot};

use ondin_core::kurbo::Rect;
use ondin_core::{NodeId, Resolved};

/// The world box `origins` occupy together, or `None` when not one of them has
/// bounds at all — an empty set, or ids naming nothing.
///
/// Spelled once because both writers frame from it and they must agree: an SVG's
/// `viewBox` and a PNG's pixel grid describing different boxes is a difference
/// nobody would see until the two files were laid over each other.
///
/// **`None` rather than a fallback box**, deliberately. The two callers want
/// different answers to "there is nothing here" — the CLI frames a 512-square so a
/// blank document still writes a valid file, the SVG writer a 100-square — and a
/// helper that picked one would have the other quietly overriding it.
///
/// **`ink_bounds`, not `world_bounds`** (§5.3a). An export is a picture of what
/// the layer *draws*, and a drop shadow is drawn outside the layer's own box — so
/// framing from the editing box crops the shadow off at the layer's edge, which
/// is the whole reason those are two questions. The two are equal for any layer
/// with no effect beneath it, so this changed no existing file.
///
/// It is the selection outline and the inspector's W/H that want the other one,
/// and neither comes through here.
pub fn extent(res: &Resolved, origins: &[NodeId]) -> Option<Rect> {
    origins
        .iter()
        .filter_map(|id| res.ink_bounds(*id))
        .reduce(|a: Rect, b| a.union(b))
}
