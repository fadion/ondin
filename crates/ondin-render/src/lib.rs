//! ondin-render — one scene walk, two backends (§6, invariant 11).
//!
//! `gpu` (vello + wgpu) drives the interactive canvas; `cpu` (vello_cpu) is
//! deterministic and drives export, CI and headless. Both implement
//! [`scene::ScenePainter`], so the document→scene logic — culling, z-order,
//! clipping, layers, brushes, glyph runs — exists exactly once in `scene`.
//! The sRGB→backend colour conversion lives ONLY in `color` (invariant 7).
//! Nothing outside this crate references vello.
//!
//! The rustdoc gate, and why one of the three lints is allowed: see
//! `ondin_core`'s crate doc (§15 D296).
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]

pub mod color;
pub mod cpu;
pub mod effects;
/// The effect passes as compute shaders — `effects` ported, not reinvented.
pub mod fx_gpu;
pub mod gpu;
pub mod images;
pub mod renderer;
pub mod scene;

pub use cpu::{MAX_RASTER_SIDE, VelloCpuRenderer, rasterize_glyph_run};
pub use gpu::VelloGpuRenderer;
pub use images::{
    ADJUSTED_BYTES, DECODED_BYTES, DecodeError, ImageStore, PREVIEW_EDGE, Pixels, THUMB_BASE,
    ThumbKey,
};
pub use renderer::{Ghost, GhostNode, NodeOverride, RenderOverrides, Viewport};
pub use scene::{ScenePainter, StrokePaint, TextRun};
