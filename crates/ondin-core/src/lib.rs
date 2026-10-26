//! ondin-core — the headless engine.
//!
//! Model, operations, history, resolve, text layout, and IO. No GPU, windowing,
//! or UI dependency (invariant 1); fully testable without a graphics context.
//!
//! Model, operations, history, IO, and the resolve/query layer are implemented
//! (M1–M2); text layout (parley) fills in at M2b.
//!
//! **The rustdoc lints below are a gate, not decoration** (§15 D296). A doc
//! comment naming an item that no longer exists is exactly the silent drift
//! CLAUDE.md is written against, and `cargo build`/`test`/`clippy` cannot see it —
//! `broken_intra_doc_links` can, so it denies rather than warns, and
//! `invalid_html_tags` with it because a stale `<Type>` in prose is the same
//! failure wearing markup. `private_intra_doc_links` is **allowed on purpose**:
//! it fires on links that are entirely correct and merely cannot be rendered as
//! hyperlinks in *published* docs, and nobody reads these as published docs —
//! they are read in the source, where "go and look at `foo`" is worth saying
//! about a private item. It also cannot hide staleness, since a name that does
//! not exist at all is a `broken` link and not a `private` one.
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]

pub mod boolean;
pub mod build;
pub mod component;
pub mod container;
pub mod diff;
pub mod document;
pub mod effect;
pub mod export;
pub mod families;
pub mod fit;
pub mod geometry;
pub mod guide;
pub mod history;
pub mod id;
pub mod image;
pub mod io;
pub mod item;
pub mod kit;
pub mod layout;
pub mod merge;
pub mod meta;
pub mod naming;
pub mod node;
pub mod op;
pub mod page;
pub mod paint_box;
pub mod propagate;
pub mod query;
pub mod reset;
pub mod resolve;
pub mod style;
pub mod svg_in;
pub mod swap;
pub mod text;
pub mod tokens;
pub mod typography;
pub mod variable;
pub mod variant;

// Re-export the geometry/paint crates whose types appear in our public API, so
// downstream crates (and tests) use the exact same versions (§6.3).
pub use kurbo;
pub use peniko;

pub use boolean::evaluate as evaluate_boolean;
pub use build::{
    ColorUse, GradientUse, PaintAt, PaintShown, PaintTarget, Placement, any_paint_in, any_rect_in,
    bind_brush_at_all, bind_corners_ops, bind_effect_ops, bind_opacity_ops, bind_size_ops,
    bind_spacing_ops, bind_stroke_width_ops, bind_text_ops, bind_visible_ops, bindable_size_axes,
    boolean, can_parent, colors_in, detach_brush_at_all, detach_corner_ops, detach_corners_ops,
    detach_effect_ops, detach_opacity_ops, detach_size_ops, detach_spacing_ops,
    detach_stroke_width_ops, detach_text_ops, detach_visible_ops, edit_brush_at_all,
    edit_brushes_all, edit_fill_at_all, edit_fills_all, edit_stroke_at_all, edit_strokes_all,
    edit_strokes_at_all, edited_brush, gradients_in, group, insert_subtrees, keep_corner_bindings,
    keep_effect_bindings, keep_opacity_bindings, keep_size_bindings, keep_spacing_bindings,
    keep_stroke_width_bindings, keep_text_bindings, keep_visible_bindings, kept_effect_vars,
    kept_size_vars, kept_stroke_widths, local_for_world, move_by_world, outermost, paint_at,
    paint_targets, place_at_world, recolor, repaint, reparent_preserving_world, retarget_fills_all,
    retarget_strokes_all, set_boolean_op, set_brush_at_all, set_corner_radius_all, set_fills_all,
    set_opacity_all, set_strokes_all, set_top_paint_all, shared_corner_radius, shared_fills,
    shared_over_fills, shared_over_strokes, shared_strokes, subtree_nodes, ungroup, valid_opacity,
};
pub use container::{AutoMargins, Insets, LengthPct};
pub use document::{DEFAULT_CANVAS_BACKGROUND, Document, remap_subtree, reserve_existing_ids};
pub use effect::{
    BLUR_CUTOFF, BLUR_DEVIATION, DEFAULT_BLUR_RADIUS, EFFECT_BLUR, EFFECT_SPREAD, EFFECT_X,
    EFFECT_Y, Effect, EffectKind, EffectVars, FilterChannel, Filters, Shadow, deviation, reach,
    stack_escape,
};
pub use export::{ExportBackground, ExportFormat, ExportScale, ExportSpec, ExportText};
pub use guide::{
    DEFAULT_GUIDE_COLOR, Guide, GuideAxis, GuideId, guide_coord_of, guide_point, guide_span,
};
pub use history::{History, Origin, StepNote, StepView};
pub use id::{IdSource, NodeId};
pub use image::{
    AdjustPipeline, Adjustment, Brush, Framing, GradientBrush, ImageAdjust, ImageBrush, ImageEntry,
    ImageFit, ImageFormat, ImageId, ImageOrient, ImageRef, ImageSource, Missing, OrientOp,
    TONE_STEPS, crop_clamped, crop_panned, crop_reframed, crop_scaled, image_brush,
    missing_placeholder, transfer, whole_crop,
};
pub use item::{ItemId, Keyed, keyed_by_position, same_values};
pub use layout::{
    DEFAULT_GRID_COLOR, GRID_COLORS, GridAlign, GridAxis, LayoutGrid, next_grid_color,
    tracks as grid_tracks,
};
pub use meta::DocumentMeta;
pub use naming::{copy_name, created_name, free_name, is_boolean_label, numbered};
pub use node::{
    BoolOp, DashStyle, Fill, FillRule, MIN_MITER_DEGREES, MaskMode, Node, NodeKind, Paint, Pivot,
    Side, Stroke, StrokeAlign, StrokeSides, TextRef, TextSizing, miter_degrees_of_ratio,
    miter_ratio_of_degrees,
};
pub use op::{ApplyOutcome, Described, DirtySet, GeometryPatch, OpError, Operation, Transaction};
pub use query::{
    boolean_placeholder, group_chain, hit_test, is_effectively_locked, is_within, local_box,
    nodes_in_view, outline_at, pivot_world,
};
pub use resolve::Resolved;
pub use text::{
    DecorationInk, FaceFeature, FamilyPreview, FontAxis, FontVariant, GlyphRun, NamedInstance,
    TextEdit, TextLayout, TextParts, paragraph_bounds,
};
pub use typography::{
    AXES_DRIVEN_ELSEWHERE, AxisSetting, BlockStyle, BoxTrim, CharAttr, CharAttrKind, CharSpan,
    CharSpans, Decoration, FeatureSetting, ITAL, JustifyLast, Length, LengthUnit, LineStyle,
    ListMarker, MAX_FONT_SIZE, MAX_LINE_LIMIT, MAX_LIST_LEVEL, MIN_FONT_SIZE, OPSZ, OverflowWrap,
    ParaAttr, ParaAttrKind, ParaSpan, ParaSpans, ParagraphStyle, SLNT, Span, SpanAttr, Spans, Tag,
    TextAlign, TextCase, TextDirection, TextOverflow, TextStyle, VerticalAlign, WGHT, WordBreak,
    WrapMode, character_variant, stylistic_set,
};
pub use variable::{
    BrushSource, Cell, CornerVars, Mode, ModeId, SizeVars, SpacingVars, TextVars, Value, VarId,
    VarKind, VarRule, Variable, Variables,
};
