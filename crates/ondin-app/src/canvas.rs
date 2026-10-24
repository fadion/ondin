//! The Vello canvas embedded in egui, and the interaction on top of it (§9.2).
//!
//! Two halves that share one coordinate story (egui points → canvas device
//! pixels → world units, via `Camera`):
//!
//! - [`CanvasRenderer`] owns the wgpu texture Vello draws into and egui
//!   composites. This is the only place in the app that touches GPU resources;
//!   everything about *what* is drawn lives in `EditorSession`, which stays
//!   device-free so it can be tested and reused headlessly.
//! - The `OndinApp` methods below turn pointer and keyboard input into gestures
//!   and, on release, into exactly one `Transaction` (§9.3).
//!
//! Overlays — selection outlines, handles, marquee, pen rubber-band, caret,
//! snap guides — are drawn with egui directly. They are not document content,
//! so they never go near the renderer or the model.

use crate::app::{LoadedImage, OndinApp};
use crate::cursor::{Bitmap, Remaining, Turn};
use crate::input::Mode;
use crate::measure::{self, Axis};
use crate::preview::{
    Drag, Handle, LineEnd, PenAnchor, PenEdit, PenState, PenSubpath, PointRef, Side, Target,
    TextSession,
};
use crate::session::EditorSession;
use crate::snap;
use crate::theme::{self, color, icon};
use crate::tools::{self, ClickCreate, Tool};
use crate::ui::{Frame2, color_to_egui};
use crate::view::Camera;
use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use ondin_core::kurbo::{Affine, Point, Rect as KRect, Vec2};
use ondin_core::peniko::Color;
use ondin_core::text::TextParts;
use ondin_core::{
    CharSpans, Document, NodeId, NodeKind, Operation, ParaSpans, Pivot, Resolved, TextEdit,
    TextRef, Transaction, build, geometry, hit_test,
};
use ondin_render::{RenderOverrides, VelloGpuRenderer};

/// The offscreen texture the Vello canvas renders into, exposed to egui.
struct CanvasTexture {
    /// Kept alive for as long as `view` is registered, and the eyedropper's
    /// readback copies out of it.
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    id: egui::TextureId,
}

/// Everything about the canvas that needs a GPU. Only [`CanvasRenderer::render`]
/// and `ensure_target` touch it, so making it an `Option` is what lets the app
/// run headless in tests.
struct Gpu {
    render_state: RenderState,
    vello: VelloGpuRenderer,
    target: Option<CanvasTexture>,
    /// The eyedropper's copy of the pixels round its hotspot, in flight.
    readback: Option<Readback>,
}

/// How many device pixels the eyedropper's loupe shows on a side; odd, so one
/// of them is the hotspot.
pub(crate) const LOUPE_SIDE: u32 = 11;

/// What a [`Sample`] was read against. A sample whose key still matches is
/// what the canvas shows now, so it is neither read again nor stale for a pick.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct SampleKey {
    /// The hotspot, in canvas device pixels.
    pub at: (u32, u32),
    pub revision: u64,
    pub center: Point,
    pub zoom: f64,
    pub size: (u32, u32),
    /// A click's own number, 0 for the hover. A pick always reads afresh:
    /// uncommitted changes move pixels without moving the revision, and a stale
    /// pick is a wrong write.
    pub pick: u64,
}

/// The canvas's rendered pixels round the eyedropper's hotspot (the design's
/// rule 2): composited over the canvas ground and opaque, overlays never in it,
/// since egui paints those over this texture.
pub(crate) struct Sample {
    pub key: SampleKey,
    /// [`LOUPE_SIDE`]² pixels, row-major, sRGB; `None` past the canvas's edge.
    pub pixels: Vec<Option<[u8; 3]>>,
}

impl Sample {
    /// The hotspot's colour.
    pub fn center(&self) -> Option<[u8; 3]> {
        self.pixels[(LOUPE_SIDE * LOUPE_SIDE / 2) as usize]
    }
}

/// A copy of the texture round a hotspot, waiting on the GPU.
struct Readback {
    buf: wgpu::Buffer,
    key: SampleKey,
    /// The copied region's corner and size in device pixels — the loupe clipped
    /// to the texture.
    origin: (u32, u32),
    extent: (u32, u32),
    bytes_per_row: u32,
    /// 0 while mapping, 1 once mapped, 2 if mapping failed.
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

/// The texture region the loupe round `at` covers, clipped to `size`: its
/// corner and extent.
fn loupe_region(at: (u32, u32), size: (u32, u32)) -> ((u32, u32), (u32, u32)) {
    let half = LOUPE_SIDE / 2;
    let x0 = at.0.saturating_sub(half);
    let y0 = at.1.saturating_sub(half);
    let x1 = (at.0 + half + 1).min(size.0);
    let y1 = (at.1 + half + 1).min(size.1);
    ((x0, y0), (x1.saturating_sub(x0), y1.saturating_sub(y0)))
}

/// Lay a region's straight-alpha RGBA out as a loupe round `at`, compositing
/// each pixel over `ground`. `pixel(x, y)` reads the region at offset `(x, y)`.
fn loupe_pixels(
    at: (u32, u32),
    origin: (u32, u32),
    extent: (u32, u32),
    ground: [u8; 3],
    pixel: impl Fn(u32, u32) -> [u8; 4],
) -> Vec<Option<[u8; 3]>> {
    let half = (LOUPE_SIDE / 2) as i64;
    let mut out = Vec::with_capacity((LOUPE_SIDE * LOUPE_SIDE) as usize);
    for dy in -half..=half {
        for dx in -half..=half {
            let x = at.0 as i64 + dx - origin.0 as i64;
            let y = at.1 as i64 + dy - origin.1 as i64;
            if x < 0 || y < 0 || x >= extent.0 as i64 || y >= extent.1 as i64 {
                out.push(None);
                continue;
            }
            let [r, g, b, a] = pixel(x as u32, y as u32);
            let over = |c: u8, bg: u8| {
                ((c as u32 * a as u32 + bg as u32 * (255 - a as u32) + 127) / 255) as u8
            };
            out.push(Some([
                over(r, ground[0]),
                over(g, ground[1]),
                over(b, ground[2]),
            ]));
        }
    }
    out
}

pub struct CanvasRenderer {
    /// `None` in a headless build (`CanvasRenderer::headless`), where there is no
    /// device to draw with.
    gpu: Option<Gpu>,
    /// Decoded images (§5.5a), kept across frames. Lives on the canvas because
    /// it is renderer state derived from the document, never edited.
    images: ondin_render::ImageStore,
    /// The eyedropper's last sample, kept until a newer one arrives.
    sample: Option<Sample>,
}

impl CanvasRenderer {
    pub fn new(render_state: RenderState) -> Self {
        let vello = VelloGpuRenderer::new(&render_state.device).expect("create vello renderer");
        Self {
            gpu: Some(Gpu {
                render_state,
                vello,
                target: None,
                readback: None,
            }),
            // The one store that may answer a moving adjustment with a reduced
            // picture; export takes `ImageStore::new`, which never previews.
            images: ondin_render::ImageStore::interactive(),
            sample: None,
        }
    }

    /// A canvas with no device behind it, for `OndinApp::headless`. The decode
    /// cache stays real, since the layers panel and the placeholder rule read it.
    #[cfg(test)]
    pub(crate) fn headless() -> Self {
        Self {
            gpu: None,
            images: ondin_render::ImageStore::interactive(),
            sample: None,
        }
    }

    /// The decode cache, for the one caller that puts something into it: an
    /// image being picked is decoded before the tool is armed, so a file that
    /// cannot be drawn never becomes a placement.
    pub fn images_mut(&mut self) -> &mut ondin_render::ImageStore {
        &mut self.images
    }

    /// The decode cache, read-only. The same store the walk asks, so the layers
    /// panel's missing-picture mark agrees with the canvas's placeholder.
    pub fn images(&self) -> &ondin_render::ImageStore {
        &self.images
    }

    /// Render `doc`/`res` through `camera` at `pixel_size` device pixels and
    /// return the egui texture to draw. Recreates the target on size change.
    /// `backdrop` is the ground round the frames, set by the Appearance panel.
    pub fn render(
        &mut self,
        doc: &Document,
        res: &Resolved,
        overrides: &RenderOverrides,
        camera: &Camera,
        pixel_size: (u32, u32),
        backdrop: Color,
    ) -> Option<egui::TextureId> {
        if pixel_size.0 == 0 || pixel_size.1 == 0 {
            return None;
        }
        let vp = camera.viewport(pixel_size);
        // Every frame on purpose: it is cheap once decoded, and a missed change
        // signal (undo) would leave a picture that does not update.
        self.images.prepare(doc);
        // Runs headless too, before the device check: the layers panel reads it.
        let gpu = self.gpu.as_mut()?;
        gpu.ensure_target(pixel_size);
        let target = gpu.target.as_ref()?;
        gpu.vello
            .render(
                &gpu.render_state.device,
                &gpu.render_state.queue,
                doc,
                res,
                &vp,
                overrides,
                &target.view,
                backdrop,
                &self.images,
            )
            .ok()?;
        Some(target.id)
    }

    /// The eyedropper's latest sample, whatever it was read against.
    pub(crate) fn sample(&self) -> Option<&Sample> {
        self.sample.as_ref()
    }

    /// Read the pixels round `key.at` off what [`Self::render`] just drew, and
    /// collect a read that has landed. Returns true while one is in flight, so
    /// the caller asks for the frame that will collect it.
    ///
    /// The copy is mapped without waiting, so frames never stall on the device.
    /// A key matching the last sample is not re-read, which keeps a still pointer
    /// from repainting forever. Headless, the region is drawn on the CPU at once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn request_sample(
        &mut self,
        key: SampleKey,
        doc: &Document,
        res: &Resolved,
        camera: &Camera,
        ground: Color,
    ) -> bool {
        let [gr, gg, gb, _] = ground.to_rgba8().to_u8_array();
        let ground = [gr, gg, gb];
        let fresh = self.sample.as_ref().is_some_and(|s| s.key == key);
        let Some(gpu) = self.gpu.as_mut() else {
            if !fresh {
                let (origin, extent) = loupe_region(key.at, key.size);
                self.sample = Some(Sample {
                    key,
                    pixels: cpu_region(doc, res, camera, key, origin, extent, ground, &self.images),
                });
            }
            return false;
        };
        if let Some(rb) = &gpu.readback {
            let _ = gpu.render_state.device.poll(wgpu::PollType::Poll);
            match rb.state.load(std::sync::atomic::Ordering::Acquire) {
                0 => return true,
                1 => {
                    let rb = gpu.readback.take().expect("checked above");
                    let data = rb.buf.slice(..).get_mapped_range();
                    let pixels = loupe_pixels(rb.key.at, rb.origin, rb.extent, ground, |x, y| {
                        let i = (y * rb.bytes_per_row + x * 4) as usize;
                        [data[i], data[i + 1], data[i + 2], data[i + 3]]
                    });
                    drop(data);
                    rb.buf.unmap();
                    self.sample = Some(Sample {
                        key: rb.key,
                        pixels,
                    });
                }
                _ => gpu.readback = None,
            }
        }
        if self.sample.as_ref().is_some_and(|s| s.key == key) {
            return false;
        }
        let Some(target) = gpu.target.as_ref().filter(|t| t.size == key.size) else {
            return false;
        };
        let (origin, extent) = loupe_region(key.at, key.size);
        if extent.0 == 0 || extent.1 == 0 {
            return false;
        }
        // `copy_texture_to_buffer` wants rows on a 256-byte boundary.
        let bytes_per_row = (extent.0 * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let device = &gpu.render_state.device;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ondin-eyedropper"),
            size: (bytes_per_row * extent.1) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin.0,
                    y: origin.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(extent.1),
                },
            },
            wgpu::Extent3d {
                width: extent.0,
                height: extent.1,
                depth_or_array_layers: 1,
            },
        );
        gpu.render_state.queue.submit([enc.finish()]);
        let state = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        let flag = state.clone();
        buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            flag.store(
                if r.is_ok() { 1 } else { 2 },
                std::sync::atomic::Ordering::Release,
            );
        });
        gpu.readback = Some(Readback {
            buf,
            key,
            origin,
            extent,
            bytes_per_row,
            state,
        });
        true
    }
}

/// The loupe's region drawn on the CPU — the headless canvas's sample, which has
/// no texture to read. The same viewport mapping as the canvas, cut to `origin`
/// and `extent`, so its device pixels are the texture's.
#[allow(clippy::too_many_arguments)]
fn cpu_region(
    doc: &Document,
    res: &Resolved,
    camera: &Camera,
    key: SampleKey,
    origin: (u32, u32),
    extent: (u32, u32),
    ground: [u8; 3],
    images: &ondin_render::ImageStore,
) -> Vec<Option<[u8; 3]>> {
    if extent.0 == 0 || extent.1 == 0 {
        return vec![None; (LOUPE_SIDE * LOUPE_SIDE) as usize];
    }
    let view = camera.viewport(key.size).view;
    let per_px = 1.0 / camera.zoom;
    let x0 = view.x0 + origin.0 as f64 * per_px;
    let y0 = view.y0 + origin.1 as f64 * per_px;
    let vp = ondin_render::Viewport {
        view: KRect::new(
            x0,
            y0,
            x0 + extent.0 as f64 * per_px,
            y0 + extent.1 as f64 * per_px,
        ),
        pixel_size: extent,
    };
    let (rgba, w, _) = ondin_render::VelloCpuRenderer::new().render_to_rgba(doc, res, &vp, images);
    loupe_pixels(key.at, origin, extent, ground, |x, y| {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    })
}

impl Gpu {
    fn ensure_target(&mut self, pixel_size: (u32, u32)) {
        if self.target.as_ref().is_some_and(|c| c.size == pixel_size) {
            return;
        }
        if let Some(old) = self.target.take() {
            self.render_state.renderer.write().free_texture(&old.id);
        }
        let texture = self
            .render_state
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("ondin-canvas"),
                size: wgpu::Extent3d {
                    width: pixel_size.0,
                    height: pixel_size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let id = self.render_state.renderer.write().register_native_texture(
            &self.render_state.device,
            &view,
            wgpu::FilterMode::Nearest,
        );
        self.target = Some(CanvasTexture {
            texture,
            view,
            size: pixel_size,
            id,
        });
    }
}

// --- interaction ----------------------------------------------------------

/// How much of the photograph shows outside the crop window, as a multiplier on
/// the layer's own opacity.
const GHOST_ALPHA: f32 = 0.3;

/// A component set's tab's padding round its text.
const SET_TAB_PAD: egui::Vec2 = egui::vec2(6.0, 3.0);

/// A frame tag's point size.
const LABEL_PT: f32 = 11.0;

/// The padding inside a main's chip: its tag on a neutral ground.
const CHIP_PAD: egui::Vec2 = egui::vec2(4.0, 2.0);

/// The radius of a selected set's `+` disc on its bottom edge, in points.
const SET_PLUS_R: f32 = 8.0;

/// The id the crop ghost wears — see `OndinApp::image_ghost`.
const GHOST_ID: NodeId = NodeId {
    actor: u64::MAX,
    seq: u64::MAX,
};

/// How close the pointer must be to grab a corner handle, in logical points.
/// Every pointer-aimed `*_PX` allowance below is in points too.
pub(crate) const HANDLE_PICK_PX: f32 = 9.0;
/// The handle square, in screen px.
const HANDLE_PX: f32 = 7.0;
/// The handle's border. Handles are hollow, since a filled square hides the
/// corner it is there to place.
const HANDLE_STROKE_PX: f32 = 2.0;
/// The crop window's dash: length, gap, and weight. Heavier than the selection
/// hairline so the crop box never reads as the selection box.
const CROP_DASH_PX: f32 = 6.0;
const CROP_GAP_PX: f32 = 6.0;
const CROP_STROKE_PX: f32 = 3.0;
/// How far off a stroke-only shape (a `Line`) a click still counts, in screen
/// points.
const PICK_SLOP_PX: f32 = 4.0;
/// How far either side of an occupied frame's border a press takes the frame
/// ([`OndinApp::frame_edge_at`]); the chain's last link, so a layer wins its pixels.
const FRAME_EDGE_PICK_PX: f32 = 8.0;
/// How close to a pen anchor or a path's segment counts as hitting it, in
/// screen pt. The pen's snap and the click must share this one number.
const PEN_PICK_PX: f32 = 7.0;
/// What the node tool would take at a given point.
///
/// Tried in order: a handle (beats its anchor, which it can sit on), an anchor
/// (beats a box corner), a box corner or side (beats a segment flush with it),
/// a segment, then a marquee.
#[derive(Clone, Copy, PartialEq, Debug)]
enum NodeGrab {
    Anchor(PointRef),
    Handle(PointRef, Side),
    /// A corner or side of the box over the selected points, which scales the set.
    Box(Handle),
    /// The band just outside a corner of that box, which turns the set.
    BoxRotate,
    /// The segment leaving this anchor, at parameter `t`: a press on the ink but
    /// on none of its points.
    Segment(PointRef, f64),
}

/// What a press would do while the pen is armed inside the node tool's edit
/// (`app::pen_bias`, §15 D125) — or `None` when the bias is off, or when the
/// target under the pointer is one the node tool keeps.
///
/// An open end continues the path, its ink inserts a point, bare canvas starts
/// another subpath; middle anchors, handles and the point box keep node-tool meanings.
enum PenVerb {
    /// An open end of the edited path: pick it up and keep drawing. Carries the
    /// [`PenEdit`] `pen_resume` would build, so hover ring and press agree.
    Continue(Box<PenEdit>),
    /// The path's ink: add an anchor to the segment leaving `.0` at parameter `.1`.
    Insert(PointRef, f64),
    /// Bare canvas: begin a fresh subpath in this same node.
    NewSubpath,
}

/// Length above which an anchor's handle is drawn, and therefore grabbable.
/// `draw_points` and `node_grab` both ask this, so a grab never lands on an
/// undrawn zero-length handle.
const HANDLE_DRAWN: f64 = 1e-9;
/// How close (screen pt) a dragged handle has to come to collinear with its
/// partner, or one of the eight axes and diagonals, before it lands on it
/// (`tools::tangent_snap`).
const TANGENT_SNAP_PX: f32 = 4.0;
/// The ring that marks an armed snap target: radius and weight, in screen pt.
/// Deliberately not [`PEN_PICK_PX`], which is a target area.
const SNAP_RING_PX: f32 = 5.0;
const SNAP_RING_STROKE_PX: f32 = 2.0;
/// Side of a path anchor's marker in the node tool, in screen pt. Its corners
/// sit just inside `PEN_PICK_PX`; raise that too, or they stop being grabbable.
const POINT_PX: f32 = 9.0;
/// The marker's outline, and what it becomes under the pointer. Hover is on the
/// stroke, laid inside the edge so the silhouette does not swell
/// (`the_hover_weight_does_not_change_a_markers_silhouette`).
const POINT_STROKE_PX: f32 = 1.0;
const POINT_STROKE_HOVER_PX: f32 = 2.0;
/// Radius of the dot on the end of a handle's stalk, in screen pt.
const HANDLE_DOT_PX: f32 = 3.0;
/// How close to a side of the selection box counts as grabbing that side.
const EDGE_PICK_PX: f32 = 6.0;
/// How far outside a handle the rotation ring reaches.
const ROTATE_RING_PX: f32 = 15.0;
/// Drag distance (screen px) below which a shape-create drag is discarded. A
/// still press is a click, and `click_create` makes a default-sized shape.
const MIN_CREATE_PX: f64 = 3.0;
/// Cap on how many neighbours a snap considers, so a dense viewport cannot make
/// dragging quadratic.
pub(crate) const SNAP_TARGET_LIMIT: usize = 200;
/// Rotation snap step with Shift held: 15°.
const ROTATE_SNAP: f64 = std::f64::consts::FRAC_PI_2 / 6.0;
/// Type size for the pivot marker's glyph; Phosphor inks about `size - 4`.
const PIVOT_GLYPH_PX: f32 = 22.0;
/// How close the pointer must be to grab the pivot marker — a corner handle's
/// distance, since the pivot beats the handles.
const PIVOT_PICK_PX: f32 = 9.0;
/// How close (screen px) a dragged pivot has to come to one of the box's nine
/// points to land on it.
const PIVOT_SNAP_PX: f32 = 6.0;
/// Band at the canvas edge inside which a drag in flight scrolls the view, in
/// points.
const AUTOPAN_BAND: f32 = 32.0;
/// Fastest the view auto-scrolls, in screen points per second.
const AUTOPAN_MAX: f32 = 900.0;
/// Longest frame the auto-scroll integrates over, so a hitch does not throw the
/// view across the document.
const AUTOPAN_MAX_DT: f32 = 1.0 / 15.0;

/// The canvas band's share of [`crate::ui::edge_scroll_speed`], whose curve the
/// layers tree shares.
fn autopan_axis(p: f32, min: f32, max: f32) -> f32 {
    crate::ui::edge_scroll_speed(p, min, max, AUTOPAN_BAND, AUTOPAN_MAX)
}

/// One auto-scroll frame, as the argument [`Camera::pan_pixels`] takes (device
/// px), or `None` when the pointer is clear of both bands on both axes.
///
/// Negated: `pan_pixels` takes the delta the content moves by, and auto-scroll
/// moves the view toward the pointer. The sign has a test of its own.
fn autopan_delta(p: egui::Pos2, rect: egui::Rect, dt: f32, ppp: f32) -> Option<Vec2> {
    let vx = autopan_axis(p.x, rect.left(), rect.right());
    let vy = autopan_axis(p.y, rect.top(), rect.bottom());
    if vx == 0.0 && vy == 0.0 {
        return None;
    }
    Some(-Vec2::new(
        f64::from(vx * dt * ppp),
        f64::from(vy * dt * ppp),
    ))
}

/// Default fill for drawn rectangles/ellipses (Figma's neutral #D9D9D9).
fn default_shape_fill() -> Color {
    Color::from_rgba8(217, 217, 217, 255)
}

/// The stroke a freshly drawn pen path and line get, which the pen's preview
/// shares.
const PEN_STROKE_COLOR: Color = Color::BLACK;
const PEN_STROKE_WIDTH: f64 = 2.0;

/// The transform box and its four corners, in screen space.
///
/// For a single node this is its own local box through its world transform; for a
/// multi-selection the union in the set's session orientation (§15 D326), with
/// `world` the identity while that is upright.
struct Handles {
    target: Target,
    /// World-space point a rotation turns about: the node's `ondin_core::Pivot`,
    /// or the union box's centre. Rotation only; an Alt-resize holds the centre.
    pivot: Point,
    /// Box → world, for turning a handle's box-local direction into a screen one.
    world: Affine,
    /// The four corners, clockwise from the top-left. The box itself is not
    /// kept: `resize_tx` recomputes it from the committed tree, since scaling
    /// against the mid-drag preview compounds every frame.
    corners: [(Handle, egui::Pos2); 4],
    /// The selection holds a locked layer: the handles are drawn and arm nothing.
    locked: bool,
}

/// Which pivot marker to draw: draggable, or a read-only hint that the pivot
/// was moved. Two glyph shapes rather than two tints, so it survives small sizes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Marker {
    /// The toggle is on and this pivot is draggable.
    Editable,
    /// Shown on hover on a layer whose pivot has been placed, and nothing more.
    ReadOnly,
}

impl Marker {
    fn glyph(self) -> &'static str {
        match self {
            Marker::Editable => icon::CROSSHAIR,
            Marker::ReadOnly => icon::CROSSHAIR_SIMPLE,
        }
    }
}

/// What the pointer is over on a selection box: a part that resizes, a side
/// that leans under Ctrl, or the band outside a corner that rotates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Armed {
    Resize(Handle),
    Skew(Handle),
    Rotate(Handle),
}

/// The copy an Alt-drag is carrying: one remapped subtree per selected layer,
/// prepared when Alt went down ([`OndinApp::sync_alt_clone`]).
pub(crate) struct AltClone {
    copies: Vec<ClonedSubtree>,
    /// Where the copy sits relative to its original, in world units, stashed by
    /// the gesture so the chrome can follow ([`OndinApp::alt_clone_offset`]).
    delta: Vec2,
}

/// One layer's worth of an Alt-drag copy, with its ids already minted.
struct ClonedSubtree {
    /// The layer being copied: the copy is offset from its world transform, and
    /// its frame membership decides the copy's.
    source: NodeId,
    /// The source's parent — where the copy lands if the drop crosses no frame.
    parent: NodeId,
    /// The remapped subtree, ready for `Operation::InsertSubtree`.
    nodes: Vec<ondin_core::Node>,
    /// The remapped root's id, which gets positioned and then selected.
    root: NodeId,
}

impl Handles {
    /// The corner handle under `screen`, if any is close enough to grab.
    fn grab(&self, screen: egui::Pos2) -> Option<Handle> {
        self.corners
            .iter()
            .filter(|(_, pos)| screen.distance(*pos) <= HANDLE_PICK_PX)
            .min_by(|a, b| screen.distance(a.1).total_cmp(&screen.distance(b.1)))
            .map(|(h, _)| *h)
    }

    /// The side of the box `screen` is hugging, if any. `Handle::EDGES[i]` runs
    /// from corner `i` to corner `i + 1`.
    fn grab_edge(&self, screen: egui::Pos2) -> Option<Handle> {
        Handle::EDGES
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let a = self.corners[i].1;
                let b = self.corners[(i + 1) % 4].1;
                (*h, distance_to_segment(screen, a, b))
            })
            .filter(|(_, d)| *d <= EDGE_PICK_PX)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
    }

    /// The corner whose rotation band `screen` falls in — outside the box only,
    /// since inside the box a press means move.
    fn rotate_zone(&self, screen: egui::Pos2) -> Option<Handle> {
        if self.contains(screen) {
            return None;
        }
        self.corners
            .iter()
            .find(|(_, pos)| {
                let d = screen.distance(*pos);
                d > HANDLE_PICK_PX && d <= HANDLE_PICK_PX + ROTATE_RING_PX
            })
            .map(|(h, _)| *h)
    }

    /// Whether `screen` is inside the (possibly rotated or sheared) selection
    /// box — a convex-quad test.
    fn contains(&self, screen: egui::Pos2) -> bool {
        let (mut left, mut right) = (false, false);
        for i in 0..4 {
            let a = self.corners[i].1;
            let b = self.corners[(i + 1) % 4].1;
            let cross = (b.x - a.x) * (screen.y - a.y) - (b.y - a.y) * (screen.x - a.x);
            left |= cross > 0.0;
            right |= cross < 0.0;
        }
        // Consistently on one side of every edge — a mirrored box winds the
        // other way, so neither sign alone can be the test.
        !(left && right)
    }

    /// What a press at `screen` would do to the selection — the one statement of
    /// the priority, so cursor, gesture and hover outline agree. Corners beat
    /// sides, sides beat rotation. `skewing` (Ctrl) re-reads the side bands only.
    fn armed(&self, screen: egui::Pos2, skewing: bool) -> Option<Armed> {
        if self.locked {
            return None;
        }
        if let Some(h) = self.grab(screen) {
            return Some(Armed::Resize(h));
        }
        if let Some(h) = self.grab_edge(screen) {
            return Some(if skewing {
                Armed::Skew(h)
            } else {
                Armed::Resize(h)
            });
        }
        self.rotate_zone(screen).map(Armed::Rotate)
    }

    /// Where a handle faces on screen, which the resize cursor points along.
    fn screen_direction(&self, handle: Handle) -> egui::Vec2 {
        let (ux, uy) = handle.outward();
        let [a, b, c, d, _, _] = self.world.as_coeffs();
        let v = Vec2::new(a * ux + c * uy, b * ux + d * uy);
        egui::vec2(v.x as f32, v.y as f32)
    }

    /// Where a side runs on screen, which the skew cursor points along. Turned a
    /// quarter in the box's own frame, so a sheared box reports its real direction.
    fn screen_tangent(&self, handle: Handle) -> egui::Vec2 {
        let (ux, uy) = handle.outward();
        let [a, b, c, d, _, _] = self.world.as_coeffs();
        let v = Vec2::new(a * -uy + c * ux, b * -uy + d * ux);
        egui::vec2(v.x as f32, v.y as f32)
    }
}

/// Where a handle sits on an upright world-space box — the point box's corners
/// and the midpoints of its sides.
fn box_handle_at(box_: KRect, handle: Handle) -> Point {
    let (ux, uy) = handle.unit();
    Point::new(
        box_.min_x() + box_.width() * ux,
        box_.min_y() + box_.height() * uy,
    )
}

/// How far `p` is from the side of `box_` that `handle` names, in world units.
/// Measured to the segment, so the band does not run on past the corners.
fn box_edge_distance(box_: KRect, handle: Handle, p: Point) -> f64 {
    let (a, b) = match handle {
        Handle::Top => (box_.min_x(), box_.max_x()),
        Handle::Bottom => (box_.min_x(), box_.max_x()),
        Handle::Left | Handle::Right => (box_.min_y(), box_.max_y()),
        // A corner names no side; keep it out of the band test entirely.
        _ => return f64::INFINITY,
    };
    let (along, across, edge) = match handle {
        Handle::Top => (p.x, p.y, box_.min_y()),
        Handle::Bottom => (p.x, p.y, box_.max_y()),
        Handle::Left => (p.y, p.x, box_.min_x()),
        _ => (p.y, p.x, box_.max_x()),
    };
    let over = (along.clamp(a, b) - along).abs();
    (across - edge).hypot(over)
}

/// Shortest distance from `p` to the segment `a`..`b`.
fn distance_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// A flattened subpath in screen space (see [`OndinApp::flatten_to_screen`]).
struct Polyline {
    pts: Vec<egui::Pos2>,
    closed: bool,
}

impl Polyline {
    /// The subpath as a stroked shape, closed or open as the path was.
    fn stroked(self, stroke: egui::Stroke) -> egui::Shape {
        match (self.pts.len(), self.closed) {
            (0..=1, _) => egui::Shape::Noop,
            (_, true) => egui::Shape::closed_line(self.pts, stroke),
            (_, false) => egui::Shape::line(self.pts, stroke),
        }
    }
}

/// A grid container's track lines and gaps in its own space
/// ([`OndinApp::track_lines`]).
#[derive(Debug, Default)]
struct TrackLines {
    lines: Vec<(Point, Point, bool)>,
    gaps: Vec<KRect>,
}

#[cfg(test)]
thread_local! {
    /// How many grids `OndinApp::draw_grid_tracks` has laid on this thread, so a
    /// test can see the container-wide cull skip a layout pass.
    static GRIDS_LAID: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What [`OndinApp::import_svg`] did with some markup.
pub(crate) enum SvgImport {
    /// Committed and selected; what was made, as `5 layers, skipped filter`.
    Placed(String),
    /// Nested past what the reader will descend into.
    TooDeep,
    /// Not SVG, no shapes in it, or the build did not apply.
    Unreadable,
}

/// Where the pictures inside a pasted SVG go (`ondin_core::svg_in::ImageSink`).
/// The same steps as `OndinApp::load_image_bytes`, which cannot be called
/// because the importer already borrows `self.session.ids` mutably.
struct SvgImages<'a> {
    images: &'a mut ondin_render::ImageStore,
}

impl ondin_core::svg_in::ImageSink for SvgImages<'_> {
    fn place(
        &mut self,
        bytes: Vec<u8>,
        format: ondin_core::ImageFormat,
    ) -> Option<(ondin_core::ImageId, ondin_core::ImageEntry)> {
        let id = crate::app::content_hash(&bytes);
        // The decode both proves the file drawable and supplies its size.
        let (w, h) = self.images.insert(&id, &bytes, format).ok()?;
        Some((
            id,
            ondin_core::ImageEntry {
                source: ondin_core::ImageSource::Embedded(bytes.into()),
                format,
                width: w,
                height: h,
            },
        ))
    }
}

impl OndinApp {
    pub(crate) fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        let size = ui.available_size();
        let (resp, painter) = ui.allocate_painter(size, egui::Sense::click_and_drag());
        let rect = resp.rect;
        let ppp = ui.ctx().pixels_per_point();
        self.canvas_px = (
            ((rect.width() * ppp).round() as u32).max(1),
            ((rect.height() * ppp).round() as u32).max(1),
        );
        self.canvas_ppp = ppp;
        // `fold_chrome_hold` runs before this and reads the previous frame's.
        self.canvas_rect = rect;

        self.wheel_input(ui, &resp, rect, ppp);

        self.canvas_cursor(ui, &resp, rect, ppp);

        // Before the mode input, so this frame's gesture reads the panned camera
        // and a still hand's drag keeps advancing.
        self.autopan(ui, &resp, rect, ppp);

        // A click that dismisses a menu is spent doing so (`docs/context-menus.md`
        // §0 R4); `context_menu_ui` handles the dismissal itself further down.
        if self.context_menu.is_none() {
            match self.mode {
                // A picker opened over a text session can pick too.
                Mode::TextInsert if self.dropper_armed().is_some() => {
                    if !self.gesture_cancelled {
                        self.dropper_input(ui, &resp, rect, ppp);
                    }
                }
                Mode::TextInsert => self.text_mode_input(ui, &resp, rect, ppp),
                Mode::Normal => self.normal_mode_input(ui, &resp, rect, ppp),
            }
        }

        // After the mode input, so a gesture the same button cancelled has
        // already set the flag that spends this click (§0 R1).
        self.canvas_context_menu(ui, &resp, rect, ppp);

        self.draw_canvas(ui, &painter, rect, ppp, &resp);
    }

    /// The canvas's door onto the context menu (`docs/context-menus.md` §2).
    ///
    /// Opens on release at the press position (`secondary_press`), so jitter does
    /// not shift it. Claimants are asked in §2's order: mode, guide, layer, page.
    fn canvas_context_menu(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) {
        if !resp.secondary_clicked() {
            return;
        }
        // A right-click that cancelled a gesture is spent, selection included: the
        // branches below select before `open_context_menu` would refuse.
        if self.gesture_cancelled {
            return;
        }
        let ctx = ui.ctx().clone();
        let screen = self
            .secondary_press
            .or_else(|| resp.interact_pointer_pos())
            .unwrap_or_default();
        let world = self.to_world(screen, rect, ppp);

        // C8: a live text session or point edit claims its own menu.
        if self.mode == Mode::TextInsert {
            self.open_context_menu(&ctx, crate::menu::Target::TextSession, Some(world));
            return;
        }
        if self.tool == Tool::Node && self.edited_path().is_some() {
            self.open_context_menu(&ctx, crate::menu::Target::Points, Some(world));
            return;
        }
        // C7: guides out-rank layers while unlocked (`guide_at` answers `None`
        // once locked, which is why the canvas menu carries *Lock guides*).
        if let Some(g) = self.guide_at(screen, rect, ppp) {
            self.session.selection.set_guide(g);
            self.open_context_menu(&ctx, crate::menu::Target::Guide(g), Some(world));
            return;
        }
        // C1/C5: the target is whatever a left-click would have selected, by the
        // same chain (`pick_at_pointer`, which includes an occupied frame's name
        // tag and edge).
        let Some(leaf) = self.pick_at_pointer(ui, screen, world, rect, ppp) else {
            // C3: empty canvas does not clear the selection.
            self.open_context_menu(&ctx, crate::menu::Target::Canvas, Some(world));
            return;
        };
        let id = self.pick_for_click(leaf, ui);
        // C2: a hit inside the selection leaves it whole; outside replaces it.
        if !self.session.selection.contains(id) {
            self.session.selection.set_one(id);
        }
        self.open_context_menu(
            &ctx,
            crate::menu::Target::Layer {
                id,
                door: crate::menu::Door::Canvas,
            },
            Some(world),
        );
    }

    /// Scroll the view while a gesture is dragged into the canvas edge; which
    /// drags qualify is [`Drag::autopans`]'s decision.
    ///
    /// Runs before the input handlers, so the gesture sees the new camera this
    /// frame. It must `request_repaint`, or a still hand at the edge gets no more
    /// frames. Snap candidates re-cull as the view moves, on purpose.
    fn autopan(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, ppp: f32) {
        if !self.drag.autopans() {
            return;
        }
        // The pointer keeps reporting past the widget's edge while a drag holds it.
        let Some(p) = resp.interact_pointer_pos() else {
            return;
        };
        let dt = ui.input(|i| i.stable_dt).min(AUTOPAN_MAX_DT);
        let Some(pan) = autopan_delta(p, rect, dt, ppp) else {
            return;
        };
        let before = self.session.camera.center;
        self.session.camera.pan_pixels(pan);
        // World units, so `cancel_gesture` can undo the pan without undoing a zoom.
        self.autopan_total += self.session.camera.center - before;
        ui.ctx().request_repaint();
    }

    /// The cursor over the canvas: what the armed gesture or the active tool is
    /// about to do. Panning outranks the tool; otherwise the tool decides.
    ///
    /// Only claims a cursor, never resets one off-canvas: egui starts each frame
    /// at `Default`, and a reset would erase one the layers panel claimed earlier.
    fn canvas_cursor(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, ppp: f32) {
        let ctx = ui.ctx();
        // `hovered` goes false once a drag leaves the widget.
        let on_canvas = resp.hovered() || resp.dragged();
        if !on_canvas {
            return;
        }

        let (space, middle) = ui.input(|i| (i.key_down(egui::Key::Space), i.pointer.middle_down()));
        let panning = matches!(self.drag, Drag::Pan);
        // Space repositions a shape being drawn instead (`reshape_create`).
        let armed = ((space && self.mode == Mode::Normal) || self.tool == Tool::Hand)
            && !matches!(self.drag, Drag::Create { .. });

        // `icon` is the stock fallback when `bitmap` is absent or cannot rasterize.
        let pointer = resp.hover_pos().or(resp.interact_pointer_pos());
        let (icon, bitmap) = if panning || middle {
            (egui::CursorIcon::Grabbing, Some(Bitmap::HandClosed))
        } else if armed {
            (egui::CursorIcon::Grab, Some(Bitmap::HandOpen))
        } else if self.dropper_armed().is_some() {
            // The loupe is the cursor; its ringed middle pixel is the hotspot.
            (egui::CursorIcon::None, None)
        } else if self.text.is_some()
            && (resp.dragged() || pointer.is_some_and(|p| self.over_edited_text(p, rect, ppp)))
        {
            // A live text session owns the cursor over its layer; `dragged()`
            // keeps it through a selection that overshoots the box.
            (egui::CursorIcon::Text, None)
        } else if let Some(cursor) = pointer.and_then(|p| self.ruler_cursor(p, rect)) {
            // The ruler bar takes the press from every tool, so the cursor too.
            cursor
        } else {
            match self.tool {
                // A drawn crosshair: the stock `IDC_CROSS` is 32 texels across.
                Tool::Frame
                | Tool::Rect
                | Tool::Ellipse
                | Tool::Polygon
                | Tool::Star
                | Tool::Line => (egui::CursorIcon::Crosshair, Some(Bitmap::Crosshair)),
                // The same crosshair, badged with how many images are left to place.
                Tool::Image => (
                    egui::CursorIcon::Crosshair,
                    Some(match Remaining::of(self.pending_images.len()) {
                        Some(n) => Bitmap::Loaded(n),
                        None => Bitmap::Crosshair,
                    }),
                ),
                Tool::Pen => (egui::CursorIcon::Crosshair, Some(Bitmap::Pen)),
                // The plain arrow over points, whose markers are the affordance;
                // the point box gets resize arrows ([`Self::point_box_cursor`]),
                // and the nib shows where `pen_verb`, the press's query, arms.
                Tool::Node => match self.point_box_cursor(resp, rect, ppp) {
                    Some(c) => c,
                    None => {
                        // A path in flight is always the pen; else only where a
                        // verb is armed, never mid-drag.
                        let armed = self.pen.is_some()
                            || (self.drag.is_none()
                                && pointer
                                    .map(|p| self.to_world(p, rect, ppp))
                                    .is_some_and(|w| self.pen_verb(w).is_some()));
                        match armed {
                            true => (egui::CursorIcon::Crosshair, Some(Bitmap::Pen)),
                            false => (egui::CursorIcon::Default, None),
                        }
                    }
                },
                // An I-beam over a text layer, else the crosshair; not re-read
                // mid-drag.
                Tool::Text => {
                    let world = (self.drag.is_none())
                        .then(|| pointer.map(|p| self.to_world(p, rect, ppp)))
                        .flatten();
                    // The press's order: existing text beats an outline.
                    match world {
                        Some(w) if self.text_layer_at(w).is_some() => {
                            (egui::CursorIcon::Text, None)
                        }
                        // An outline under the pointer: a click writes along it.
                        Some(w) if self.rail_at(w).is_some() => {
                            (egui::CursorIcon::Text, Some(Bitmap::TextOnPath))
                        }
                        _ => (egui::CursorIcon::Crosshair, Some(Bitmap::Crosshair)),
                    }
                }
                // The loupe arm above answers first; this is the fallback.
                Tool::Eyedropper => (egui::CursorIcon::None, None),
                // Handles get resize cursors from `crop_hover`.
                Tool::ImageEdit => match self.drag {
                    Drag::CropPan { .. } => (egui::CursorIcon::Grabbing, Some(Bitmap::HandClosed)),
                    _ => match self.crop_hover(resp, rect, ppp) {
                        Some(c) => c,
                        None => (egui::CursorIcon::Default, None),
                    },
                },
                // Select and Scale differ only in the rest state.
                Tool::Select | Tool::Scale => {
                    // The press order (`normal_mode_input`): pivot, handles, guide, rest.
                    if matches!(self.drag, Drag::Pivot { .. })
                        || pointer.is_some_and(|p| self.on_pivot(p, rect, ppp).is_some())
                    {
                        (egui::CursorIcon::Move, None)
                    } else {
                        let handle = self.select_cursor(resp, rect, ppp);
                        match (handle, pointer) {
                            ((egui::CursorIcon::Default, None), Some(p)) => {
                                match self.guide_cursor(p, rect, ppp) {
                                    Some(icon) => (icon, None),
                                    None => self.rest_cursor(),
                                }
                            }
                            ((egui::CursorIcon::Default, None), None) => self.rest_cursor(),
                            _ => handle,
                        }
                    }
                }
                Tool::Hand => (egui::CursorIcon::Default, None),
            }
        };
        ctx.set_cursor_icon(icon);
        ctx.set_cursor_image(bitmap.and_then(|b| self.cursors.image(ctx, b)));
    }

    fn rest_cursor(&self) -> (egui::CursorIcon, Option<Bitmap>) {
        rest_cursor(self.tool)
    }

    /// What the Select tool's cursor should be: what the handle under (or just
    /// outside) the pointer will do, else an arrow. Directions are measured in
    /// screen space, so they are right on a rotated layer.
    fn select_cursor(
        &self,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) -> (egui::CursorIcon, Option<Bitmap>) {
        const NONE: (egui::CursorIcon, Option<Bitmap>) = (egui::CursorIcon::Default, None);
        // Mid-gesture the cursor reports the gesture, so a resize dragged past the
        // opposite corner does not flicker.
        let handles = self.selection_handles(rect, ppp);
        match &self.drag {
            // The arrow faces from the pivot to the pointer, not the grabbed corner.
            Drag::Rotate { pivot, .. } => {
                let facing = resp
                    .hover_pos()
                    .or_else(|| resp.interact_pointer_pos())
                    .map(|p| Turn::toward(p - self.to_screen(*pivot, rect, ppp)))
                    .unwrap_or_default();
                return (egui::CursorIcon::Grabbing, Some(Bitmap::Rotate(facing)));
            }
            Drag::Resize { handle, .. } => {
                return match &handles {
                    Some(h) => (resize_icon(h.screen_direction(*handle)), None),
                    None => NONE,
                };
            }
            // Along the side, held even if Ctrl is let go part way.
            Drag::Skew { handle, .. } => {
                return match &handles {
                    Some(h) => (resize_icon(h.screen_tangent(*handle)), None),
                    None => NONE,
                };
            }
            Drag::LineEnd { .. } => return (egui::CursorIcon::Grabbing, None),
            Drag::RailOffset { .. } => return (egui::CursorIcon::Grabbing, None),
            Drag::Spacing { id, band, .. } => return (self.spacing_cursor(*id, *band), None),
            _ => {}
        }

        // A selected set's `+` resolves ahead of the box handles, as the press
        // does: the plain arrow (no control shows a pointing hand, §9.2).
        if let Some(p) = resp.hover_pos()
            && self.set_plus_at(p, rect, ppp).is_some()
        {
            return (egui::CursorIcon::Default, None);
        }

        // A line's ends, then the rail handle, in the press's order.
        if let Some(p) = resp.hover_pos()
            && self.grab_line_end(p, rect, ppp).is_some()
        {
            return (egui::CursorIcon::Grab, None);
        }
        if let Some(p) = resp.hover_pos()
            && self.grab_rail_handle(p, rect, ppp).is_some()
        {
            return (egui::CursorIcon::Grab, None);
        }

        // A spacing band, in the press's order.
        if let Some(p) = resp.hover_pos()
            && let Some((s, i)) = self.spacing_at(p, rect, ppp)
        {
            return (self.spacing_cursor(s.id, s.bands[i].kind), None);
        }

        let (Some(p), Some(h)) = (resp.hover_pos(), handles) else {
            return NONE;
        };
        let skewing = resp.ctx.input(|i| i.modifiers.ctrl || i.modifiers.command);
        match h.armed(p, skewing) {
            Some(Armed::Resize(handle)) => (resize_icon(h.screen_direction(handle)), None),
            Some(Armed::Skew(handle)) => (resize_icon(h.screen_tangent(handle)), None),
            Some(Armed::Rotate(handle)) => (
                egui::CursorIcon::Grab,
                Some(Bitmap::Rotate(Turn::toward(h.screen_direction(handle)))),
            ),
            None => NONE,
        }
    }

    /// The arrow for a corner of the point box, or `None` anywhere else in the
    /// node tool. Measured on screen from the box's centre.
    fn point_box_cursor(
        &self,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<(egui::CursorIcon, Option<Bitmap>)> {
        let id = self.edited_path()?;
        let turning = |pivot: Point, at: Option<egui::Pos2>| {
            let facing = at
                .map(|p| Turn::toward(p - self.to_screen(pivot, rect, ppp)))
                .unwrap_or_default();
            (egui::CursorIcon::Grabbing, Some(Bitmap::Rotate(facing)))
        };
        if let Drag::PointRotate { pivot, .. } = self.drag {
            let at = resp.hover_pos().or_else(|| resp.interact_pointer_pos());
            return Some(turning(pivot, at));
        }
        let (handle, box_) = match self.drag {
            Drag::PointBox { handle, box_, .. } => (handle, box_),
            Drag::None => {
                let p = resp.hover_pos()?;
                let box_ = self.point_box(id)?;
                match self.node_grab(self.to_world(p, rect, ppp))? {
                    NodeGrab::Box(handle) => (handle, box_),
                    // Armed but not yet turning: the open hand.
                    NodeGrab::BoxRotate => {
                        let (_, bitmap) = turning(box_.center(), Some(p));
                        return Some((egui::CursorIcon::Grab, bitmap));
                    }
                    _ => return None,
                }
            }
            _ => return None,
        };
        let out = self.to_screen(box_handle_at(box_, handle), rect, ppp)
            - self.to_screen(box_.center(), rect, ppp);
        Some((resize_icon(out), None))
    }

    // --- normal mode ------------------------------------------------------

    fn normal_mode_input(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) {
        // The button is still down after a cancel; sit the press out, or a short
        // cancelled drag comes back as a `clicked()`.
        if self.gesture_cancelled {
            return;
        }

        // The auto-scroll total resets here, above the tool routing, since the
        // pen, node and image-edit inputs return before `finish_drag`. Safe
        // against a cancel because `cancel_gesture` sets `gesture_cancelled` first.
        if resp.drag_started() || resp.drag_stopped() {
            self.autopan_total = Vec2::ZERO;
        }

        let handles = self.selection_handles(rect, ppp);
        let (shift, alt, space) = ui.input(|i| {
            (
                i.modifiers.shift,
                i.modifiers.alt,
                i.key_down(egui::Key::Space),
            )
        });
        // Space arms a pan from any tool, so it outranks the tool's own gesture.
        let panning_armed =
            space || self.tool == Tool::Hand || resp.dragged_by(egui::PointerButton::Middle);

        // An armed eyedropper owns every press an armed pan does not, chrome included.
        if self.dropper_armed().is_some() && !panning_armed && !matches!(self.drag, Drag::Pan) {
            self.dropper_input(ui, resp, rect, ppp);
            return;
        }

        // Rulers and guides, ahead of the tools but behind an armed pan and any
        // selection chrome the guide lies across ([`Self::chrome_claims`]).
        let on_handle = resp
            .hover_pos()
            .or(resp.interact_pointer_pos())
            .is_some_and(|p| self.chrome_claims(p, rect, ppp, &handles));
        if !panning_armed
            && !matches!(self.drag, Drag::Pan)
            && self.guide_input(resp, rect, ppp, on_handle)
        {
            return;
        }

        // A path in flight owns the pointer whichever tool armed it, so this
        // sits ahead of the node tool's arm.
        if (self.tool == Tool::Pen || self.pen.is_some())
            && !panning_armed
            && !matches!(self.drag, Drag::Pan)
        {
            self.pen_input(ui, resp, rect, ppp);
            return;
        }

        if self.tool == Tool::Node && !panning_armed && !matches!(self.drag, Drag::Pan) {
            self.node_input(ui, resp, rect, ppp);
            return;
        }

        // Routed from here to keep this path's guards (pan, Space, focus).
        if self.tool == Tool::ImageEdit && !panning_armed && !matches!(self.drag, Drag::Pan) {
            self.image_edit_input(ui, resp, rect, ppp);
            return;
        }

        // A Space+click is a pan that did not travel.
        let clicked = resp.clicked() && !panning_armed;

        // Every creation tool's click, dispatched on the exhaustive `Tool::click_makes`.
        if clicked
            && let Some(answer) = self.tool.click_makes()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let world = self.to_world(p, rect, ppp);
            match answer {
                // Existing text, then an outline, then bare canvas — the cursor arm's order.
                ClickCreate::TextNode => match self.text_layer_at(world) {
                    Some(id) => self.begin_edit_text(Some(id), Some(world)),
                    None => match self.rail_at(world) {
                        Some(path) => self.begin_text_on_path(path, world),
                        None => self.begin_text(world),
                    },
                },
                ClickCreate::FittedImage => self.place_image_at(self.pixel_point(world)),
                ClickCreate::DefaultBox => self.click_create(world, alt),
            }
            return;
        }

        // A selected set's `+`, ahead of every pick: it sits over the set's edge.
        if self.tool.selects()
            && clicked
            && let Some(p) = resp.interact_pointer_pos()
            && self.set_plus_at(p, rect, ppp).is_some()
        {
            self.add_variant();
            return;
        }

        if self.tool.selects()
            && clicked
            && let Some(p) = resp.interact_pointer_pos()
        {
            let world = self.to_world(p, rect, ppp);
            let leaf = self.pick_at_pointer(ui, p, world, rect, ppp);
            // Counted by `ui::double_clicked`, not egui's.
            if crate::ui::double_clicked(ui, resp) {
                self.double_click_pick(leaf, world);
            } else {
                let picked = leaf.map(|id| self.pick_for_click(id, ui));
                match picked {
                    // Alt+Shift designates the key layer (§9.4); Ctrl+Shift stays
                    // deep-select plus extend.
                    Some(id) if alt && shift => self.designate_key(id),
                    Some(id) if shift => self.session.selection.toggle(id),
                    Some(id) => self.session.selection.set_one(id),
                    // Clicking nothing leaves the group you had stepped into.
                    None if !shift => {
                        self.entered_group = None;
                        self.session.selection.clear();
                    }
                    None => {}
                }
            }
        }

        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let world = self.to_world(p, rect, ppp);
            if panning_armed {
                self.drag = Drag::Pan;
            } else if self.tool.drags_a_box() {
                // The start corner takes the grid but no constraint: no extent yet.
                self.begin_create(self.pixel_point(world));
            } else if self.tool.selects() {
                self.begin_select_drag(ui, p, rect, ppp, &handles, shift);
            }
        }

        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let world = self.to_world(p, rect, ppp);
            self.update_drag(ui, resp, world, shift, alt);
        }

        if resp.drag_stopped() {
            let world = resp
                .interact_pointer_pos()
                .map(|p| self.to_world(p, rect, ppp));
            self.finish_drag(world, shift, alt);
        }
    }

    // --- what a click on a grouped layer actually selects --------------------

    /// Turn the leaf the hit test found into the node a click should select.
    ///
    /// A click on any part of a group selects the whole group. Ctrl reaches the
    /// leaf; Ctrl+Alt picks the outermost group's immediate child. Inside
    /// `entered_group` ([`Self::double_click_pick`]) the chain stops at that group.
    pub(crate) fn pick_for_click(&mut self, leaf: NodeId, ui: &egui::Ui) -> NodeId {
        // A click outside the entered group ends the isolation before the chain
        // is built.
        if let Some(g) = self.entered_group
            && !ondin_core::is_within(&self.session.doc, leaf, g)
        {
            self.entered_group = None;
        }
        let picked = self.pick_preview(leaf, ui);

        // Ctrl-reaching into a group opens the isolation a double-click does; after
        // `pick_preview`, which must see the old scope.
        if ui.input(|i| i.modifiers.ctrl || i.modifiers.command)
            && let Some(outermost) =
                ondin_core::group_chain(&self.session.doc, leaf, self.entered_group).first()
        {
            self.entered_group = Some(*outermost);
        }
        picked
    }

    /// [`Self::pick_for_click`] without the side effect, for the hover outline.
    fn pick_preview(&self, leaf: NodeId, ui: &egui::Ui) -> NodeId {
        let (ctrl, alt) = ui.input(|i| (i.modifiers.ctrl || i.modifiers.command, i.modifiers.alt));
        let chain = ondin_core::group_chain(&self.session.doc, leaf, self.entered_group);
        pick_from_chain(ctrl, alt, &chain, leaf)
    }

    /// A double-click goes one level in: steps into a group, opens a text node,
    /// crops a picture, or opens a path for point editing.
    ///
    /// One level means one: descend while there is a group to enter
    /// (`entered_group` remembers how far), then open the content.
    fn double_click_pick(&mut self, leaf: Option<NodeId>, world: Point) {
        let Some(leaf) = leaf else {
            self.entered_group = None;
            self.session.selection.clear();
            return;
        };
        let chain = ondin_core::group_chain(&self.session.doc, leaf, self.entered_group);
        if let [outermost, rest @ ..] = chain.as_slice() {
            self.entered_group = Some(*outermost);
            self.session
                .selection
                .set_one(rest.first().copied().unwrap_or(leaf));
            return;
        }
        // An instance double-clicked on its own background steps in, as `Enter`
        // does; its group chain starts above it.
        if ondin_core::component::instance_root(&self.session.doc, leaf) == Some(leaf)
            && self.entered_group != Some(leaf)
            && self
                .session
                .doc
                .get(leaf)
                .is_some_and(|n| !n.children().is_empty())
        {
            self.enter_container(leaf, Some(world));
            return;
        }
        if matches!(
            self.session.doc.get(leaf).map(|n| n.kind()),
            Some(NodeKind::Text { .. })
        ) {
            self.begin_edit_text(Some(leaf), Some(world));
            return;
        }
        // A picture's only door to image editing; behind text (an image-filled
        // text node is type first).
        if self
            .session
            .doc
            .get(leaf)
            .and_then(|n| tools::croppable_fill(n.paint()))
            .is_some()
        {
            self.begin_image_edit(leaf);
            return;
        }
        if self.is_path(leaf) {
            self.session.selection.set_one(leaf);
            self.points.clear();
            self.choose_tool(Tool::Node);
            self.session.info(
                "Editing points: drag an anchor, a handle or a segment; double-click a segment to \
                 add a point; Delete to remove",
            );
            return;
        }
        self.session.selection.set_one(leaf);
    }

    /// `Alt+Shift`+click: make `id` the key — the layer align aligns *to*, and the
    /// base operand of a boolean.
    ///
    /// Inside a `Boolean`, operand order is child order (`boolean::evaluate`), so
    /// it is an undoable reorder to index 0; elsewhere a designation on the selection.
    pub(crate) fn designate_key(&mut self, id: NodeId) {
        let parent = self.session.doc.get(id).and_then(|n| n.parent());
        if let Some(parent) = parent
            && let Some(op) = match self.session.doc.get(parent).map(|n| n.kind()) {
                Some(NodeKind::Boolean { op }) => Some(*op),
                _ => None,
            }
        {
            let tx = Transaction(vec![Operation::Reorder { id, index: 0 }]);
            if self.session.commit(tx) {
                // Only `Subtract` reads the order; say so for the others.
                self.session.info(match op {
                    ondin_core::BoolOp::Subtract => "Base operand",
                    _ => "Moved to the bottom — this operation ignores operand order",
                });
            }
            return;
        }
        // An unselected layer is added first, so the key has a selection to be in.
        if !self.session.selection.contains(id) {
            self.session.selection.add(id);
        }
        self.session.selection.set_key(id);
    }

    /// Whether the selection chrome would claim a press at `screen` — the rungs
    /// [`Self::begin_select_drag`] walks, as a yes/no for `rulers::guide_input`.
    /// A parallel ladder; `a_guide_does_not_steal_a_press_the_chrome_claims`
    /// asserts the two agree.
    fn chrome_claims(
        &self,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
        handles: &Option<Handles>,
    ) -> bool {
        self.on_pivot(screen, rect, ppp).is_some()
            || self.set_plus_at(screen, rect, ppp).is_some()
            || (self.line_ends(rect, ppp).is_some()
                && self.grab_line_end(screen, rect, ppp).is_some())
            || self.grab_rail_handle(screen, rect, ppp).is_some()
            || handles
                .as_ref()
                .is_some_and(|h| h.armed(screen, false).is_some())
            || self.spacing_at(screen, rect, ppp).is_some()
    }

    fn begin_select_drag(
        &mut self,
        ui: &egui::Ui,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
        handles: &Option<Handles>,
        shift: bool,
    ) {
        let world = self.to_world(screen, rect, ppp);

        // What was grabbed is decided at the press origin, not past egui's drag
        // threshold, so it matches what the cursor promised.
        let grab = ui.input(|i| i.pointer.press_origin()).unwrap_or(screen);
        let grab_world = self.to_world(grab, rect, ppp);

        if let Some(id) = self.on_pivot(grab, rect, ppp) {
            self.drag = Drag::Pivot { id };
            return;
        }

        // A press that slides off a set's `+` starts nothing.
        if self.set_plus_at(grab, rect, ppp).is_some() {
            return;
        }

        if let Some((id, _, _)) = self.line_ends(rect, ppp)
            && let Some(which) = self.grab_line_end(grab, rect, ppp)
        {
            self.drag = Drag::LineEnd { id, which };
            return;
        }

        if let Some(id) = self.grab_rail_handle(grab, rect, ppp) {
            self.drag = Drag::RailOffset { id };
            return;
        }

        if let Some(h) = handles {
            let skewing = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
            match h.armed(grab, skewing) {
                Some(Armed::Resize(handle)) => {
                    self.drag = Drag::Resize {
                        target: h.target,
                        handle,
                    };
                    return;
                }
                Some(Armed::Skew(handle)) => {
                    self.drag = Drag::Skew {
                        target: h.target,
                        handle,
                        press: world,
                        lean: Affine::IDENTITY,
                    };
                    return;
                }
                Some(Armed::Rotate(_)) => {
                    let v = world - h.pivot;
                    self.drag = Drag::Rotate {
                        target: h.target,
                        pivot: h.pivot,
                        start: v.y.atan2(v.x),
                        turn: Affine::IDENTITY,
                    };
                    return;
                }
                None => {}
            }
        }

        // A selected container's gap or padding band: above the Alt+Shift refusal,
        // since that chord is the band's all-sides drag.
        if self.begin_spacing_drag(grab, rect, ppp) {
            return;
        }

        // Alt+Shift on the artwork starts no drag, so a key-layer designation that
        // slips past the drag threshold cannot leave an Alt-duplicate behind.
        if shift && ui.input(|i| i.modifiers.alt) {
            return;
        }

        // A click's `pick_at_pointer` chain, plus a selected frame's whole box.
        let grabbed = self
            .frame_label_at(ui, grab, rect, ppp)
            .or_else(|| self.pick_leaf(grab_world))
            .or_else(|| self.selected_frame_at(grab_world))
            .or_else(|| self.frame_edge_at(grab_world));
        match grabbed {
            Some(leaf) => {
                let top = self.pick_for_click(leaf, ui);
                if !self.session.selection.contains(top) {
                    if shift {
                        self.session.selection.add(top);
                    } else {
                        self.session.selection.set_one(top);
                    }
                }
                // A locked selection is not dragged; the press still selected.
                if self.selection_locked() {
                    return;
                }
                self.drag = Drag::Move { anchor: world };
            }
            None => {
                self.entered_group = None;
                self.drag = Drag::Marquee {
                    // The press, not where the drag was recognised.
                    origin: grab_world,
                    additive: shift,
                };
            }
        }
    }

    fn update_drag(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        world: Point,
        shift: bool,
        alt: bool,
    ) {
        match &self.drag {
            Drag::Pan => self.pan_by_drag(ui, resp),
            Drag::Move { anchor } => {
                let (delta, _) = self.snapped_move(world - *anchor, shift);
                // Alt turns the move into a copy at any point in the drag.
                self.sync_alt_clone(alt);
                match &mut self.alt_clone {
                    Some(clone) => {
                        clone.delta = delta;
                        let tx = self.clone_tx(delta);
                        let leaving = !self.leaving_their_frames(delta).is_empty();
                        self.session.set_preview_escaping_ghosts(&tx, leaving);
                    }
                    None => {
                        if let Some(tx) = self.move_preview_tx(delta) {
                            let leaving = self.leaving_their_frames(delta);
                            self.session.set_preview_escaping(&tx, &leaving);
                        }
                    }
                }
            }
            Drag::Resize { target, handle } => {
                let (at, _) = self.snapped_resize(*target, *handle, world);
                let tx =
                    self.resize_tx(*target, *handle, at, self.resize_opts(*target, shift, alt));
                self.session.set_preview(&tx);
                // The Transform card's Scale field reads out the drag; its valve
                // fires only on the field's own engagement.
                if self.tool == Tool::Scale {
                    self.scale_pct = self.live_scale_pct().unwrap_or(100.0);
                }
            }
            Drag::LineEnd { id, which } => {
                let tx = self.line_end_tx(*id, *which, world, shift);
                self.session.set_preview(&tx);
            }
            Drag::RailOffset { id } => {
                let tx = self.rail_offset_tx(*id, world);
                self.session.set_preview(&tx);
            }
            // Modifiers are read every frame, so they can change mid-drag.
            Drag::Spacing { .. } => {
                if let Some((tx, ..)) = self.band_drag_tx(world, shift, alt) {
                    self.session.set_preview(&tx);
                }
            }
            // `node_input` runs first and owns the node tool's drags end to end.
            Drag::Points { .. }
            | Drag::Handle { .. }
            | Drag::Segment { .. }
            | Drag::PointBox { .. }
            | Drag::PointRotate { .. } => {}
            // Unsnapped to guides, since the pointer stands for an angle.
            Drag::Skew {
                target,
                handle,
                press,
                ..
            } => {
                let (target, handle, press) = (*target, *handle, *press);
                let (tx, lean) = self.skew_tx(target, handle, press, world, shift);
                self.session.set_preview(&tx);
                // Stored from the transaction's own computation so the drawn frame agrees.
                if let Drag::Skew { lean: slot, .. } = &mut self.drag {
                    *slot = lean;
                }
            }
            Drag::Rotate {
                target,
                pivot,
                start,
                ..
            } => {
                let (target, pivot, start) = (*target, *pivot, *start);
                let (tx, turn) = self.rotate_tx(target, pivot, start, world, shift);
                self.session.set_preview(&tx);
                if let Drag::Rotate { turn: slot, .. } = &mut self.drag {
                    *slot = turn;
                }
            }
            Drag::Create { .. } => {
                // The mid-draw keys act before the box is rebuilt from the gesture.
                self.reshape_create(ui, world);
                let Drag::Create { start, shape, .. } = &self.drag else {
                    return;
                };
                let (from, to) = self.create_box(*shape, *start, world, shift, alt);
                if let Some(tx) = self.create_tx(from, to) {
                    self.session.set_preview(&tx);
                }
            }
            // Guide and crop drags are owned by `guide_input`/`image_edit_input`;
            // a pivot or marquee drag has nothing to preview through the renderer.
            Drag::CropPan { .. }
            | Drag::CropResize { .. }
            | Drag::Pivot { .. }
            | Drag::Guide(_)
            | Drag::Marquee { .. }
            | Drag::TextSelect
            | Drag::None => {}
        }
        ui.ctx().request_repaint();
    }

    fn finish_drag(&mut self, world: Option<Point>, shift: bool, alt: bool) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        // Only a cancel unwinds the auto-scroll.
        self.autopan_total = Vec2::ZERO;
        let Some(world) = world else {
            self.alt_clone = None;
            self.session.clear_preview();
            return;
        };
        match drag {
            Drag::Move { anchor } => {
                let raw = world - anchor;
                if raw.hypot() > 1e-4 {
                    let (delta, _) = self.snapped_move(raw, shift);
                    self.sync_alt_clone(alt);
                    // Built with the drag back in place: whether an in-flow item
                    // leaves its layout is read off the drag's pointer (`move_pointer`).
                    self.drag = Drag::Move { anchor };
                    let built = match &self.alt_clone {
                        Some(_) => self.clone_tx(delta),
                        None => self.move_tx(delta),
                    };
                    self.drag = Drag::None;
                    match &self.alt_clone {
                        Some(clone) => {
                            let roots: Vec<NodeId> = clone.copies.iter().map(|c| c.root).collect();
                            let tx = built;
                            if self.session.commit(tx) {
                                // The copy is what stays selected.
                                self.session.selection.set(roots);
                            }
                        }
                        None => {
                            // Asked before the commit, so a refused drop says why.
                            let refused_set = self.drops_into_a_set_of_another_kind(delta);
                            if self.session.commit(built)
                                && let Some(why) = refused_set
                            {
                                self.session.info(why);
                            }
                        }
                    }
                }
                self.alt_clone = None;
            }
            Drag::Resize { target, handle } => {
                let (at, _) = self.snapped_resize(target, handle, world);
                let tx = self.resize_tx(target, handle, at, self.resize_opts(target, shift, alt));
                self.session.commit(tx);
            }
            Drag::LineEnd { id, which } => {
                let tx = self.line_end_tx(id, which, world, shift);
                self.session.commit(tx);
            }
            Drag::RailOffset { id } => {
                let tx = self.rail_offset_tx(id, world);
                self.session.commit(tx);
            }
            // A release where every slot already draws the value commits nothing,
            // compared against the drawn value so a bound slot is right.
            drag @ Drag::Spacing { id, .. } => {
                self.drag = drag;
                let built = self.band_drag_tx(world, shift, alt);
                self.drag = Drag::None;
                let drawn = self.session.doc.resolved_display(id).map(|d| d.spacing());
                match built {
                    Some((tx, value, slots))
                        if !drawn.is_some_and(|s| slots.iter().all(|i| s[*i] == value)) =>
                    {
                        let cut = self.band_drag_cut(&tx);
                        if self.session.commit(tx) && !cut.is_empty() {
                            let (styles, mut vars) = self.session.last_detached().clone();
                            vars.extend(cut);
                            vars.sort();
                            vars.dedup();
                            if let Some(said) = crate::session::said_detached(styles, vars) {
                                self.session.info(said);
                            }
                        }
                    }
                    _ => self.session.clear_preview(),
                }
            }
            // `node_input` commits its own release.
            Drag::Points { .. }
            | Drag::Handle { .. }
            | Drag::Segment { .. }
            | Drag::PointBox { .. }
            | Drag::PointRotate { .. } => {}
            Drag::Skew {
                target,
                handle,
                press,
                ..
            } => {
                let (tx, lean) = self.skew_tx(target, handle, press, world, shift);
                if self.session.commit(tx) && matches!(target, Target::Selection) {
                    // A set has no stored frame; keep its lean on the selection.
                    let was = self.session.selection.frame();
                    self.session.selection.set_frame(lean * was);
                }
            }
            Drag::Rotate {
                target,
                pivot,
                start,
                ..
            } => {
                let (tx, turn) = self.rotate_tx(target, pivot, start, world, shift);
                if self.session.commit(tx) && matches!(target, Target::Selection) {
                    let was = self.session.selection.frame();
                    self.session.selection.set_frame(turn * was);
                }
            }
            Drag::Create {
                id, start, shape, ..
            } => {
                let screen_len = (world - start).hypot() * self.points_per_world();
                if screen_len > MIN_CREATE_PX {
                    // `create_tx` reads the still-live gesture, so restore it.
                    self.drag = drag_create_again(&drag, id, start);
                    let (from, to) = self.create_box(shape, start, world, shift, alt);
                    self.commit_create(id, shape, from, to);
                } else if shape == Tool::Text {
                    // An out-and-back drag is a click, and for text that still plants
                    // a caret (a shape abandons it); egui's drag threshold exceeds
                    // `MIN_CREATE_PX`, so only a return to the origin gets here.
                    self.drag = Drag::None;
                    self.begin_text(start);
                } else if shape == Tool::Image {
                    self.drag = Drag::None;
                    self.place_image_at(start);
                }
            }
            Drag::Marquee { origin, additive } => self.apply_marquee(origin, world, additive),
            // `pivot_at` answers `None` back at the centre, which clears the field.
            Drag::Pivot { id } => {
                let pivot = self.pivot_at(id, world);
                if self.session.doc.get(id).is_some_and(|n| n.pivot() != pivot) {
                    self.session
                        .commit(Transaction(vec![Operation::SetPivot { id, pivot }]));
                }
            }
            // Guide and crop releases are finished by their own input paths.
            Drag::CropPan { .. }
            | Drag::CropResize { .. }
            | Drag::Guide(_)
            | Drag::Pan
            | Drag::TextSelect
            | Drag::None => {}
        }
        self.session.clear_preview();
    }

    /// Prepare the copy an Alt-drag will carry, or throw it away when Alt is
    /// released. The capture runs on the edge; ids are minted once, so every
    /// [`Self::clone_tx`] — each preview frame and the commit — is the same copy (§6.2).
    fn sync_alt_clone(&mut self, alt: bool) {
        if !alt {
            self.alt_clone = None;
            return;
        }
        if self.alt_clone.is_some() {
            return;
        }
        let doc = &self.session.doc;
        let mut copies = Vec::new();
        for source in build::outermost(doc, self.session.selection.ids()) {
            let Some(template) = doc.capture_subtree(source) else {
                continue;
            };
            let Some(parent) = doc.get(source).and_then(|n| n.parent()) else {
                continue;
            };
            let Some((nodes, root)) = ondin_core::remap_subtree(&template, &mut self.session.ids)
            else {
                continue;
            };
            copies.push(ClonedSubtree {
                source,
                parent,
                nodes,
                root,
            });
        }
        self.alt_clone = (!copies.is_empty()).then_some(AltClone {
            copies,
            delta: Vec2::ZERO,
        });
    }

    /// The transaction that drops the prepared copy `delta` from its original.
    /// The copy lands in the frame the move's rule picks ([`Self::move_destination`]),
    /// just above its original among its siblings, appended otherwise; the ghost
    /// previews in the same slot (`render::Ghost::index`).
    fn clone_tx(&self, delta: Vec2) -> Transaction {
        let Some(clone) = &self.alt_clone else {
            return Transaction::default();
        };
        let res = &self.session.resolved;
        let moved = Affine::translate(delta);
        // The indices already inserted at, per parent — not a count: the selection
        // is in pick order, so an earlier insert may land above this one.
        let mut done: Vec<(NodeId, Vec<usize>)> = Vec::new();
        let mut ops = Vec::new();
        // Copies in one transaction cannot see each other's names yet.
        let mut names = build::CopyNames::default();

        for copy in &clone.copies {
            let Some(old_world) = res.world_transform(copy.source) else {
                continue;
            };
            let parent = self
                .move_destination(copy.source, delta)
                .unwrap_or(copy.parent);
            // Just above the original among its siblings; appended in another frame.
            let above = (parent == copy.parent)
                .then(|| {
                    self.session
                        .doc
                        .get(parent)?
                        .children()
                        .iter()
                        .position(|c| *c == copy.source)
                        .map(|i| i + 1)
                })
                .flatten();
            let base_index = above.unwrap_or_else(|| self.child_count(parent));
            let at = match done.iter_mut().find(|(p, _)| *p == parent) {
                Some((_, seen)) => seen,
                None => {
                    done.push((parent, Vec::new()));
                    &mut done.last_mut().expect("just pushed").1
                }
            };
            let shift = at.iter().filter(|i| **i <= base_index).count();
            at.push(base_index);
            ops.push(Operation::InsertSubtree {
                nodes: copy.nodes.clone(),
                parent,
                index: base_index + shift,
            });
            // Through the destination's world transform, which differs once the
            // copy crosses into a frame.
            let to_world = res.world_transform(parent).unwrap_or(Affine::IDENTITY);
            ops.push(Operation::SetTransform {
                id: copy.root,
                transform: to_world.inverse() * moved * old_world,
            });
            let was_called = copy
                .nodes
                .iter()
                .find(|n| n.id() == copy.root)
                .map(|n| n.name().to_string())
                .unwrap_or_default();
            ops.extend(names.rename(&self.session.doc, parent, copy.root, &was_called));
        }
        Transaction(ops)
    }

    /// Where `id` would land if the move committed now: `Some(parent)` when the
    /// move changes its parent, `None` when it stays where it is.
    ///
    /// An in-flow item stays while the pointer is inside its container (or, if
    /// pressed on an overflowing part, while its box still meets it) unless a
    /// frame inside the container covers most of its box; any other layer goes to
    /// the frame covering most of its moved box. Only children of the root or a
    /// frame hop, and a layer in a frame inside a group only between that group's
    /// frames. Shared with [`Self::draw_frame_drop_outline`].
    fn move_destination(&self, id: NodeId, delta: Vec2) -> Option<NodeId> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let root = doc.root();
        let node = doc.get(id)?;
        let parent = node.parent();

        // A layer inside a group stays in it; children of the root or a frame,
        // frames included, may hop.
        let can_hop = parent.is_some_and(|p| {
            p == root
                || matches!(
                    doc.get(p).map(|n| n.kind()),
                    Some(NodeKind::Artboard { .. })
                )
        });
        if !can_hop {
            return None;
        }
        let landed = res.world_bounds(id)? + delta;
        // An in-flow item leaves its layout only when the pointer does: its box
        // may overflow the frame, while the pointer is shared by a block's items.
        if build::is_flex_item(doc, id)
            && let Some(p) = parent
        {
            // A frame inside the item's own container takes it by the half-area
            // rule first, since the pointer over it is always "in the container".
            if let Some(nested) = self
                .frame_covering_except(landed, id)
                .filter(|d| *d != p)
                .filter(|d| ondin_core::is_within(doc, *d, p))
            {
                return Some(nested);
            }
            if self.pointer_in(p, self.move_pointer(id, delta)?) {
                return None;
            }
            let pressed_in = self.pointer_in(p, self.move_pointer(id, Vec2::ZERO)?);
            if !pressed_in && self.box_meets(p, landed) {
                return None;
            }
        }
        if let Some(fence) = group_fence(doc, id) {
            let moving = self.moving_with(id);
            let destination = self.with_frames(|i| {
                // Never the moved layer, one moving with it, or a frame inside either.
                let inside: Vec<(NodeId, KRect)> = i
                    .boxes
                    .iter()
                    .filter(|(f, _)| ondin_core::is_within(doc, *f, fence))
                    .filter(|(f, _)| !within_moving(doc, *f, &moving))
                    .copied()
                    .collect();
                self.covering_that_holds(landed, id, inside, i.components)
            })?;
            return (Some(destination) != parent).then_some(destination);
        }
        // A frame cannot land in itself or its descendants (`Reparent` refuses the cycle).
        let destination = self.frame_covering_except(landed, id).unwrap_or(root);
        (Some(destination) != parent).then_some(destination)
    }

    /// Where the pointer is in world space for a move of `id` by `delta`: the
    /// drag's anchor carried by the delta, else (no drag, as a test asks) the
    /// middle of `id`'s box carried the same way.
    fn move_pointer(&self, id: NodeId, delta: Vec2) -> Option<Point> {
        match self.drag {
            Drag::Move { anchor } => Some(anchor + delta),
            _ => Some(self.session.resolved.world_bounds(id)?.center() + delta),
        }
    }

    /// Whether world point `at` is inside container `id`'s own box through its
    /// world transform, so a rotated container answers for its turned box.
    fn pointer_in(&self, id: NodeId, at: Point) -> bool {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let (Some(local), Some(world)) =
            (ondin_core::local_box(doc, res, id), res.world_transform(id))
        else {
            return false;
        };
        local.contains(world.inverse() * at)
    }

    /// Whether the world rectangle `r` meets container `id`'s own box through its
    /// world transform — [`Self::pointer_in`]'s twin for a box.
    ///
    /// Separating axes over the rectangle's two and the quad's two edge
    /// directions; open intervals, so a box that only touches an edge misses.
    fn box_meets(&self, id: NodeId, r: KRect) -> bool {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let (Some(local), Some(world)) =
            (ondin_core::local_box(doc, res, id), res.world_transform(id))
        else {
            return false;
        };
        let corners = |b: KRect| {
            [
                Point::new(b.x0, b.y0),
                Point::new(b.x1, b.y0),
                Point::new(b.x1, b.y1),
                Point::new(b.x0, b.y1),
            ]
        };
        let quad = corners(local).map(|p| world * p);
        let rect = corners(r);
        let axes = [
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
            quad[1] - quad[0],
            quad[3] - quad[0],
        ];
        axes.iter().all(|axis| {
            if axis.hypot2() == 0.0 {
                return true;
            }
            let span = |pts: &[Point; 4]| {
                pts.iter()
                    .map(|p| p.to_vec2().dot(*axis))
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v), hi.max(v))
                    })
            };
            let ((a0, a1), (b0, b1)) = (span(&quad), span(&rect));
            a0 < b1 && b0 < a1
        })
    }

    /// Which of the moving layers this `delta` would take out of their frame —
    /// the ones the preview draws unclipped. Asked through
    /// [`Self::move_destination`], the release's own question.
    fn leaving_their_frames(&self, delta: Vec2) -> Vec<NodeId> {
        build::outermost(&self.session.doc, self.session.selection.ids())
            .into_iter()
            .filter(|id| self.move_destination(*id, delta).is_some())
            .collect()
    }

    /// Whether moving the selection by `delta` is a **reorder in a flex
    /// container** rather than a move — and if so, which item and the `Reorder` it
    /// asks for (`None` inside: it lands where it was).
    ///
    /// One selected in-flow layer the move does not take out of its parent. In a
    /// grid the drop writes lines instead (`build::layout_drop` answers for both).
    /// Several layers are [`Self::flex_block_reorder_of`]'s.
    fn flex_reorder_of(&self, delta: Vec2) -> Option<(NodeId, Option<Operation>)> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let [id] = build::outermost(doc, self.session.selection.ids())[..] else {
            return None;
        };
        if !build::is_flex_item(doc, id) || self.move_destination(id, delta).is_some() {
            return None;
        }
        Some((id, build::layout_drop(doc, res, id, delta)))
    }

    /// [`Self::flex_reorder_of`] for **several** layers: when every one is an
    /// in-flow item of the same container and none leaves it, the move drops the
    /// block (`build::layout_drop_many`'s ops, empty when it lands where it was).
    fn flex_block_reorder_of(&self, delta: Vec2) -> Option<Vec<Operation>> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let ids = build::outermost(doc, self.session.selection.ids());
        if ids.len() < 2
            || ids
                .iter()
                .any(|id| self.move_destination(*id, delta).is_some())
        {
            return None;
        }
        build::layout_drop_many(doc, res, &ids, delta)
    }

    /// The transaction a move of the selection by `delta` **previews** — what
    /// [`Self::move_tx`] commits, drawn the way a drag has to be drawn.
    ///
    /// `build::move_by_world`'s translation for every selected layer, so the
    /// dragged layer stays under the hand; one in-flow item, or several of one
    /// container, also gain the reorder or grid lines the release commits, and any
    /// other selection drops the translation of every item that keeps its slot
    /// ([`Self::stays_in_flow`]).
    fn move_preview_tx(&self, delta: Vec2) -> Option<Transaction> {
        let mut tx = build::move_by_world(
            &self.session.doc,
            &self.session.resolved,
            self.session.selection.ids(),
            delta,
        )
        .ok()?;
        match self.flex_reorder_of(delta) {
            Some((_, Some(reorder))) => tx.0.push(reorder),
            Some((_, None)) => {}
            None => match self.flex_block_reorder_of(delta) {
                Some(reorders) => tx.0.extend(reorders),
                None => tx.0.retain(|op| match op {
                    Operation::SetTransform { id, .. } => !self.stays_in_flow(*id, delta),
                    _ => true,
                }),
            },
        }
        Some(tx)
    }

    /// Whether `id`, moved by `delta` as one of several, **keeps its slot in its
    /// container's flow** — an in-flow flex item the move does not take out of its
    /// parent. Commit and preview both ask it.
    fn stays_in_flow(&self, id: NodeId, delta: Vec2) -> bool {
        build::is_flex_item(&self.session.doc, id) && self.move_destination(id, delta).is_none()
    }

    /// The transaction that finishes a move: the translation, plus any change
    /// of frame the move implies ([`Self::move_destination`]), as one undo step.
    /// Built by hand because a reparent projects the new transform through the
    /// destination's world transform, not the current parent's.
    fn move_tx(&self, delta: Vec2) -> Transaction {
        // An in-flow item commits only its reorder (or grid lines): a stored
        // translation would be invisible until the layout is removed.
        if let Some((_, reorder)) = self.flex_reorder_of(delta) {
            return Transaction(reorder.into_iter().collect());
        }
        if let Some(reorders) = self.flex_block_reorder_of(delta) {
            return Transaction(reorders);
        }
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let moved = Affine::translate(delta);
        // Net change to each parent's child count: a `Reparent` index is validated
        // against the list as it stands when the op runs.
        let mut net: Vec<(NodeId, isize)> = Vec::new();
        let mut ops = Vec::new();

        for id in build::outermost(doc, self.session.selection.ids()) {
            let Some(node) = doc.get(id) else { continue };
            let Some(old_world) = res.world_transform(id) else {
                continue;
            };
            let new_world = moved * old_world;
            let parent = node.parent();

            match self.move_destination(id, delta) {
                Some(to) => {
                    let mut bump =
                        |p: NodeId, by: isize| match net.iter_mut().find(|(q, _)| *q == p) {
                            Some((_, n)) => {
                                let before = *n;
                                *n += by;
                                before
                            }
                            None => {
                                net.push((p, by));
                                0
                            }
                        };
                    let shift = bump(to, 1);
                    if let Some(from) = parent {
                        bump(from, -1);
                    }
                    let index = (self.child_count(to) as isize + shift).max(0) as usize;
                    let to_world = res.world_transform(to).unwrap_or(Affine::IDENTITY);
                    ops.push(Operation::Reparent {
                        id,
                        new_parent: to,
                        index,
                    });
                    ops.push(Operation::SetTransform {
                        id,
                        transform: to_world.inverse() * new_world,
                    });
                }
                None if self.stays_in_flow(id, delta) => {}
                None => ops.push(Operation::SetTransform {
                    id,
                    transform: build::local_for_world(doc, res, id, new_world),
                }),
            }
        }
        Transaction(ops)
    }

    /// The frame that owns `bounds` for a move of `moved`, out of the document's
    /// artboards but those moving with it ([`Self::moving_with`]) and anything
    /// inside them. Excluded before the topmost is chosen: a moved frame lies
    /// over its own moved box, and the frame under it must still be a candidate.
    fn frame_covering_except(&self, bounds: KRect, moved: NodeId) -> Option<NodeId> {
        let doc = &self.session.doc;
        let moving = self.moving_with(moved);
        self.with_frames(|i| {
            let others: Vec<(NodeId, KRect)> = i
                .boxes
                .iter()
                .filter(|(f, _)| !within_moving(doc, *f, &moving))
                .copied()
                .collect();
            self.covering_that_holds(bounds, moved, others, i.components)
        })
    }

    /// [`frame_covering`] over `candidates`, skipping a frame the component rules
    /// (`Self::holds`) would refuse `moved` in, so the frame underneath is still a
    /// candidate. Skipped in a document with no components ([`FrameIndex`]'s `components`).
    ///
    /// [`FrameIndex`]: crate::app::FrameIndex
    fn covering_that_holds(
        &self,
        bounds: KRect,
        moved: NodeId,
        mut candidates: Vec<(NodeId, KRect)>,
        components: bool,
    ) -> Option<NodeId> {
        loop {
            let f = frame_covering(bounds, &candidates)?;
            if !components || self.holds(f, moved) {
                return Some(f);
            }
            candidates.retain(|(g, _)| *g != f);
        }
    }

    /// Whether frame `f` may hold `moved` dropped into it by this move:
    /// `component::can_hold`, and for a main entering a component set, the set
    /// taking every main moving with it (`variant::takes_together`).
    fn holds(&self, f: NodeId, moved: NodeId) -> bool {
        ondin_core::component::can_hold(&self.session.doc, f, moved)
            && self.set_takes_moving(f, moved)
    }

    /// [`Self::holds`]'s second half: `true` unless `moved` is a main entering
    /// the set `f` beside mains of another kind, or of a kind other than the set's.
    /// An Alt-drag asks `moved` alone ([`Self::moving_with`]): its copies are instances.
    fn set_takes_moving(&self, f: NodeId, moved: NodeId) -> bool {
        let doc = &self.session.doc;
        if !ondin_core::variant::is_set(doc, f)
            || !doc
                .get(moved)
                .is_some_and(|n| n.component() && n.parent() != Some(f))
        {
            return true;
        }
        let moving: Vec<NodeId> = self.moving_with(moved).into_iter().collect();
        ondin_core::variant::takes_together(doc, f, &moving)
    }

    /// Whether a move of the selection by `delta` drops a main over a component
    /// set of another kind, which [`Self::covering_that_holds`] passed over — the
    /// one refusal the release explains. Answers the message: [`SET_OF_ANOTHER_KIND`]
    /// for a main of a kind the set's variants are not, [`SET_OF_MIXED_KINDS`] for
    /// mains of two kinds moving into it together (an empty set fits either alone).
    fn drops_into_a_set_of_another_kind(&self, delta: Vec2) -> Option<&'static str> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        build::outermost(doc, self.session.selection.ids())
            .into_iter()
            .filter(|id| doc.get(*id).is_some_and(|n| n.component()))
            .find_map(|id| {
                let landed = res.world_bounds(id).map(|b| b + delta)?;
                let moving = self.moving_with(id);
                let mut candidates: Vec<(NodeId, KRect)> = self.with_frames(|i| {
                    i.boxes
                        .iter()
                        .filter(|(f, _)| !within_moving(doc, *f, &moving))
                        .copied()
                        .collect()
                });
                let kind = doc.get(id).expect("filtered on it").kind();
                // The frames `covering_that_holds` passed over, topmost first.
                while let Some(f) = frame_covering(landed, &candidates) {
                    if self.holds(f, id) {
                        return None;
                    }
                    if ondin_core::variant::is_set(doc, f) {
                        if !ondin_core::variant::fits_set(doc, f, kind, Some(id)) {
                            return Some(SET_OF_ANOTHER_KIND);
                        }
                        if !self.set_takes_moving(f, id) {
                            return Some(SET_OF_MIXED_KINDS);
                        }
                    }
                    candidates.retain(|(g, _)| *g != f);
                }
                None
            })
    }

    /// The layers that move with `id` in a move of it — none of them, nor anything
    /// inside them, can be the frame it lands in. `id` and the selection's
    /// outermost roots for a plain move; `id` alone for an Alt-drag. A hash set so
    /// [`within_moving`] needs one walk up per candidate frame.
    fn moving_with(&self, id: NodeId) -> std::collections::HashSet<NodeId> {
        let mut moving = std::collections::HashSet::from([id]);
        if self.alt_clone.is_none() {
            moving.extend(build::outermost(
                &self.session.doc,
                self.session.selection.ids(),
            ));
        }
        moving
    }

    /// The snapped pointer position for a resize, and the lines explaining it.
    /// Snapping the pointer rather than the box keeps the anchored edge still
    /// ([`snap::snap_point`]); the ratio constraint is applied after the snap.
    fn snapped_resize(
        &self,
        target: Target,
        handle: Handle,
        world: Point,
    ) -> (Point, Vec<snap::Guide>) {
        let moving: Vec<NodeId> = match target {
            Target::Layer(id) => vec![id],
            Target::Selection => self.session.selection.ids().to_vec(),
        };
        let axes = self.resize_axes(target, handle);
        let result = snap::snap_point(world, axes, &self.snapping(&moving));
        (result.at, result.guides)
    }

    /// Which world axes a resize handle travels along: [`Handle::outward`] mapped
    /// through the box's world transform, so a rotated side snaps on both axes.
    /// Axes it does not travel along are left alone, or the anchored edge would
    /// shift. A multi-selection uses [`Self::selection_frame`].
    fn resize_axes(&self, target: Target, handle: Handle) -> (bool, bool) {
        /// Well inside 1/√2, so a 45° edge claims both axes rather than neither.
        const AXIS: f64 = 0.3;
        let (ox, oy) = handle.outward();
        let world = match target {
            Target::Selection => self.selection_frame(),
            Target::Layer(id) => match self.session.preview_world_transform(id) {
                Some(w) => w,
                None => return (ox != 0.0, oy != 0.0),
            },
        };
        let out = world * Point::new(ox, oy) - world * Point::ZERO;
        if out.hypot() < 1e-9 {
            return (ox != 0.0, oy != 0.0);
        }
        let out = out.normalize();
        (out.x.abs() > AXIS, out.y.abs() > AXIS)
    }

    /// Whether a resize of `id` should hold its aspect ratio: Shift XOR the
    /// proportion lock, so Shift frees a locked layer. A multi-selection is locked
    /// only when every member is. The Scale tool counts as locked, since one
    /// stroke width has no answer for two factors ([`tools::Scaling`]).
    fn keep_ratio(&self, target: Target, shift: bool) -> bool {
        let doc = &self.session.doc;
        let locked = self.tool == Tool::Scale
            || match target {
                Target::Layer(id) => tools::proportions_locked(doc, &[id]),
                Target::Selection => tools::proportions_locked(doc, self.session.selection.ids()),
            };
        locked != shift
    }

    /// The terms of a handle drag: the ratio lock, Alt, and what the active tool
    /// says it means — the whole difference between Select and Scale.
    fn resize_opts(&self, target: Target, shift: bool, alt: bool) -> tools::Resize {
        tools::Resize {
            keep_ratio: self.keep_ratio(target, shift),
            symmetric: alt,
            scaling: self.tool.scaling(),
        }
    }

    /// How far a resize in flight has scaled the selection, as a percentage, for
    /// the Transform card's Scale field. The previewed box against the committed
    /// one, as the geometric mean of the two factors (as `Scaling::scalar`); an
    /// axis with no extent contributes 1.
    pub(crate) fn live_scale_pct(&self) -> Option<f64> {
        let (roots, before) = self.selection_union()?;
        let after = roots
            .iter()
            .filter_map(|id| self.session.preview_world_bounds(*id))
            .reduce(|a, b| a.union(b))?;
        let ratio = |a: f64, b: f64| if b.abs() < 1e-9 { 1.0 } else { a / b };
        let (sx, sy) = (
            ratio(after.width(), before.width()),
            ratio(after.height(), before.height()),
        );
        Some((sx * sy).abs().sqrt() * 100.0)
    }

    /// The resize transaction for whatever kind of node is being dragged: a
    /// geometry edit on a shape, a recursive scale on a container, each outermost
    /// member about the drawn box for a multi-selection. `opts.symmetric` is Alt
    /// (grow about the centre).
    fn resize_tx(
        &self,
        target: Target,
        handle: Handle,
        world: Point,
        opts: tools::Resize,
    ) -> Transaction {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let id = match target {
            Target::Layer(id) => id,
            Target::Selection => {
                // The committed tree, not the preview (scaling a box being scaled
                // compounds), and the box the chrome draws.
                let roots = build::outermost(doc, self.session.selection.ids());
                let Some(sel) = self.selection_resize_box() else {
                    return Transaction(Vec::new());
                };
                return tools::resize_selection(doc, res, &roots, sel, handle, world, opts);
            }
        };
        tools::resize_layer(doc, res, id, handle, world, opts)
    }

    /// Lean the dragged side, holding the one facing it still.
    ///
    /// A skew always goes into the transform: a shape or group leans on its own
    /// local box, a multi-selection about the upright union as one parallelogram.
    fn skew_tx(
        &self,
        target: Target,
        handle: Handle,
        press: Point,
        world: Point,
        snap_to_steps: bool,
    ) -> (Transaction, Affine) {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let drag = tools::SideDrag { press, at: world };
        match target {
            Target::Layer(id) => match ondin_core::local_box(doc, res, id) {
                Some(local) => (
                    tools::skew_to_handle(doc, res, id, local, handle, drag, snap_to_steps),
                    Affine::IDENTITY,
                ),
                None => (Transaction(Vec::new()), Affine::IDENTITY),
            },
            Target::Selection => {
                let roots = build::outermost(doc, self.session.selection.ids());
                let Some(union) = roots
                    .iter()
                    .filter_map(|id| res.world_bounds(*id))
                    .reduce(|a, b| a.union(b))
                else {
                    return (Transaction(Vec::new()), Affine::IDENTITY);
                };
                // One shear for the shapes and the frame drawn round them.
                let Some(shear) = tools::side_shear(union, handle, drag, snap_to_steps) else {
                    return (Transaction(Vec::new()), Affine::IDENTITY);
                };
                (tools::skew_selection(doc, res, &roots, shear), shear)
            }
        }
    }

    /// A multi-selection turns about the shared pivot the handles were built from
    /// (the union box's centre), so the set holds its arrangement.
    fn rotate_tx(
        &self,
        target: Target,
        pivot: Point,
        start: f64,
        world: Point,
        snap_to_steps: bool,
    ) -> (Transaction, Affine) {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let v = world - pivot;
        let mut angle = v.y.atan2(v.x) - start;
        if snap_to_steps {
            angle = (angle / ROTATE_SNAP).round() * ROTATE_SNAP;
        }
        match target {
            Target::Layer(id) => (
                tools::rotate_node(doc, res, id, pivot, angle),
                Affine::IDENTITY,
            ),
            Target::Selection => {
                let roots = build::outermost(doc, self.session.selection.ids());
                (
                    tools::rotate_selection(doc, res, &roots, pivot, angle),
                    tools::turn_about(pivot, angle),
                )
            }
        }
    }

    /// The snap sources the top bar's **Snap** menu currently admits, with
    /// `moving` excluded from the shape targets. A toggle that is off contributes
    /// an empty candidate list ([`snap::Snapping`]); no modifier switches snapping off.
    pub(crate) fn snapping(&self, moving: &[NodeId]) -> snap::Snapping {
        let zoom = self.points_per_world();
        let view = self.session.camera.viewport(self.canvas_px).view;
        snap::Snapping {
            targets: if self.snap_shapes {
                snap::targets(
                    &self.session.doc,
                    &self.session.resolved,
                    moving,
                    view,
                    SNAP_TARGET_LIMIT,
                )
            } else {
                Vec::new()
            },
            guides: if self.snap_guides {
                self.guide_lines()
            } else {
                snap::GuideLines::default()
            },
            baselines: if self.snap_baselines {
                self.baseline_lines(moving, view)
            } else {
                Vec::new()
            },
            pixel: self.snap_grid,
            zoom,
        }
    }

    /// Every text baseline in view, in world space, for [`snap::Snapping`].
    ///
    /// Read off the cached `TextLayout::baselines`, never re-shaped every frame of
    /// a drag. Rotated or skewed text is excluded: snapping is axis-aligned.
    fn baseline_lines(&self, moving: &[NodeId], view: KRect) -> Vec<f64> {
        let mut out = Vec::new();
        for id in ondin_core::nodes_in_view(&self.session.resolved, view) {
            // As `snap::targets`: a group must not snap to its own text.
            if moving.contains(&id) || crate::snap::is_inside_any(&self.session.doc, id, moving) {
                continue;
            }
            if self.session.doc.get(id).is_some() && !self.session.doc.shown(id) {
                continue;
            }
            let (Some(layout), Some(world)) = (
                self.session.resolved.text_layout(id),
                self.session.resolved.world_transform(id),
            ) else {
                continue;
            };
            let m = world.as_coeffs();
            if m[1].abs() > 1e-9 || m[2].abs() > 1e-9 {
                continue;
            }
            for local in &layout.baselines {
                out.push((world * Point::new(0.0, *local)).y);
            }
            if out.len() >= SNAP_TARGET_LIMIT {
                break;
            }
        }
        out
    }

    /// The delta a move should actually apply, and the lines explaining it:
    /// `raw` constrained to one axis if Shift is held, then snapped along the
    /// axes left. The constraint is enforced inside the snap ([`snap::snap_rect`]'s
    /// `axes`), so no snap slides the layer off the locked line.
    fn snapped_move(&self, raw: Vec2, shift: bool) -> (Vec2, snap::Marks) {
        let axes = snap::move_axes(raw, shift);
        let delta = snap::constrain(raw, axes);
        if self.session.selection.is_empty() {
            return (delta, snap::Marks::default());
        }
        let res = &self.session.resolved;
        let Some(bounds) = self
            .session
            .selection
            .ids()
            .iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|a, b| a.union(b))
        else {
            return (delta, snap::Marks::default());
        };
        let ids = self.session.selection.ids().to_vec();
        let result = snap::snap_rect(bounds + delta, axes, &self.snapping(&ids));
        (delta + result.adjust, result.marks)
    }

    fn apply_marquee(&mut self, origin: Point, end: Point, additive: bool) {
        let area = KRect::new(
            origin.x.min(end.x),
            origin.y.min(end.y),
            origin.x.max(end.x),
            origin.y.max(end.y),
        );
        // A marquee picks frame contents, not the frames, plus loose layers.
        let mut picked = Vec::new();
        for child in self.marquee_candidates() {
            let inside = self
                .session
                .resolved
                .world_bounds(child)
                .is_some_and(|b| overlaps(b, area));
            // Locks are filtered in `marquee_candidates`. Visibility is the node's
            // own flag only, so a layer in a hidden group stays selectable
            // (`context-menus.md` §5.9).
            let selectable = self.session.doc.shown(child);
            if inside && selectable {
                picked.push(child);
            }
        }
        if additive {
            for id in picked {
                self.session.selection.add(id);
            }
        } else {
            self.session.selection.set(picked);
        }
    }

    // --- shape creation ---------------------------------------------------

    /// A world point rounded to the pixel grid, when the grid is on, so a shape is
    /// born on whole pixels. Only the grid, not alignment snapping.
    fn pixel_point(&self, world: Point) -> Point {
        if !self.snap_grid {
            return world;
        }
        Point::new(world.x.round(), world.y.round())
    }

    /// Where the far corner of what is being drawn sits: on the pixel grid, and
    /// with Shift, equal extents for boxes (text included) or a direction for a
    /// line ([`Self::axial_corner`]). The pen arm is unreachable
    /// (`Tool::drags_a_box`) and kept so the fallthrough never squares a pen point.
    fn create_corner(&self, shape: Tool, start: Point, world: Point, shift: bool) -> Point {
        if !shift {
            return self.pixel_point(world);
        }
        match shape {
            Tool::Line => self.axial_corner(start, world),
            Tool::Pen => self.pixel_point(world),
            // Rounded before squaring, or the "square" can be a pixel off.
            _ => squared_corner(start, self.pixel_point(world)),
        }
    }

    /// **Both** ends of what is being drawn — the pair every `tools::create_*`
    /// takes. With Alt the press point is the centre and the corner is mirrored
    /// through it, lines included. Constrain first, mirror second, so Alt+Shift
    /// stays square (or on its octant) and on the pixel grid (§15 D289).
    fn create_box(
        &self,
        shape: Tool,
        start: Point,
        world: Point,
        shift: bool,
        alt: bool,
    ) -> (Point, Point) {
        let corner = self.create_corner(shape, start, world, shift);
        match alt {
            true => centred_ends(start, corner),
            false => (start, corner),
        }
    }

    /// `world` pulled onto the nearest eighth of a turn from `start` — a
    /// Shift-drawn line. The direction is an integer step like `(±1, ±1)`, so a
    /// grid-rounded endpoint keeps the exact angle; the length is the pointer's
    /// projection onto it, as [`snap::constrain`] does for a move.
    fn axial_corner(&self, start: Point, world: Point) -> Point {
        const OCTANT: f64 = std::f64::consts::TAU / 8.0;
        let d = world - start;
        if d.hypot() < 1e-9 {
            return start;
        }
        let angle = (d.y.atan2(d.x) / OCTANT).round() * OCTANT;
        let step = Vec2::new(angle.cos().round(), angle.sin().round());
        let mut t = d.dot(step) / step.dot(step);
        if self.snap_grid {
            t = t.round();
        }
        start + step * t
    }

    /// Put one end of a line at `world`; the geometry is [`tools::move_line_end`]'s.
    ///
    /// Shift constrains to eighths about the other end ([`Self::axial_corner`]);
    /// otherwise the point snaps. Not both: a snapped point forced to 45° is on neither.
    fn line_end_tx(&self, id: NodeId, which: LineEnd, world: Point, shift: bool) -> Transaction {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let Some((start_world, end_world)) = tools::line_ends_world(doc, res, id) else {
            return Transaction::default();
        };
        let anchor = match which {
            LineEnd::Start => end_world,
            LineEnd::End => start_world,
        };
        let target = if shift {
            self.axial_corner(anchor, world)
        } else {
            snap::snap_point(world, (true, true), &self.snapping(&[id])).at
        };
        tools::move_line_end(doc, res, id, which, target)
    }

    /// The two keys a create drag reads in flight (`docs/shortcuts.md` §9):
    /// `Space` repositions the uncommitted shape, `↑`/`↓` add and remove sides.
    /// `dispatch` drops a `Nudge` while a create drag is up, so the arrows do not
    /// also move the selection.
    fn reshape_create(&mut self, ui: &egui::Ui, world: Point) {
        let (space, up, down) = ui.input(|i| {
            (
                i.key_down(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        let Drag::Create {
            start, sides, last, ..
        } = &mut self.drag
        else {
            return;
        };
        // The far corner tracks the pointer, so moving `start` translates the box.
        if space {
            *start += world - *last;
        }
        *last = world;

        *sides = tools::step_sides(*sides, up, down);
    }

    fn begin_create(&mut self, world: Point) {
        let shape = self.tool;
        // Everything goes where it was drawn, frames included (§5.3).
        let (parent, parent_world) = self.draw_target(world);
        let id = self.session.ids.mint();
        self.drag = Drag::Create {
            id,
            parent,
            parent_world,
            start: world,
            shape,
            sides: match shape {
                Tool::Star => tools::STAR_DEFAULT_POINTS,
                _ => tools::POLYGON_DEFAULT_SIDES,
            },
            last: world,
        };
    }

    /// Turn the live `Drag::Create` into a committed layer spanning `from`–`to`:
    /// the one tail for the drag's release and [`Self::click_create`]. Clears
    /// `self.drag` on every path, or the next frame's overlay reads it again.
    fn commit_create(&mut self, id: NodeId, shape: Tool, from: Point, to: Point) {
        let tx = self.create_tx(from, to);
        self.drag = Drag::None;
        if let Some(tx) = tx
            && self.session.commit(tx)
        {
            self.session.selection.set_one(id);
            if shape == Tool::Text {
                self.open_text_session(id);
            } else if shape == Tool::Image {
                self.placed_one_image();
            } else {
                self.choose_tool(Tool::Select);
                self.session.info("Created shape");
            }
        }
    }

    /// A shape at [`tools::CLICK_CREATE_SIZE`] where the pointer clicked, for the
    /// tools whose click answers [`ClickCreate::DefaultBox`]. Synthesizes a
    /// short-lived `Drag::Create` for the release's tail, not a second creation
    /// path. The press is the top-left corner, or the centre with Alt.
    fn click_create(&mut self, world: Point, alt: bool) {
        self.begin_create(self.pixel_point(world));
        let Drag::Create {
            id, start, shape, ..
        } = self.drag
        else {
            return;
        };
        let (from, to) = click_box(start, alt);
        self.commit_create(id, shape, from, to);
    }

    /// Build the create transaction for the in-flight `Drag::Create`. Both ends
    /// are passed in: under Alt the near one is mirrored ([`Self::create_box`]).
    fn create_tx(&self, start: Point, corner: Point) -> Option<Transaction> {
        let Drag::Create {
            id,
            parent,
            parent_world,
            shape,
            sides,
            ..
        } = &self.drag
        else {
            return None;
        };
        let (id, parent, parent_world, shape, sides) =
            (*id, *parent, *parent_world, *shape, *sides);
        let index = self.child_count(parent);
        Some(match shape {
            Tool::Frame => {
                tools::create_artboard(id, parent, index, parent_world, start, corner, Color::WHITE)
            }
            Tool::Rect => tools::create_rect(
                id,
                parent,
                index,
                parent_world,
                start,
                corner,
                default_shape_fill(),
            ),
            Tool::Ellipse => tools::create_ellipse(
                id,
                parent,
                index,
                parent_world,
                start,
                corner,
                default_shape_fill(),
            ),
            Tool::Polygon => tools::create_polygon(
                id,
                parent,
                index,
                parent_world,
                start,
                corner,
                sides,
                default_shape_fill(),
            ),
            Tool::Star => tools::create_star(
                id,
                parent,
                index,
                parent_world,
                start,
                corner,
                sides,
                default_shape_fill(),
            ),
            Tool::Line => tools::create_line(
                id,
                parent,
                index,
                parent_world,
                start,
                corner,
                PEN_STROKE_COLOR,
                PEN_STROKE_WIDTH,
            ),
            Tool::Text => tools::create_text(
                id,
                parent,
                index,
                parent_world,
                start,
                corner,
                &crate::fonts::FontService::default_text_parts(),
            ),
            // `None` when the cursor has nothing left to place.
            Tool::Image => {
                let pending = self.pending_images.first()?;
                let (pid, entry, given) = (
                    pending.id.clone(),
                    (!self.session.doc.has_image(&pending.id)).then(|| pending.entry.clone()),
                    pending.name.clone(),
                );
                tools::create_image_in_box(
                    id,
                    parent,
                    index,
                    parent_world,
                    start,
                    corner,
                    pid,
                    entry,
                    Some(self.image_layer_name(parent, given)),
                )
            }
            // Not box-dragging tools (`drags_a_box`).
            Tool::Pen
            | Tool::Node
            | Tool::ImageEdit
            | Tool::Select
            | Tool::Scale
            | Tool::Hand
            | Tool::Eyedropper => {
                return None;
            }
        })
    }

    /// The topmost frame **containing** the point, if any. No fallback — see
    /// [`Self::draw_target`], which is what creation goes through.
    fn target_artboard(&self, world: Point) -> Option<(NodeId, Affine)> {
        // Reversed: `artboards()` is in z-order, so the last hit is the topmost.
        let id = self.artboards().into_iter().rev().find(|id| {
            self.session
                .resolved
                .world_bounds(*id)
                .is_some_and(|b| b.contains(world))
        })?;
        Some((id, self.session.resolved.world_transform(id)?))
    }

    /// Where a layer drawn at `world` belongs: the frame under the point, or the
    /// document root; there is no fallback frame.
    fn draw_target(&self, world: Point) -> (NodeId, Affine) {
        self.target_artboard(world)
            .unwrap_or_else(|| (self.session.doc.root(), Affine::IDENTITY))
    }

    // --- pen --------------------------------------------------------------

    /// The pen's whole gesture loop. Press plants an anchor; dragging pulls its
    /// handles out symmetrically; release settles it. On press, not click, since
    /// a click resolves only after the shaping drag.
    fn pen_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, ppp: f32) {
        let (enter, pressed, released, alt, shift) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.modifiers.alt,
                i.modifiers.shift,
            )
        });
        // Escape also finishes, but `OndinApp::escape` owns it.
        if enter {
            self.finish_pen(false);
            return;
        }
        // A double-click finishes, dropping the duplicate anchor its second press planted.
        if crate::ui::double_clicked(ui, resp) {
            if let Some(pen) = self.pen.as_mut()
                && pen.anchors.len() > 1
            {
                pen.anchors.pop();
            }
            self.finish_pen(false);
            return;
        }

        if pressed
            && resp.contains_pointer()
            && let Some(p) = ui.ctx().pointer_interact_pos()
        {
            self.pen_press(self.to_world(p, rect, ppp), shift);
        }
        if self.pen.as_ref().is_some_and(|p| p.shaping)
            && let Some(p) = resp.hover_pos().or_else(|| resp.interact_pointer_pos())
        {
            let retract = f64::from(PEN_PICK_PX) / self.points_per_world();
            // The drag is the outgoing handle; the incoming one mirrors it unless
            // Alt breaks the pair. Within `PEN_PICK_PX` the handle retracts onto the
            // anchor, or a click on a snapped anchor leaves a hair of a curve.
            let tangent = f64::from(TANGENT_SNAP_PX) / self.points_per_world();
            let world = self.to_world(p, rect, ppp);
            if let Some(last) = self.pen.as_mut().and_then(|s| s.anchors.last_mut()) {
                let aim = tools::tangent_snap(last, Side::Out, world, tangent);
                last.pull_within(Side::Out, aim, alt, retract);
            }
            ui.ctx().request_repaint();
        }
        if released && let Some(pen) = self.pen.as_mut() {
            pen.shaping = false;
        }
    }

    /// Where a pen anchor pressed at `world` lands, and the lines explaining it.
    ///
    /// Snaps on both axes through [`Self::snapping`], excluding the node being
    /// resumed. Shift instead constrains the segment to an eighth of a turn from
    /// the previous anchor ([`Self::axial_corner`]); the first has nothing to constrain.
    fn pen_point(&self, world: Point, shift: bool) -> (Point, Vec<snap::Guide>) {
        if shift && let Some(from) = self.pen.as_ref().and_then(|p| p.anchors.last()) {
            return (self.axial_corner(from.at, world), Vec::new());
        }
        let moving: Vec<NodeId> = self
            .pen
            .as_ref()
            .and_then(|p| p.editing.as_ref())
            .map(|e| vec![e.id])
            .unwrap_or_default();
        let snapped = snap::snap_point(world, (true, true), &self.snapping(&moving));
        (snapped.at, snapped.guides)
    }

    fn pen_press(&mut self, world: Point, shift: bool) {
        if self.pen.is_none() {
            if self.pen_resume(world) {
                return;
            }
            let at = self.pen_point(world, shift).0;
            let (parent, parent_world) = self.draw_target(at);
            self.pen = Some(PenState {
                parent,
                parent_world,
                anchors: vec![PenAnchor::corner(at)],
                shaping: true,
                editing: None,
            });
            self.session
                .info("Pen: click for a corner, drag for a curve; Enter or double-click to finish");
            return;
        }
        // Snapped before the close test, so a press closes only where the preview showed.
        let at = self.pen_point(world, shift).0;
        let points_per_world = self.points_per_world();
        let Some(pen) = self.pen.as_mut() else {
            return;
        };
        let closes =
            tools::pen_close_snap(&pen.anchors, at, points_per_world, f64::from(PEN_PICK_PX));
        if closes.is_some() {
            self.finish_pen(true);
            // `finish_pen` switched to Select; swallow the release, or it arrives
            // as a `clicked()` that re-picks.
            self.gesture_cancelled = true;
            return;
        }
        if let Some(target) = self.pen_join_target(world)
            && self.pen_join(target)
        {
            self.gesture_cancelled = true;
            return;
        }
        let Some(pen) = self.pen.as_mut() else { return };
        pen.anchors.push(PenAnchor::corner(at));
        pen.shaping = true;
    }

    /// The open endpoint at `world` that a press would join to, if there is one.
    /// Asked by the press and the hover ring. The run's own far end is a close
    /// (`pen_close_snap`), never a join.
    fn pen_join_target(&self, world: Point) -> Option<PenEdit> {
        let pen = self.pen.as_ref()?;
        let target = self.pen_resume_target_in(world, self.pen_scope())?;
        let same_run = pen
            .editing
            .as_ref()
            .is_some_and(|e| e.id == target.id && e.active == target.active);
        (!same_run).then_some(target)
    }

    /// Join the run being drawn to the open endpoint under the pointer, and finish.
    ///
    /// The node already held survives and the other is absorbed and deleted; with
    /// no node held, the target survives. `false` leaves the pen untouched for the
    /// press to place an anchor.
    fn pen_join(&mut self, target: PenEdit) -> bool {
        let Some(pen) = self.pen.as_ref() else {
            return false;
        };
        let mut sub = tools::join_runs(
            &pen.anchors,
            &target.subpaths[target.active],
            target.from_start,
        );
        let (host, host_world, subpaths, absorbed) = match &pen.editing {
            Some(edit) => {
                if edit.from_start {
                    // The working copy was reversed to draw; restore the order.
                    sub.reverse();
                }
                let mut list = edit.subpaths.clone();
                list[edit.active] = sub;
                let same = edit.id == target.id;
                if same {
                    list.remove(target.active);
                } else {
                    list.extend(
                        target
                            .subpaths
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| *i != target.active)
                            .map(|(_, s)| s.clone()),
                    );
                }
                (edit.id, edit.world, list, (!same).then_some(target.id))
            }
            None => {
                let mut list = target.subpaths.clone();
                list[target.active] = sub;
                (target.id, target.world, list, None)
            }
        };
        let mut tx = tools::rewrite_path(host, host_world, &subpaths, self.stored_path(host));
        if tx.0.is_empty() {
            return false;
        }
        if let Some(other) = absorbed {
            tx.0.push(Operation::DeleteNode { id: other });
        }
        if !self.session.commit(tx) {
            return false;
        }
        self.pen = None;
        self.end_pen_gesture(host);
        self.session.info(match host == target.id {
            true => "Joined two subpaths of this path",
            false => "Joined paths",
        });
        true
    }

    /// Pressing an **open** path's endpoint with the pen (grabbed at
    /// [`PEN_PICK_PX`]) carries on drawing that path. The pen only appends, so
    /// resuming at a start reverses the run (`PenSubpath::reverse`); `shaping`
    /// stays off so the endpoint keeps its handles.
    fn pen_resume(&mut self, world: Point) -> bool {
        let Some(edit) = self.pen_resume_target(world) else {
            return false;
        };
        self.begin_pen_edit(edit);
        self.session
            .info("Resumed path: click to extend; Enter or double-click to finish");
        true
    }

    /// The open path endpoint at `world`, read into the edit that would resume it;
    /// asked by the press and the hover ring.
    fn pen_resume_target(&self, world: Point) -> Option<PenEdit> {
        self.pen_resume_target_in(world, None)
    }

    /// The node the pen may reach while it is armed inside an edit, and `None`
    /// when it may reach anything — the standalone tool. Inside an edit every pen
    /// verb stays in the edited node; reaching another layer would be a node merge
    /// with a second join rule (§15 D125).
    fn pen_scope(&self) -> Option<NodeId> {
        self.pen_bias.then(|| self.edited_path()).flatten()
    }

    /// [`Self::pen_resume_target`], restricted to one node when `only` names one.
    fn pen_resume_target_in(&self, world: Point, only: Option<NodeId>) -> Option<PenEdit> {
        let slop = f64::from(PEN_PICK_PX) / self.points_per_world();
        hit_test(&self.session.resolved, &self.session.doc, world, slop)
            .iter()
            .filter(|id| only.is_none_or(|want| **id == want))
            .find_map(|id| {
                let NodeKind::Path { path, corner_radii } = self.session.doc.get(*id)?.kind()
                else {
                    return None;
                };
                let node_world = self.session.resolved.world_transform(*id)?;
                let mut subpaths = tools::pen_anchors(path, corner_radii);
                tools::map_subpaths(&mut subpaths, node_world);
                let (active, from_start) = subpaths.iter().enumerate().find_map(|(i, sub)| {
                    if sub.closed || sub.anchors.len() < 2 {
                        return None;
                    }
                    let first = (sub.anchors[0].at - world).hypot();
                    let last = (sub.anchors[sub.anchors.len() - 1].at - world).hypot();
                    (first.min(last) <= slop).then_some((i, first <= last))
                })?;
                Some(PenEdit {
                    id: *id,
                    world: node_world,
                    subpaths,
                    active,
                    from_start,
                    minted: false,
                })
            })
    }

    /// What a press at `world` means while the pen is armed inside an edit — see
    /// [`PenVerb`]. Asked by the press and the cursor.
    fn pen_verb(&self, world: Point) -> Option<PenVerb> {
        let id = self.pen_scope()?;
        if let Some(edit) = self.pen_resume_target_in(world, Some(id)) {
            return Some(PenVerb::Continue(Box::new(edit)));
        }
        match self.node_grab(world) {
            Some(NodeGrab::Segment(at, t)) => Some(PenVerb::Insert(at, t)),
            Some(_) => None,
            // Another layer still switches subject (`node_press`); the edited
            // path's own fill counts as empty.
            None => self
                .pick_leaf(world)
                .is_none_or(|under| under == id)
                .then_some(PenVerb::NewSubpath),
        }
    }

    /// Take the press with the verb [`Self::pen_verb`] named. `shift` is the
    /// pen's own constraint (`pen_point`), not the node tool's extend.
    fn pen_verb_press(&mut self, id: NodeId, world: Point, shift: bool, verb: PenVerb) {
        match verb {
            PenVerb::Continue(edit) => {
                self.begin_pen_edit(*edit);
                self.session
                    .info("Carrying on from this end: click to extend, Enter to stop");
            }
            PenVerb::Insert(at, t) => self.insert_point(id, at, t),
            PenVerb::NewSubpath => {
                let Some((world_t, mut subpaths)) = self.edited_subpaths(id) else {
                    return;
                };
                let active = subpaths.len();
                subpaths.push(PenSubpath {
                    anchors: Vec::new(),
                    closed: false,
                });
                self.begin_pen_edit(PenEdit {
                    id,
                    world: world_t,
                    subpaths,
                    active,
                    from_start: false,
                    minted: true,
                });
                // Snapped only once `editing` is set, so `pen_point` excludes the
                // host from the targets for the first anchor too.
                let at = self.pen_point(world, shift).0;
                if let Some(pen) = self.pen.as_mut() {
                    pen.anchors = vec![PenAnchor::corner(at)];
                    pen.shaping = true;
                }
                self.session.info(
                    "New subpath in this path — draw it the other way round to cut a hole; \
                     Shift+D reverses one after the fact",
                );
            }
        }
    }

    /// Start a pen gesture on an existing node without changing the tool, so a
    /// node-tool edit is still standing when the gesture ends (`finish_pen`).
    /// The working run is a copy, reversed when the grab was at the start.
    fn begin_pen_edit(&mut self, edit: PenEdit) {
        let parent = self.session.doc.get(edit.id).and_then(|n| n.parent());
        let parent_world = parent
            .and_then(|p| self.session.resolved.world_transform(p))
            .unwrap_or(Affine::IDENTITY);
        self.pen = Some(PenState {
            parent: parent.unwrap_or(self.session.doc.root()),
            parent_world,
            anchors: edit.working(),
            shaping: false,
            editing: Some(edit),
        });
        self.points.clear();
    }

    // --- node tool ----------------------------------------------------------

    /// The path the node tool is editing: the selection, when it is a single
    /// `Path` and the tool is armed. Derived from the selection, not a second field.
    pub(crate) fn edited_path(&self) -> Option<NodeId> {
        if self.tool != Tool::Node {
            return None;
        }
        let [id] = *self.session.selection.ids() else {
            return None;
        };
        // The funnel every reader passes through: several doors arm the tool
        // without a lock check.
        if ondin_core::is_effectively_locked(&self.session.doc, id) {
            return None;
        }
        matches!(
            self.session.doc.get(id).map(|n| n.kind()),
            Some(NodeKind::Path { .. })
        )
        .then_some(id)
    }

    /// The image fill being edited: the node, and which fill — derived, like
    /// `edited_path`. The first visible image fill (`tools::croppable_fill`).
    pub(crate) fn edited_image(&self) -> Option<(NodeId, usize)> {
        if self.tool != Tool::ImageEdit {
            return None;
        }
        let [id] = *self.session.selection.ids() else {
            return None;
        };
        // The layers panel can lock a picture while the mode is armed.
        if ondin_core::is_effectively_locked(&self.session.doc, id) {
            return None;
        }
        let i = tools::croppable_fill(self.session.doc.get(id)?.paint())?;
        Some((id, i))
    }

    /// The cropped node's frame: its world transform, and its box in local space
    /// — the rectangle `ImageRef::framing` is given, so gesture and renderer agree.
    fn crop_frame(&self, id: NodeId) -> Option<(Affine, KRect)> {
        let world = self.session.resolved.world_transform(id)?;
        let kind = self.session.resolved.used_kind(&self.session.doc, id)?;
        let frame = {
            use ondin_core::kurbo::Shape;
            ondin_core::geometry::local_path(kind)?.bounding_box()
        };
        (frame.width() > 0.0 && frame.height() > 0.0).then_some((world, frame))
    }

    /// The transaction a crop **pan** previews and commits: one `SetFills` with
    /// the rectangle changed and the mode forced to `Crop`.
    fn crop_tx(&self, id: NodeId, fill: usize, crop: KRect) -> Option<Transaction> {
        let mut fills = self.session.doc.get(id)?.paint().fills.clone();
        let Some(ondin_core::Brush::Image(img)) = fills.get_mut(fill)?.brush.literal_mut() else {
            return None;
        };
        img.image.fit = ondin_core::ImageFit::Crop;
        img.image.crop = crop;
        Some(Transaction(vec![Operation::SetFills { id, fills }]))
    }

    /// The transaction a crop **handle** drag previews and commits: the node
    /// resized, and the crop reframed so the picture does not move — one
    /// transaction, so neither edit can show or undo without the other. The resize
    /// goes through [`tools::resize_layer`], aimed at the corner of
    /// [`tools::crop_resized`]'s rectangle.
    #[allow(clippy::too_many_arguments)]
    fn crop_resize_tx(
        &self,
        id: NodeId,
        fill: usize,
        handle: Handle,
        frame: KRect,
        start: KRect,
        limit: KRect,
        world: Affine,
        at: Point,
    ) -> Option<Transaction> {
        let next = tools::crop_resized(frame, handle, at, limit);
        let (ux, uy) = handle.unit();
        let corner = Point::new(
            next.min_x() + next.width() * ux,
            next.min_y() + next.height() * uy,
        );
        let mut tx = tools::resize_layer(
            &self.session.doc,
            &self.session.resolved,
            id,
            handle,
            world * corner,
            tools::Resize::geometry(false, false),
        );
        if tx.0.is_empty() {
            return None;
        }
        let mut fills = self.session.doc.get(id)?.paint().fills.clone();
        let Some(ondin_core::Brush::Image(img)) = fills.get_mut(fill)?.brush.literal_mut() else {
            return None;
        };
        img.image.fit = ondin_core::ImageFit::Crop;
        img.image.crop = ondin_core::crop_reframed(start, frame, next);
        tx.0.push(Operation::SetFills { id, fills });
        Some(tx)
    }

    /// What a press seeds a crop gesture from: the crop rectangle in force, and
    /// the rectangle the whole picture occupies in the node's local space. Read
    /// from the committed document, since a preview would compound;
    /// [`Self::shown_crop`] is the on-screen twin.
    fn crop_state(&self, id: NodeId, fill: usize) -> Option<(KRect, KRect)> {
        let Some(ondin_core::Brush::Image(img)) = self
            .session
            .doc
            .get(id)?
            .paint()
            .fills
            .get(fill)?
            .brush
            .literal()
        else {
            return None;
        };
        let (_, frame) = self.crop_frame(id)?;
        Self::crop_and_limit(&img.image, frame, self.session.doc.image(&img.image.id)?)
    }

    /// The crop rectangle and the picture's box **as on screen**, read through any
    /// live preview, with the frame measured against: [`Self::crop_state`]'s twin.
    fn shown_crop(&self, id: NodeId, fill: usize) -> Option<(KRect, KRect, KRect)> {
        let node = self.session.display_node(id)?;
        let Some(ondin_core::Brush::Image(img)) = node.paint().fills.get(fill)?.brush.literal()
        else {
            return None;
        };
        let frame = self.session.preview_local_box(id)?;
        if frame.width() <= 0.0 || frame.height() <= 0.0 {
            return None;
        }
        let (crop, limit) =
            Self::crop_and_limit(&img.image, frame, self.session.doc.image(&img.image.id)?)?;
        Some((crop, limit, frame))
    }

    /// The rectangle this reference is cropped to in `frame`, and where the whole
    /// picture sits around it. A picture not yet in `Crop` mode seeds
    /// `ImageRef::cover_crop` (a `Fit` picture loses its letterbox, which no crop
    /// can express); the limit is asked of a reference forced to `Crop`.
    fn crop_and_limit(
        img: &ondin_core::ImageRef,
        frame: KRect,
        entry: &ondin_core::ImageEntry,
    ) -> Option<(KRect, KRect)> {
        let (w, h) = (entry.width, entry.height);
        let crop = if img.fit == ondin_core::ImageFit::Crop {
            img.crop
        } else {
            img.cover_crop(frame, w, h)
        };
        let cropped = ondin_core::ImageRef {
            fit: ondin_core::ImageFit::Crop,
            crop,
            ..img.clone()
        };
        Some((crop, cropped.picture_box(frame, w, h)?))
    }

    /// Whether the faded original and its outline are shown: in `Crop` mode only,
    /// since the other modes re-derive the position from the frame. Asked of the
    /// displayed brush, so the ghost arrives on a drag's first frame.
    fn image_ghost_live(&self, id: NodeId, fill: usize) -> bool {
        let Some(node) = self.session.display_node(id) else {
            return false;
        };
        matches!(
            node.paint().fills.get(fill).and_then(|f| f.brush.literal()),
            Some(ondin_core::Brush::Image(img)) if img.image.fit == ondin_core::ImageFit::Crop
        )
    }

    /// The faded whole photograph drawn behind the layer being cropped.
    ///
    /// A `Ghost` at the layer's own index, so the real fill covers the middle and
    /// the offcut shows round it: a `Rect` at [`ondin_core::ImageRef::picture_box`]
    /// with `whole_crop`, carrying the reference's orientation and adjustments.
    /// Re-installed every frame by `draw_canvas` (`RenderOverrides::chrome`).
    fn image_ghost(&self) -> Option<(NodeId, ondin_render::Ghost)> {
        let (id, fill) = self.edited_image()?;
        if !self.image_ghost_live(id, fill) {
            return None;
        }
        let (_, limit, _) = self.shown_crop(id, fill)?;
        let node = self.session.doc.get(id)?;
        let parent = node.parent()?;
        let index = self
            .session
            .doc
            .get(parent)?
            .children()
            .iter()
            .position(|c| *c == id)?;
        let display = self.session.display_node(id)?;
        let Some(ondin_core::Brush::Image(img)) = display.paint().fills.get(fill)?.brush.literal()
        else {
            return None;
        };
        let image = ondin_core::ImageBrush {
            image: ondin_core::ImageRef {
                fit: ondin_core::ImageFit::Crop,
                crop: ondin_core::whole_crop(),
                ..img.image.clone()
            },
            sampler: img.sampler,
        };
        // Paired with its layer so the two pixel-snap together (`unsnapped`).
        Some((
            id,
            ondin_render::Ghost {
                parent,
                index,
                node: ondin_render::GhostNode {
                    // A sentinel: a real id would answer lookups for the node, a
                    // minted one would spend a `seq`.
                    id: GHOST_ID,
                    transform: self.session.preview_local_transform(id)?
                        * Affine::translate(limit.origin().to_vec2()),
                    kind: NodeKind::Rect {
                        size: limit.size(),
                        corner_radii: ondin_core::kurbo::RoundedRectRadii::from_single_radius(0.0),
                    },
                    paint: ondin_core::Paint {
                        fills: ondin_core::keyed_by_position([ondin_core::Fill {
                            brush: ondin_core::Brush::Image(image).into(),
                            visible: true,
                        }]),
                        strokes: Vec::new(),
                    },
                    effects: Vec::new(),
                    text: None,
                    path: None,
                    opacity: display.opacity() * GHOST_ALPHA,
                    visible: true,
                    clip: false,
                    mask: false,
                    mask_mode: ondin_core::MaskMode::default(),
                    children: Vec::new(),
                },
            },
        ))
    }

    /// The edited path's subpaths in **world** space from the committed document —
    /// the fixed origin an edit is computed from, since the preview compounds.
    fn edited_subpaths(&self, id: NodeId) -> Option<(Affine, Vec<PenSubpath>)> {
        let NodeKind::Path { path, corner_radii } = self.session.doc.get(id)?.kind() else {
            return None;
        };
        let world = self.session.resolved.world_transform(id)?;
        let mut subpaths = tools::pen_anchors(path, corner_radii);
        tools::map_subpaths(&mut subpaths, world);
        Some((world, subpaths))
    }

    /// The same, read **through the preview** — where the points are right now.
    /// For anything that shows a point and for an incremental edit like a scrub
    /// ([`Self::nudge_points_tx`]).
    fn preview_subpaths(&self, id: NodeId) -> Option<(Affine, Vec<PenSubpath>)> {
        let NodeKind::Path { path, corner_radii } = self.session.display_node(id)?.kind() else {
            return None;
        };
        let world = self.session.preview_world_transform(id)?;
        let mut subpaths = tools::pen_anchors(path, corner_radii);
        tools::map_subpaths(&mut subpaths, world);
        Some((world, subpaths))
    }

    /// What the pointer would take in image editing, as a cursor: a resize arrow
    /// over a handle, an open hand over the picture, `None` off the frame.
    fn crop_hover(
        &self,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<(egui::CursorIcon, Option<Bitmap>)> {
        let (id, fill) = self.edited_image()?;
        let (world, frame) = self.crop_frame(id)?;
        let at = world.inverse() * self.to_world(resp.hover_pos()?, rect, ppp);
        match (self.crop_handle_at(frame, at), frame.contains(at)) {
            (Some(h), _) => {
                let (ox, oy) = h.outward();
                let d = (world * Point::new(ox, oy)) - (world * Point::ZERO);
                Some((resize_icon(egui::vec2(d.x as f32, d.y as f32)), None))
            }
            (None, true) => (self.crop_state(id, fill).is_some())
                .then_some((egui::CursorIcon::Grab, Some(Bitmap::HandOpen))),
            (None, false) => None,
        }
    }

    /// The crop handle under a local-space point: a corner first (it lies in two
    /// side bands), then a side.
    fn crop_handle_at(&self, frame: KRect, at: Point) -> Option<Handle> {
        let zoom = self.points_per_world();
        Handle::CORNERS
            .into_iter()
            .map(|h| (h, (box_handle_at(frame, h) - at).hypot()))
            .filter(|(_, d)| *d <= f64::from(HANDLE_PICK_PX) / zoom)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
            .or_else(|| {
                Handle::EDGES
                    .into_iter()
                    .map(|h| (h, box_edge_distance(frame, h, at)))
                    .filter(|(_, d)| *d <= f64::from(EDGE_PICK_PX) / zoom)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(h, _)| h)
            })
    }

    /// Image editing's press/drag/release. A handle moves an edge of the window
    /// (and the layer) with the picture fixed; a drag on the picture slides it
    /// under a fixed window. Neither scales the photograph.
    fn image_edit_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let (pressed, released) =
            ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
        let Some((id, fill)) = self.edited_image() else {
            return;
        };
        let Some((world, frame)) = self.crop_frame(id) else {
            return;
        };
        // Local space, so a rotated layer crops along its own axes.
        let Some(inv) = world.inverse().into() else {
            return;
        };
        let local = |p: egui::Pos2, app: &Self| -> Point { inv * app.to_world(p, rect, ppp) };

        if pressed
            && resp.contains_pointer()
            && let Some(p) = ui.ctx().pointer_interact_pos()
        {
            let at = local(p, self);
            let grab_world = self.to_world(p, rect, ppp);
            let Some((start, limit)) = self.crop_state(id, fill) else {
                return;
            };
            self.drag = match self.crop_handle_at(frame, at) {
                Some(handle) => Drag::CropResize {
                    id,
                    fill,
                    handle,
                    frame,
                    start,
                    limit,
                    grab_world,
                },
                None if frame.contains(at) => Drag::CropPan {
                    id,
                    fill,
                    start,
                    grab: at,
                    grab_world,
                },
                None => Drag::None,
            };
        }

        let Some(p) = resp.hover_pos().or_else(|| resp.interact_pointer_pos()) else {
            return;
        };
        let at = local(p, self);
        let world_at = self.to_world(p, rect, ppp);
        // The subject comes from the drag captured at the press; `travelled`
        // gates the commit only.
        let (next, travelled) = match self.drag {
            Drag::CropPan {
                id,
                fill,
                start,
                grab,
                grab_world,
            } => (
                self.crop_tx(id, fill, ondin_core::crop_panned(start, at - grab, frame)),
                self.gesture_travelled(world_at - grab_world),
            ),
            Drag::CropResize {
                id,
                fill,
                handle,
                frame,
                start,
                limit,
                grab_world,
            } => (
                self.crop_resize_tx(id, fill, handle, frame, start, limit, world, at),
                self.gesture_travelled(world_at - grab_world),
            ),
            _ => (None, false),
        };
        if let Some(tx) = next {
            self.session.set_preview(&tx);
            if released {
                self.drag = Drag::None;
                // Even a zero-delta pan forces `fit = Crop`, so a still click would
                // flip a `Fill` picture's mode. The preview stays ungated for
                // `image_ghost_live`.
                if travelled {
                    self.session.commit(tx);
                } else {
                    self.session.clear_gesture_preview();
                }
            }
        } else if released {
            self.drag = Drag::None;
        }
    }

    fn node_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, ppp: f32) {
        let (pressed, released, shift, alt, ctrl) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.modifiers.shift,
                i.modifiers.alt,
                i.modifiers.ctrl || i.modifiers.command,
            )
        });
        if pressed
            && resp.contains_pointer()
            && let Some(p) = ui.ctx().pointer_interact_pos()
        {
            self.node_press(self.to_world(p, rect, ppp), shift, ctrl);
        }
        let Some(p) = resp.hover_pos().or_else(|| resp.interact_pointer_pos()) else {
            return;
        };
        let world = self.to_world(p, rect, ppp);
        // A double-click on a segment inserts an anchor, cancelling the drag the
        // second press armed.
        if crate::ui::double_clicked(ui, resp)
            && let Drag::Segment { id, at, t, .. } = self.drag
        {
            self.drag = Drag::None;
            self.insert_point(id, at, t);
            return;
        }
        match self.drag {
            Drag::Points { id, grab, anchor } => {
                let base = self.anchor_position(id, anchor);
                let (delta, _) = self.snapped_point(id, base, world - grab, shift);
                let tx = self.points_tx(id, delta);
                self.session.set_preview(&tx);
                if released {
                    self.drag = Drag::None;
                    // Empty when the pointer never moved: a click leaves no undo step.
                    self.session.commit(tx);
                }
            }
            Drag::Handle { id, at, side } => {
                let tx = self.handle_tx(id, at, side, world, alt);
                self.session.set_preview(&tx);
                if released {
                    self.drag = Drag::None;
                    self.session.commit(tx);
                }
            }
            // Plain moves; Ctrl (read live) shapes the curvature.
            Drag::Segment { id, at, t, grab } => {
                let raw = world - grab;
                // A bend moves control points and does not snap.
                let delta = match ctrl {
                    true => raw,
                    false => {
                        let base = self.segment_position(id, at, t);
                        self.snapped_point(id, base, raw, shift).0
                    }
                };
                let tx = self.segment_tx(id, at, t, delta, ctrl);
                self.session.set_preview(&tx);
                if released {
                    self.drag = Drag::None;
                    self.session.commit(tx);
                }
            }
            Drag::PointBox {
                id,
                handle,
                box_,
                grab,
            } => {
                let (at, _) = self.snapped_point_box(id, handle, grab, world);
                let tx = self.point_box_tx(id, handle, box_, at, shift, alt);
                self.session.set_preview(&tx);
                if released {
                    self.drag = Drag::None;
                    self.session.commit(tx);
                }
            }
            Drag::PointRotate { id, pivot, start } => {
                let tx = self.point_rotate_tx(id, pivot, start, world, shift);
                self.session.set_preview(&tx);
                if released {
                    self.drag = Drag::None;
                    self.session.commit(tx);
                }
            }
            Drag::Marquee { origin, additive } if released => {
                self.drag = Drag::None;
                self.marquee_points(origin, world, additive);
            }
            _ => {}
        }
    }

    /// What the node tool would grab at `world`.
    ///
    /// Handles outrank anchors, but only a drawn one counts, matching
    /// `draw_points`: a subject anchor's ([`Self::subject_points`]) non-zero
    /// handle. A corner's zero-length handles would otherwise turn a second click
    /// into a curve; `Ctrl`+drag does that.
    fn node_grab(&self, world: Point) -> Option<NodeGrab> {
        let id = self.edited_path()?;
        let (_, subpaths) = self.edited_subpaths(id)?;
        let slop = f64::from(PEN_PICK_PX) / self.points_per_world();
        let anchors = || {
            subpaths.iter().enumerate().flat_map(|(s, sub)| {
                sub.anchors
                    .iter()
                    .enumerate()
                    .map(move |(a, an)| ((s, a), an))
            })
        };
        let nearest = |a: &(NodeGrab, Point), b: &(NodeGrab, Point)| {
            (a.1 - world).hypot().total_cmp(&(b.1 - world).hypot())
        };
        // A selected segment's ends draw their handles, so they offer them too.
        let subject = self.subject_points(id);
        let handle = anchors()
            .filter(|(at, _)| subject.contains(at))
            .flat_map(|(at, an)| {
                Side::BOTH.map(|side| (NodeGrab::Handle(at, side), an.handle(side), an.at))
            })
            .filter(|(_, p, at)| (*p - *at).hypot() > HANDLE_DRAWN)
            .map(|(grab, p, _)| (grab, p))
            .filter(|(_, p)| (*p - world).hypot() <= slop)
            .min_by(nearest);
        if let Some((grab, _)) = handle {
            return Some(grab);
        }
        let anchor = anchors()
            .map(|(at, an)| (NodeGrab::Anchor(at), an.at))
            .filter(|(_, p)| (*p - world).hypot() <= slop)
            .min_by(nearest);
        if let Some((grab, _)) = anchor {
            return Some(grab);
        }
        let zoom = self.points_per_world();
        if let Some(box_) = self.point_box(id) {
            let corner = Handle::CORNERS
                .into_iter()
                .map(|h| (NodeGrab::Box(h), box_handle_at(box_, h)))
                .filter(|(_, p)| (*p - world).hypot() <= f64::from(HANDLE_PICK_PX) / zoom)
                .min_by(nearest);
            if let Some((grab, _)) = corner {
                return Some(grab);
            }
            // Sides are invisible bands, after the corners (a corner lies in two).
            let side = Handle::EDGES
                .into_iter()
                .map(|h| (NodeGrab::Box(h), box_edge_distance(box_, h, world)))
                .filter(|(_, d)| *d <= f64::from(EDGE_PICK_PX) / zoom)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((grab, _)) = side {
                return Some(grab);
            }
            // `Handles::rotate_zone`'s rule in world space.
            let ring = (
                f64::from(HANDLE_PICK_PX) / zoom,
                f64::from(ROTATE_RING_PX) / zoom,
            );
            if !box_.contains(world)
                && Handle::CORNERS.into_iter().any(|h| {
                    let d = (box_handle_at(box_, h) - world).hypot();
                    d > ring.0 && d <= ring.0 + ring.1
                })
            {
                return Some(NodeGrab::BoxRotate);
            }
        }
        // Finally the ink, numbered by core's own walk (`geometry::nearest_segment`).
        // Rejected first against the control hull, which contains the ink, so the
        // per-frame `BezPath` build is skipped off the path. Not a revision-keyed
        // cache: a preview changes without moving `EditorSession::revision`.
        let hull = subpaths
            .iter()
            .flat_map(|s| s.anchors.iter())
            .flat_map(|a| [a.at, a.leaving(), a.arriving()])
            .fold(None::<KRect>, |acc, p| {
                Some(match acc {
                    Some(r) => r.union_pt(p),
                    None => KRect::from_points(p, p),
                })
            })?;
        if !hull.inflate(slop, slop).contains(world) {
            return None;
        }
        self.segment_walks.set(self.segment_walks.get() + 1);
        let hit = geometry::nearest_segment(&tools::pen_path(&subpaths), world)?;
        (hit.distance <= slop).then_some(NodeGrab::Segment((hit.subpath, hit.anchor), hit.t))
    }

    /// The box over the selected points, in world space — the frame a scale
    /// gesture is measured in. Two points at least, read from the preview so it
    /// tracks the drag; a single selected segment gets none (it has its own
    /// move and bend).
    fn point_box(&self, id: NodeId) -> Option<KRect> {
        if self.points.segment_count(id) == 1 {
            return None;
        }
        let points: Vec<PointRef> = self.subject_points(id);
        if points.len() < 2 {
            return None;
        }
        let (_, subpaths) = self.preview_subpaths(id)?;
        tools::points_box(&subpaths, &points)
    }

    fn node_press(&mut self, world: Point, shift: bool, ctrl: bool) {
        let id = match self.edited_path() {
            Some(id) => id,
            None => {
                // Nothing under edit yet: this press picks a path (the leaf) only.
                match self.pick_leaf(world).filter(|id| self.is_path(*id)) {
                    Some(id) => self.session.selection.set_one(id),
                    None => self.session.selection.clear(),
                }
                self.points.clear();
                return;
            }
        };
        // The pen's verbs first, when armed; [`PenVerb`] declines targets it does not claim.
        if let Some(verb) = self.pen_verb(world) {
            self.pen_verb_press(id, world, shift, verb);
            return;
        }
        match self.node_grab(world) {
            Some(NodeGrab::Handle(at, side)) => {
                self.drag = Drag::Handle { id, at, side };
            }
            // `Ctrl`+drag pulls handles out of the anchor, mirrored unless `Alt`.
            Some(NodeGrab::Anchor(at)) if ctrl => {
                self.points.set_one(id, at);
                self.drag = Drag::Handle {
                    id,
                    at,
                    side: Side::Out,
                };
            }
            // No `Alt`+click removal: a destructive click is one slipped modifier away.
            Some(NodeGrab::Anchor(at)) if shift => self.points.toggle(id, at),
            Some(NodeGrab::Anchor(at)) => {
                // An already-selected anchor keeps the set: a multi-point drag can
                // start from any member.
                if !self.points.contains(id, at) {
                    self.points.set_one(id, at);
                }
                self.drag = Drag::Points {
                    id,
                    grab: world,
                    anchor: at,
                };
            }
            // Captured at press: a per-frame read would be the box being moved.
            Some(NodeGrab::Box(handle)) => {
                if let Some(box_) = self.point_box(id) {
                    self.drag = Drag::PointBox {
                        id,
                        handle,
                        box_,
                        grab: world,
                    };
                }
            }
            Some(NodeGrab::BoxRotate) => {
                if let Some(box_) = self.point_box(id) {
                    let pivot = box_.center();
                    let v = world - pivot;
                    self.drag = Drag::PointRotate {
                        id,
                        pivot,
                        start: v.y.atan2(v.x),
                    };
                }
            }
            // Shift extends the segment selection without arming a drag.
            Some(NodeGrab::Segment(at, _)) if shift => self.points.toggle_segment(id, at),
            Some(NodeGrab::Segment(at, t)) => {
                if !self.points.contains_segment(id, at) {
                    self.points.set_segment(id, at);
                }
                self.drag = Drag::Segment {
                    id,
                    at,
                    t,
                    grab: world,
                };
            }
            // Nothing grabbed: another path under the pointer becomes the subject.
            None if self
                .pick_leaf(world)
                .is_some_and(|p| p != id && self.is_path(p)) =>
            {
                if let Some(other) = self.pick_leaf(world) {
                    self.session.selection.set_one(other);
                }
                self.points.clear();
            }
            // Otherwise a marquee over points (`node_input` interprets it).
            None => {
                self.drag = Drag::Marquee {
                    origin: world,
                    additive: shift,
                };
            }
        }
    }

    /// Select every anchor inside the marquee, on release. Anchors only: a
    /// handle belongs to its point.
    fn marquee_points(&mut self, origin: Point, end: Point, additive: bool) {
        let Some(id) = self.edited_path() else { return };
        let Some((_, subpaths)) = self.edited_subpaths(id) else {
            return;
        };
        let area = KRect::new(
            origin.x.min(end.x),
            origin.y.min(end.y),
            origin.x.max(end.x),
            origin.y.max(end.y),
        );
        if !additive {
            self.points.clear();
        }
        for (s, sub) in subpaths.iter().enumerate() {
            for (a, an) in sub.anchors.iter().enumerate() {
                if area.contains(an.at) && !self.points.contains(id, (s, a)) {
                    self.points.toggle(id, (s, a));
                }
            }
        }
    }

    /// Whether `id` is a `Path` — the node tool's only subject.
    fn is_path(&self, id: NodeId) -> bool {
        matches!(
            self.session.doc.get(id).map(|n| n.kind()),
            Some(NodeKind::Path { .. })
        )
    }

    /// The anchors the node tool is acting on: the selected set, or for a selected
    /// segment the two anchors it runs between. Delete alone reads the segment
    /// itself: removing a segment leaves a gap, removing its points heals.
    pub(crate) fn subject_points(&self, id: NodeId) -> Vec<PointRef> {
        let segments: Vec<PointRef> = self.points.segments(id).collect();
        if segments.is_empty() {
            return self.points.on(id).collect();
        }
        let Some((_, subpaths)) = self.edited_subpaths(id) else {
            return Vec::new();
        };
        // Two selected segments that meet share an anchor, which must not move twice.
        let mut ends: Vec<PointRef> = segments
            .iter()
            .filter_map(|at| tools::segment_ends(&subpaths, *at))
            .flat_map(|(a, b)| [a, b])
            .collect();
        ends.sort_unstable();
        ends.dedup();
        ends
    }

    /// The selected anchors of `id`, in world space — what the inspector's X/Y
    /// reads and what align counts.
    pub(crate) fn selected_point_positions(&self, id: NodeId) -> Vec<Point> {
        let Some((_, subpaths)) = self.preview_subpaths(id) else {
            return Vec::new();
        };
        self.subject_points(id)
            .into_iter()
            .filter_map(|(s, a)| Some(subpaths.get(s)?.anchors.get(a)?.at))
            .collect()
    }

    /// The one selected anchor and its control point on `side`, in world space —
    /// what the inspector's handle fields read, from the preview. Exactly one
    /// anchor, or nothing (the fields dim).
    pub(crate) fn selected_handle(&self, id: NodeId, side: Side) -> Option<(PointRef, Point)> {
        let [at] = *self.subject_points(id) else {
            return None;
        };
        let (_, subpaths) = self.preview_subpaths(id)?;
        let an = subpaths.get(at.0)?.anchors.get(at.1)?;
        Some((at, an.handle(side)))
    }

    /// Move one control point by `delta` from where the **preview** has it — the
    /// handle counterpart of `nudge_points_tx`. The mirror is preserved per
    /// [`PenAnchor::is_smooth`], since a field has no `Alt`; no retract or tangent snap.
    pub(crate) fn nudge_handle_tx(
        &self,
        id: NodeId,
        at: PointRef,
        side: Side,
        delta: Vec2,
    ) -> Transaction {
        let Some((world, mut subpaths)) = self.preview_subpaths(id) else {
            return Transaction::default();
        };
        let Some(an) = subpaths.get(at.0).and_then(|s| s.anchors.get(at.1)) else {
            return Transaction::default();
        };
        let (to, break_pair) = (an.handle(side) + delta, !an.is_smooth());
        if let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) {
            an.pull_side(side, to, break_pair);
        }
        tools::rewrite_path(id, world, &subpaths, self.stored_path(id))
    }

    /// Align the selected points: the align row's verb, redirected from layers to
    /// points while the node tool is up. One commit.
    pub(crate) fn align_points(&mut self, axis: build::Axis, edge: build::Edge) {
        self.edit_points(Some("Aligned"), None, |subpaths, points| {
            tools::align_points(subpaths, points, axis, edge)
        });
    }

    /// Distribute the selected points along `axis`, as [`Self::align_points`] aligns them.
    pub(crate) fn distribute_points(&mut self, axis: build::Axis) {
        self.edit_points(Some("Distributed"), None, |subpaths, points| {
            tools::distribute_points(subpaths, points, axis)
        });
    }

    /// *Make corner* / *Make smooth* over the point selection
    /// (`docs/context-menus.md` §6.5), as one commit; the rule is
    /// `tools::set_smoothness`.
    pub(crate) fn set_point_smoothness(&mut self, smooth: bool) {
        let verb = if smooth { "Smoothed" } else { "Cornered" };
        self.edit_points(Some(verb), None, |subpaths, points| {
            tools::set_smoothness(subpaths, points, smooth)
        });
    }

    /// Step the point selection one anchor along the path — `Tab` and
    /// `Shift`+`Tab` (`app::dispatch`): one flat wrapping order over the whole
    /// path, from the subject's last point. Silent; reveals where it lands.
    pub(crate) fn step_point(&mut self, forward: bool) {
        let Some(id) = self.edited_path() else { return };
        let Some((_, subpaths)) = self.edited_subpaths(id) else {
            return;
        };
        let order: Vec<PointRef> = subpaths
            .iter()
            .enumerate()
            .flat_map(|(s, sub)| (0..sub.anchors.len()).map(move |a| (s, a)))
            .collect();
        if order.is_empty() {
            return;
        }
        let from = self
            .subject_points(id)
            .last()
            .and_then(|at| order.iter().position(|o| o == at));
        let next = match (from, forward) {
            (Some(i), true) => (i + 1) % order.len(),
            (Some(i), false) => (i + order.len() - 1) % order.len(),
            (None, true) => 0,
            (None, false) => order.len() - 1,
        };
        self.points.set_one(id, order[next]);
        self.reveal_selection();
    }

    /// Arrow-key nudge of the selected points (`app::dispatch`). Silent, so a held
    /// key writes no status line per repeat.
    pub(crate) fn nudge_points(&mut self, delta: Vec2) {
        self.edit_points(None, Some("nudge-points"), |subpaths, points| {
            tools::move_points(subpaths, points, delta)
        });
        self.reveal_selection();
    }

    /// Run `edit` over the edited path's world-space subpaths and commit the
    /// result, saying `done` if anything changed.
    ///
    /// `run` names the verb when consecutive calls should undo as one step (an
    /// arrow-key nudge; `EditorSession::commit_run`).
    fn edit_points(
        &mut self,
        done: Option<&str>,
        run: Option<&'static str>,
        edit: impl FnOnce(&mut [PenSubpath], &[PointRef]),
    ) {
        let Some(id) = self.edited_path() else { return };
        let Some((world, mut subpaths)) = self.edited_subpaths(id) else {
            return;
        };
        let points: Vec<PointRef> = self.subject_points(id);
        edit(&mut subpaths, &points);
        let tx = tools::rewrite_path(id, world, &subpaths, self.stored_path(id));
        if tx.0.is_empty() {
            return;
        }
        let applied = match run {
            Some(verb) => self.session.commit_run(tx, verb),
            None => self.session.commit(tx),
        };
        if applied && let Some(done) = done {
            self.session.info(done);
        }
    }

    /// The transaction one numeric-spacing edit produces for the point subject;
    /// absolute, so it reads the committed subpaths.
    pub(crate) fn point_spacing_tx(&self, axis: build::Axis, gap: f64) -> Transaction {
        let Some(id) = self.edited_path() else {
            return Transaction::default();
        };
        let Some((world, mut subpaths)) = self.edited_subpaths(id) else {
            return Transaction::default();
        };
        let points: Vec<_> = self.subject_points(id);
        tools::space_points(&mut subpaths, &points, axis, gap);
        tools::rewrite_path(id, world, &subpaths, self.stored_path(id))
    }

    /// The gaps the spacing field shows, from [`Self::preview_subpaths`], or a
    /// field's own drag could not move it.
    pub(crate) fn point_spacings(&self, axis: build::Axis) -> Vec<f64> {
        let Some(id) = self.edited_path() else {
            return Vec::new();
        };
        let Some((_, subpaths)) = self.preview_subpaths(id) else {
            return Vec::new();
        };
        tools::point_spacings(&subpaths, &self.subject_points(id), axis)
    }

    /// Move every selected point of `id` by a drag's total `delta`, from the
    /// committed path. Empty when nothing changed, which keeps a click off the
    /// undo stack.
    pub(crate) fn points_tx(&self, id: NodeId, delta: Vec2) -> Transaction {
        let Some((world, mut subpaths)) = self.edited_subpaths(id) else {
            return Transaction::default();
        };
        let points: Vec<_> = self.subject_points(id);
        tools::move_points(&mut subpaths, &points, delta);
        tools::rewrite_path(id, world, &subpaths, self.stored_path(id))
    }

    /// The corner radius the radius field should show for `id`, and whether any
    /// anchor it would write to is a corner at all (`None` dims the field). The
    /// largest, so adding a corner never makes the field jump down.
    pub(crate) fn corner_radius_of(&self, id: NodeId, points: Option<&[PointRef]>) -> Option<f64> {
        let (_, subpaths) = self.preview_subpaths(id)?;
        let mut any = false;
        let mut most: f64 = 0.0;
        for (s, sub) in subpaths.iter().enumerate() {
            for (a, an) in sub.anchors.iter().enumerate() {
                if points.is_some_and(|p| !p.contains(&(s, a))) || !an.is_corner() {
                    continue;
                }
                any = true;
                most = most.max(an.radius);
            }
        }
        any.then_some(most)
    }

    /// Set the corner radius on `id` — the selected points, or every corner when
    /// `points` is `None`. Corner anchors only: one with live handles has no
    /// corner to cut. Measured from the preview, because a field scrubs against it.
    pub(crate) fn corner_radius_tx(
        &self,
        id: NodeId,
        points: Option<&[PointRef]>,
        radius: f64,
    ) -> Transaction {
        let Some((world, mut subpaths)) = self.preview_subpaths(id) else {
            return Transaction::default();
        };
        for (s, sub) in subpaths.iter_mut().enumerate() {
            for (a, an) in sub.anchors.iter_mut().enumerate() {
                if points.is_some_and(|p| !p.contains(&(s, a))) || !an.is_corner() {
                    continue;
                }
                an.radius = radius.max(0.0);
            }
        }
        tools::rewrite_path(id, world, &subpaths, self.stored_path(id))
    }

    /// Move the selected points by `delta` from where the preview has them. The
    /// counterpart to `points_tx`: a drag hands over its total since the press
    /// (applied to the committed path), a scrubbed field one frame's increment
    /// (applied to the preview, or it never accumulates).
    pub(crate) fn nudge_points_tx(&self, id: NodeId, delta: Vec2) -> Transaction {
        let Some((world, mut subpaths)) = self.preview_subpaths(id) else {
            return Transaction::default();
        };
        let points: Vec<_> = self.subject_points(id);
        tools::move_points(&mut subpaths, &points, delta);
        tools::rewrite_path(id, world, &subpaths, self.stored_path(id))
    }

    /// The delta a node-tool drag should apply, and the lines explaining it. The
    /// aimed point `base` snaps, not the box round the set; Shift constrains as in
    /// [`Self::snapped_move`], and the path is excluded from the targets since its
    /// bounds move with it.
    fn snapped_point(
        &self,
        id: NodeId,
        base: Option<Point>,
        raw: Vec2,
        shift: bool,
    ) -> (Vec2, Vec<snap::Guide>) {
        if !self.gesture_travelled(raw) {
            return (raw, Vec::new());
        }
        let axes = snap::move_axes(raw, shift);
        let delta = snap::constrain(raw, axes);
        let Some(base) = base else {
            return (delta, Vec::new());
        };
        let want = base + delta;
        let result = snap::snap_point(want, axes, &self.snapping(&[id]));
        (delta + (result.at - want), result.guides)
    }

    /// Whether a gesture has travelled far enough to be one — the gate on every
    /// snap in the node tool, since a snap on an unmoved click would edit the
    /// document and `rewrite_path` cannot catch it. The threshold is [`MIN_CREATE_PX`].
    fn gesture_travelled(&self, raw: Vec2) -> bool {
        raw.hypot() * self.points_per_world() >= MIN_CREATE_PX
    }

    /// Where the anchor `at` of `id` sits in the **committed** document — the
    /// origin a canvas drag's total delta is measured from.
    fn anchor_position(&self, id: NodeId, at: PointRef) -> Option<Point> {
        let (_, subpaths) = self.edited_subpaths(id)?;
        Some(subpaths.get(at.0)?.anchors.get(at.1)?.at)
    }

    /// The pointer a point-box scale should use, and the lines explaining it. Only
    /// the handle's own axes ([`Self::resize_axes`]) snap — a side band must not
    /// draw a guide for the axis it cannot move — behind [`Self::snapped_point`]'s
    /// travel guard.
    fn snapped_point_box(
        &self,
        id: NodeId,
        handle: Handle,
        grab: Point,
        world: Point,
    ) -> (Point, Vec<snap::Guide>) {
        if !self.gesture_travelled(world - grab) {
            return (world, Vec::new());
        }
        let axes = self.resize_axes(Target::Layer(id), handle);
        let snapped = snap::snap_point(world, axes, &self.snapping(&[id]));
        (snapped.at, snapped.guides)
    }

    /// The point on a segment the press grabbed, committed-side.
    fn segment_position(&self, id: NodeId, at: PointRef, t: f64) -> Option<Point> {
        let (_, subpaths) = self.edited_subpaths(id)?;
        tools::segment_point(&subpaths, at, t)
    }

    /// Move or bend the segment leaving `at`; `delta` is the total travel since
    /// the press. One function because `Ctrl` switches verbs mid-drag.
    fn segment_tx(&self, id: NodeId, at: PointRef, t: f64, delta: Vec2, bend: bool) -> Transaction {
        let Some((world, mut subpaths)) = self.edited_subpaths(id) else {
            return Transaction::default();
        };
        if bend {
            tools::bend_segment(&mut subpaths, at, t, delta);
        } else {
            tools::move_points(&mut subpaths, &self.subject_points(id), delta);
        }
        tools::rewrite_path(id, world, &subpaths, self.stored_path(id))
    }

    /// Scale the selected points about the box the press captured: Shift holds the
    /// ratio, Alt the centre; always geometry scaling.
    fn point_box_tx(
        &self,
        id: NodeId,
        handle: Handle,
        box_: KRect,
        world: Point,
        shift: bool,
        alt: bool,
    ) -> Transaction {
        let Some((wt, mut subpaths)) = self.edited_subpaths(id) else {
            return Transaction::default();
        };
        let points: Vec<PointRef> = self.subject_points(id);
        tools::resize_points(
            &mut subpaths,
            &points,
            box_,
            handle,
            world,
            tools::Resize::geometry(shift, alt),
        );
        tools::rewrite_path(id, wt, &subpaths, self.stored_path(id))
    }

    /// Turn the selected points about the box's centre. Shift snaps to
    /// `ROTATE_SNAP` steps; otherwise unsnapped.
    fn point_rotate_tx(
        &self,
        id: NodeId,
        pivot: Point,
        start: f64,
        world: Point,
        shift: bool,
    ) -> Transaction {
        let Some((wt, mut subpaths)) = self.edited_subpaths(id) else {
            return Transaction::default();
        };
        let v = world - pivot;
        let mut angle = v.y.atan2(v.x) - start;
        if shift {
            angle = (angle / ROTATE_SNAP).round() * ROTATE_SNAP;
        }
        tools::rotate_points(&mut subpaths, &self.subject_points(id), pivot, angle);
        tools::rewrite_path(id, wt, &subpaths, self.stored_path(id))
    }

    /// Insert an anchor into the segment leaving `at`, at parameter `t`, and select
    /// it — the insert renumbers the subpath, so the old set would be stale.
    fn insert_point(&mut self, id: NodeId, at: PointRef, t: f64) {
        let Some((world, mut subpaths)) = self.edited_subpaths(id) else {
            return;
        };
        let Some(new) = tools::insert_point(&mut subpaths, at, t) else {
            return;
        };
        let tx = tools::rewrite_path(id, world, &subpaths, self.stored_path(id));
        if self.session.commit(tx) {
            self.points.set_one(id, new);
            self.session.info("Point added");
        }
    }

    /// The segment under `world` and the parameter along it, or `None` when the
    /// pointer is on a point, the box over them, or nothing of this path. The one
    /// query behind *Add point here* (`context-menus.md` §6.5), wrapping
    /// [`Self::node_grab`] so the row means what a click there would do.
    pub(crate) fn segment_at(&self, world: Point) -> Option<(PointRef, f64)> {
        match self.node_grab(world)? {
            NodeGrab::Segment(at, t) => Some((at, t)),
            _ => None,
        }
    }

    /// Insert an anchor at `world` on the edited path, for the menu row that has a
    /// position where the keyboard has none.
    pub(crate) fn add_point_at(&mut self, world: Point) {
        let Some(id) = self.edited_path() else { return };
        let Some((at, t)) = self.segment_at(world) else {
            return;
        };
        self.insert_point(id, at, t);
    }

    /// The path and the two open ends *Join* would fuse — the row's gate and the
    /// verb's own subject, asked once (`tools::joinable_ends`).
    pub(crate) fn join_subject(&self) -> Option<(NodeId, PointRef, PointRef)> {
        let id = self.edited_path()?;
        let (_, subpaths) = self.edited_subpaths(id)?;
        let points: Vec<PointRef> = self.points.on(id).collect();
        let (a, b) = tools::joinable_ends(&subpaths, &points)?;
        Some((id, a, b))
    }

    /// Join the two selected open ends — `context-menus.md` §6.5's last row, the
    /// selection-driven twin of the pen's join. The new segment becomes the
    /// selection, since the edit renumbers the run.
    pub(crate) fn join_points(&mut self) {
        let Some((id, a, b)) = self.join_subject() else {
            return;
        };
        let Some((world, subpaths)) = self.edited_subpaths(id) else {
            return;
        };
        let Some((list, seam)) = tools::join_ends(&subpaths, a, b) else {
            return;
        };
        let tx = tools::rewrite_path(id, world, &list, self.stored_path(id));
        if !tx.0.is_empty() && self.session.commit(tx) {
            self.points.set_segment(id, seam);
            self.session.info(match a.0 == b.0 {
                true => "Closed this subpath",
                false => "Joined two subpaths of this path",
            });
        }
    }

    /// Remove the selected segments, and with them the runs' closure or, on an
    /// open path, their continuity — the counterpart to Delete's healing removal
    /// of a *point* (`delete_points`).
    fn remove_segments(&mut self, id: NodeId, cuts: &[PointRef]) {
        let Some((world, subpaths)) = self.edited_subpaths(id) else {
            return;
        };
        let left = tools::remove_segments(&subpaths, cuts);
        // Nothing left to draw: delete the node rather than keep an empty layer.
        if left.is_empty() {
            self.points.clear();
            self.delete_selection();
            return;
        }
        let tx = tools::rewrite_path(id, world, &left, self.stored_path(id));
        if !tx.0.is_empty() && self.session.commit(tx) {
            // The cut renumbers, so the set is dropped rather than remapped.
            self.points.clear();
            self.session.info("Segment removed");
        }
    }

    fn handle_tx(&self, id: NodeId, at: PointRef, side: Side, to: Point, alt: bool) -> Transaction {
        let Some((world, mut subpaths)) = self.edited_subpaths(id) else {
            return Transaction::default();
        };
        let zoom = self.points_per_world();
        let snap = f64::from(PEN_PICK_PX) / zoom;
        tools::pull_handle(
            &mut subpaths,
            at,
            side,
            to,
            alt,
            snap,
            f64::from(TANGENT_SNAP_PX) / zoom,
        );
        tools::rewrite_path(id, world, &subpaths, self.stored_path(id))
    }

    /// The **committed** outline and radii of `id`, for `rewrite_path`'s no-op
    /// comparison — both, so a radius scrubbed away and back is no undo step.
    fn stored_path(&self, id: NodeId) -> Option<(&ondin_core::kurbo::BezPath, &[f64])> {
        match self.session.doc.get(id)?.kind() {
            NodeKind::Path { path, corner_radii } => Some((path, corner_radii)),
            _ => None,
        }
    }

    /// Remove the selected points, healing each subpath — the node tool's Delete,
    /// intercepted in `app::dispatch` or `Action::Delete` would delete the layer.
    pub(crate) fn delete_points(&mut self) {
        let Some(id) = self.edited_path() else { return };
        // `PointSet` holds points or segments, never both.
        let cuts: Vec<PointRef> = self.points.segments(id).collect();
        if !cuts.is_empty() {
            self.remove_segments(id, &cuts);
            return;
        }
        let Some((world, subpaths)) = self.edited_subpaths(id) else {
            return;
        };
        let points: Vec<_> = self.points.on(id).collect();
        if points.is_empty() {
            return;
        }
        let left = tools::remove_points(&subpaths, &points);
        // Nothing left to draw: delete the node rather than keep an empty layer.
        if left.is_empty() {
            self.points.clear();
            self.delete_selection();
            return;
        }
        let tx = tools::rewrite_path(id, world, &left, None);
        if self.session.commit(tx) {
            // Every later index has shifted; a partial set carried across would be wrong.
            self.points.clear();
        }
    }

    /// Turn a path's subpaths round — `Shift+D`, the node tool's direction verb.
    /// The subject is each run with a selected point, or every run when none is,
    /// and a selected path works outside point editing too. The point selection is
    /// remapped (`PointSet::reverse_within`), since reversing is a permutation.
    pub(crate) fn reverse_subpaths(&mut self) {
        let Some(id) = self.edited_path().or_else(|| {
            let [id] = *self.session.selection.ids() else {
                return None;
            };
            // The lock check `edited_path` carries; `is_path` is a kind predicate.
            if ondin_core::is_effectively_locked(&self.session.doc, id) {
                return None;
            }
            self.is_path(id).then_some(id)
        }) else {
            return;
        };
        let Some((world, mut subpaths)) = self.edited_subpaths(id) else {
            return;
        };
        let picked = tools::subpaths_touched(&self.subject_points(id));
        let which: Vec<usize> = match picked.is_empty() {
            true => (0..subpaths.len()).collect(),
            false => picked,
        };
        tools::reverse_subpaths(&mut subpaths, &which);
        let tx = tools::rewrite_path(id, world, &subpaths, self.stored_path(id));
        if tx.0.is_empty() || !self.session.commit(tx) {
            return;
        }
        for s in &which {
            if let Some(sub) = subpaths.get(*s) {
                self.points
                    .reverse_within(id, *s, sub.anchors.len(), sub.closed);
            }
        }
        self.session.info(match which.len() {
            1 => "Subpath reversed",
            _ => "Path direction reversed",
        });
    }

    /// Take back the last anchor placed, and cancel the path when the first one
    /// goes — the pen's Backspace (`app::dispatch`). Undo cannot: nothing is
    /// committed until the path is finished.
    pub(crate) fn pen_backspace(&mut self) {
        let Some(pen) = self.pen.as_mut() else { return };
        pen.anchors.pop();
        pen.shaping = false;
        if pen.anchors.is_empty() {
            self.pen = None;
            self.session.info("Pen cancelled");
        }
    }

    pub(crate) fn finish_pen(&mut self, close: bool) {
        let Some(pen) = self.pen.take() else { return };
        let resumed = pen.editing.is_some();
        let (id, tx, done) = match pen.editing {
            Some(mut edit) => {
                let mut sub = crate::preview::PenSubpath {
                    anchors: pen.anchors,
                    closed: close,
                };
                // Back into the direction it was read in (`PenEdit::subpaths`).
                if edit.from_start {
                    sub.reverse();
                }
                // A minted run left too short goes, not a lone `MoveTo` (`PenEdit::minted`).
                if edit.minted && sub.anchors.len() < 2 {
                    edit.subpaths.remove(edit.active);
                } else {
                    edit.subpaths[edit.active] = sub;
                }
                let current = self.stored_path(edit.id);
                let tx = tools::rewrite_path(edit.id, edit.world, &edit.subpaths, current);
                (edit.id, tx, "Extended path")
            }
            None => {
                let id = self.session.ids.mint();
                let index = self.child_count(pen.parent);
                let tx = tools::create_path(
                    id,
                    pen.parent,
                    index,
                    pen.parent_world,
                    &pen.anchors,
                    close,
                    PEN_STROKE_COLOR,
                    PEN_STROKE_WIDTH,
                );
                (id, tx, "Created path")
            }
        };
        if tx.0.is_empty() {
            // A too-short new path leaves the pen armed; a resumed one ends its gesture.
            if resumed {
                self.end_pen_gesture(id);
            }
            return;
        }
        if self.session.commit(tx) {
            self.end_pen_gesture(id);
            self.session.info(done);
        }
    }

    /// Where a finished pen gesture leaves you: holding `id`; the node tool stays
    /// in its edit, the standalone pen drops to Select. The point set goes either
    /// way, since the run was renumbered.
    fn end_pen_gesture(&mut self, id: NodeId) {
        self.session.selection.set_one(id);
        if self.tool == Tool::Node {
            self.points.clear();
            return;
        }
        self.choose_tool(Tool::Select);
    }

    // --- text -------------------------------------------------------------

    /// Open the editor on a text node that has **just been created** — the Text
    /// tool's committed drag, and [`Self::begin_text`]'s second half. The original
    /// is empty, or closing would commit a second undo step; the parts come off
    /// the node, so the drag's `Fixed` sizing is measured.
    fn open_text_session(&mut self, id: NodeId) {
        let Some(parts) = self
            .session
            .doc
            .get(id)
            .map(|n| n.kind())
            .and_then(TextParts::of)
        else {
            return;
        };
        self.text = Some(TextSession {
            id,
            editor: TextEdit::new("", parts),
            original: (String::new(), CharSpans::default(), ParaSpans::default()),
            created: true,
            blink: Default::default(),
        });
        self.mode = Mode::TextInsert;
        self.session.info("Type to edit; Esc to finish");
    }

    /// Create an empty **auto-width** text node and enter insert mode — the Text
    /// tool's *click*. Node creation is its own undo step (§9.3). The drag goes
    /// through `create_text` + [`Self::open_text_session`] with `Fixed` sizing.
    fn begin_text(&mut self, world: Point) {
        let (parent, parent_world) = self.draw_target(world);
        let local = parent_world.inverse() * world;
        let id = self.session.ids.mint();
        let index = self.child_count(parent);
        let parts = crate::fonts::FontService::default_text_parts();
        let tx = Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Text {
                content: String::new(),
                style: Box::new(parts.style.clone()),
                spans: parts.spans.clone(),
                para_spans: parts.para_spans.clone(),
                paragraph: parts.paragraph.clone(),
                block: parts.block,
                sizing: parts.sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate(local.to_vec2())),
            name: None,
        }]);
        if self.session.commit(tx) {
            self.session.selection.set_one(id);
            self.open_text_session(id);
        }
    }

    /// Turn the shape under the pointer into a rail and start typing along it —
    /// the Text tool's click on an outline. It consumes the layer, so the status
    /// line says so; one transaction, one undo step.
    fn begin_text_on_path(&mut self, path: NodeId, world: Point) {
        let parts = crate::fonts::FontService::default_text_parts();
        match build::text_on_new_path(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            path,
            &parts,
            world,
        ) {
            Ok((tx, id)) => {
                if self.session.commit(tx) {
                    self.session.selection.set_one(id);
                    self.open_text_session(id);
                    self.session
                        .info("Type along the path; the shape became the rail — Ctrl+Z to undo");
                }
            }
            Err(e) => self.session.fail(format!("Cannot set on path: {e}")),
        }
    }

    /// Place the front image of the loaded cursor, centred on `world`, fitted to
    /// the frame under the point or else the viewport.
    fn place_image_at(&mut self, world: Point) {
        let Some(image) = self.pending_images.first().cloned() else {
            return;
        };
        if self.create_image_at(&image, world) {
            self.placed_one_image();
        }
    }

    /// Where the visible world is centred — what a **paste** places at.
    pub(crate) fn viewport_centre(&self) -> Point {
        self.session.camera.viewport(self.canvas_px).view.center()
    }

    /// Where a drop — or a drag still hovering — is aimed, in world units.
    ///
    /// On Windows winit's drop handler carries no position, so
    /// [`os_cursor_screen_pos`] asks the OS — first, because inside a file drag
    /// egui's own pointer is absent or stale.
    pub(crate) fn drop_point(&self, ctx: &egui::Context) -> Option<Point> {
        // `latest_pos` too: a drop's release is not an egui interaction.
        os_cursor_screen_pos(ctx)
            .or_else(|| ctx.pointer_interact_pos())
            .or_else(|| ctx.pointer_latest_pos())
            .filter(|p| self.canvas_rect.contains(*p))
            .map(|p| self.to_world(p, self.canvas_rect, ctx.pixels_per_point()))
    }

    /// The fill a drop or a paste would replace, or `None` to make a new layer.
    ///
    /// A *selected* shape under the point (§5.5a), so replacing is opt-in. `None`
    /// is the paste's arm; `drop_images_at` passes `Some(p)` or declines, else a
    /// second drop would replace the first picture. The index is the first visible
    /// image fill, else the last fill.
    pub(crate) fn image_drop_target(&self, world: Option<Point>) -> Option<(NodeId, usize)> {
        let [id] = *self.session.selection.ids() else {
            return None;
        };
        // `edited_image`'s lock guard never runs for a drop or paste.
        if ondin_core::is_effectively_locked(&self.session.doc, id) {
            return None;
        }
        if let Some(p) = world {
            let bounds = self.session.resolved.world_bounds(id)?;
            if !bounds.contains(p) {
                return None;
            }
        }
        let paint = self.session.doc.get(id)?.paint();
        let index = tools::croppable_fill(paint).or_else(|| paint.fills.len().checked_sub(1))?;
        Some((id, index))
    }

    /// The layer name a picture takes among `parent`'s children: a given name (a
    /// filename) is kept, else "Image n". Not `CreateNode { name: None }`, which
    /// would say "Rectangle n" since an image is a fill on a Rect (§5.5a).
    fn image_layer_name(&self, parent: NodeId, given: Option<String>) -> String {
        image_layer_name(&self.session.doc, parent, given)
    }

    /// Make a layer for `image` at `world`. Returns whether it landed. Takes the
    /// image, not `pending_images`, so a drop never touches the Image tool's queue
    /// (that stays with [`Self::place_image_at`]).
    pub(crate) fn create_image_at(&mut self, image: &LoadedImage, world: Point) -> bool {
        let (tx, node) = self.image_tx(None, image, world);
        if !self.session.commit(tx) {
            return false;
        }
        self.session.selection.set_one(node);
        true
    }

    /// The transaction half of [`Self::create_image_at`]: one picture's
    /// `CreateNode`, and its id. Split out so a multi-file drop is one undo step
    /// (`commit_run` cannot merge `CreateNode`s). `over` is the document to read
    /// structure from: a batch passes one advanced by the placements built so far.
    pub(crate) fn image_tx(
        &mut self,
        over: Option<&Document>,
        image: &LoadedImage,
        world: Point,
    ) -> (Transaction, NodeId) {
        let (id, entry, intrinsic, name) = (
            image.id.clone(),
            image.entry.clone(),
            image.intrinsic,
            image.name.clone(),
        );
        let (parent, parent_world) = self.draw_target(world);
        let frame = match self.session.resolved.used_kind(&self.session.doc, parent) {
            Some(NodeKind::Artboard { size, .. }) => *size,
            _ => {
                let view = self.session.camera.viewport(self.canvas_px).view;
                ondin_core::kurbo::Size::new(view.width(), view.height())
            }
        };
        let over = over.unwrap_or(&self.session.doc);
        let index = over.get(parent).map_or(0, |n| n.children().len());
        let name = image_layer_name(over, parent, name);
        let known = over.has_image(&id);
        let node = self.session.ids.mint();
        let tx = tools::create_image(
            node,
            parent,
            index,
            parent_world,
            world,
            frame,
            id.clone(),
            (!known).then_some(entry),
            intrinsic,
            Some(name),
        );
        (tx, node)
    }

    /// Make a text layer holding the clipboard's text and select it (not open it
    /// for editing) — `Ctrl+V` when the app's own clipboard has nothing. Returns
    /// whether there was any text, so [`OndinApp::paste`] can treat this as its
    /// last arm. Placement is [`Self::paste_placement`]'s.
    pub(crate) fn paste_text_as_layer(&mut self, at: Option<Point>) -> bool {
        let Some(text) = crate::app::system_clipboard_text() else {
            return false;
        };
        let (centre, parent, parent_world) = self.paste_placement(at);

        // Through a `TextEdit`: `insert` normalizes `\r\n` (a bare `TextEdit::new`
        // would double-space every line).
        let mut parts = crate::fonts::FontService::default_text_parts();
        let mut editor = TextEdit::new("", parts.clone());
        editor.insert(&text);
        let mut layout = editor.text_layout();

        // Wrapped to the frame's used width, or the view's.
        let available = match self.session.resolved.used_kind(&self.session.doc, parent) {
            Some(NodeKind::Artboard { size, .. }) => size.width,
            _ => self.session.camera.viewport(self.canvas_px).view.width(),
        };
        if let Some(sizing) = wrapped_paste_sizing(layout.size.width, available) {
            parts.sizing = sizing;
            editor.restyle(parts.clone());
            layout = editor.text_layout();
        }

        let local =
            centred_paste_origin(parent_world.inverse() * centre, layout.size, layout.origin);
        let id = self.session.ids.mint();
        let index = self.child_count(parent);
        let tx = Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Text {
                content: editor.content().to_string(),
                style: Box::new(parts.style.clone()),
                spans: parts.spans.clone(),
                para_spans: parts.para_spans.clone(),
                paragraph: parts.paragraph.clone(),
                block: parts.block,
                sizing: parts.sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate(local)),
            name: None,
        }]);
        if !self.session.commit(tx) {
            return false;
        }
        self.session.selection.set_one(id);
        self.session.info("Pasted text");
        true
    }

    /// Paste SVG markup as layers, the arm between the app's clipboard and the text
    /// fallback. Markup that will not parse falls through silently, except one nested
    /// too deeply, refused out loud rather than pasted as a huge text layer.
    pub(crate) fn paste_svg(&mut self, at: Option<Point>) -> bool {
        let Some(text) = crate::app::system_clipboard_text() else {
            return false;
        };
        if !ondin_core::svg_in::looks_like_svg(&text) {
            return false;
        }
        match self.import_svg(&text, at, None) {
            SvgImport::Placed(said) => {
                self.session.info(format!("Pasted SVG — {said}"));
                true
            }
            SvgImport::TooDeep => {
                self.session
                    .fail("That SVG is nested too deeply to read — nothing was pasted.");
                true
            }
            SvgImport::Unreadable => false,
        }
    }

    /// Build SVG markup's layers (`ondin_core::svg_in`) as one undo step, centred
    /// where [`Self::paste_placement`] puts `at`, and select them; `name` names the
    /// root group (a file's stem). Placed by applying to a clone, reading the bounds
    /// and correcting the root's transform. The status is the caller's to write.
    pub(crate) fn import_svg(
        &mut self,
        text: &str,
        at: Option<Point>,
        name: Option<&str>,
    ) -> SvgImport {
        let (centre, parent, parent_world) = self.paste_placement(at);

        let index = self.child_count(parent);
        // A sink, so the session's ids and the decode cache are borrowed at once.
        let mut sink = SvgImages {
            images: self.canvas.images_mut(),
        };
        let mut out = match ondin_core::svg_in::import(
            text,
            &mut self.session.ids,
            parent,
            index,
            Some(&mut sink),
        ) {
            Ok(out) => out,
            Err(ondin_core::svg_in::SvgError::TooDeep) => return SvgImport::TooDeep,
            Err(_) => return SvgImport::Unreadable,
        };
        if out.shapes == 0 {
            return SvgImport::Unreadable;
        }
        if let Some(name) = name
            && let Some(Operation::CreateNode { name: root, .. }) = out
                .tx
                .0
                .iter_mut()
                .find(|op| matches!(op, Operation::CreateNode { id, .. } if *id == out.root))
        {
            *root = Some(name.to_string());
        }

        let mut probe = self.session.doc.clone();
        if probe.apply(&out.tx).is_err() {
            return SvgImport::Unreadable;
        }
        if let Some(bounds) = ondin_core::Resolved::rebuild(&probe).world_bounds(out.root) {
            let inv = parent_world.inverse();
            let delta = (inv * centre) - (inv * bounds.center());
            if let Some(Operation::CreateNode { transform, .. }) = out
                .tx
                .0
                .iter_mut()
                .find(|op| matches!(op, Operation::CreateNode { id, .. } if *id == out.root))
            {
                *transform = Some(Affine::translate(delta) * transform.unwrap_or(Affine::IDENTITY));
            }
        }

        if !self.session.commit(out.tx) {
            return SvgImport::Unreadable;
        }
        self.session.selection.set_one(out.root);
        let n = out.shapes;
        let plural = if n == 1 { "layer" } else { "layers" };
        // Skipped is missing; approximated is there and slightly wrong.
        out.skipped.sort();
        out.approximated.sort();
        let mut msg = format!("{n} {plural}");
        if !out.skipped.is_empty() {
            msg.push_str(&format!(", skipped {}", out.skipped.join(", ")));
        }
        if !out.approximated.is_empty() {
            msg.push_str(&format!(", approximated {}", out.approximated.join(", ")));
        }
        SvgImport::Placed(msg)
    }

    /// The frame a paste is aimed at — the enclosing frame of the first selected
    /// node, so a frame and anything inside it agree — or `None` for a selection
    /// on the bare canvas or none.
    fn paste_frame(&self) -> Option<NodeId> {
        let id = *self.session.selection.ids().first()?;
        enclosing_artboard(&self.session.doc, id)
    }

    /// Where a text or SVG paste lands and whose child it is:
    /// `(centre, parent, parent_world)`: the pointer `at` where there is one; else
    /// the centre of [the selected frame](Self::paste_frame), parented to that
    /// frame rather than `draw_target`'s (possibly nested) answer; else the middle
    /// of the view. `app::paste_image` has no selected-frame arm.
    fn paste_placement(&self, at: Option<Point>) -> (Point, NodeId, Affine) {
        let frame = self.paste_frame();
        let centre = match (
            at,
            frame.and_then(|id| self.session.preview_world_bounds(id)),
        ) {
            (Some(p), _) => p,
            (None, Some(bounds)) => bounds.center(),
            (None, None) => self.viewport_centre(),
        };
        let (parent, parent_world) = match frame.filter(|_| at.is_none()) {
            Some(id) => (
                id,
                self.session
                    .preview_world_transform(id)
                    .unwrap_or(Affine::IDENTITY),
            ),
            None => self.draw_target(centre),
        };
        (centre, parent, parent_world)
    }

    /// One image of the loaded cursor is spent; **the tool disarms itself when the
    /// last one is** (§5.5a, §9.4).
    fn placed_one_image(&mut self) {
        if !self.pending_images.is_empty() {
            self.pending_images.remove(0);
        }
        match self.pending_images.len() {
            0 => {
                self.choose_tool(Tool::Select);
                self.session.info("Placed image");
            }
            1 => self.session.info("1 image left to place"),
            n => self.session.info(format!("{n} images left to place")),
        }
    }

    /// Enter insert mode for an existing text node, placing the caret at the
    /// clicked point, or selecting the whole string when there was no click
    /// (`Enter` means "retype this").
    pub(crate) fn begin_edit_text(&mut self, id: Option<NodeId>, world: Option<Point>) {
        let Some(id) = id else {
            self.session.selection.clear();
            return;
        };
        // A lock refuses, its group's included; the menu row and `Enter` both arrive here.
        if ondin_core::is_effectively_locked(&self.session.doc, id) {
            self.session
                .info("This layer is locked — unlock it to edit the text");
            return;
        }
        // The text as drawn, bound values resolved (`keep_text_bindings`).
        let Some(kind) = self.session.doc.resolved_kind(id) else {
            return;
        };
        let kind = &*kind;
        let Some(parts) = TextParts::of(kind) else {
            return;
        };
        let Some(content) = TextRef::of(kind).map(|r| r.content.to_owned()) else {
            return;
        };
        let original = (
            content.clone(),
            parts.spans.clone(),
            parts.para_spans.clone(),
        );
        let mut editor = TextEdit::new(content, parts);
        match world.and_then(|w| self.text_local(id, w)) {
            Some(local) => editor.click(local),
            None => editor.select_all(),
        }
        self.session.selection.set_one(id);
        self.text = Some(TextSession {
            id,
            editor,
            original,
            created: false,
            blink: Default::default(),
        });
        self.mode = Mode::TextInsert;
        self.session.info("Editing text; Esc to finish");
    }

    /// Map a world point into a text node's local space.
    fn text_local(&self, id: NodeId, world: Point) -> Option<Point> {
        Some(self.session.preview_world_transform(id)?.inverse() * world)
    }

    /// The text layer under `world`, for the Text tool's click and hover outline.
    /// `pick_leaf` filtered to text: a text node behind a rectangle is covered.
    fn text_layer_at(&self, world: Point) -> Option<NodeId> {
        let id = self.pick_leaf(world)?;
        matches!(
            self.session.display_node(id).map(|n| n.kind()),
            Some(NodeKind::Text { .. })
        )
        .then_some(id)
    }

    /// The layer whose **outline** is under `world`, for the Text tool's *type
    /// along this path* affordance. Unlike [`Self::pick_leaf`] it measures
    /// distance to the outline, so an unfilled circle counts; nearest wins. Which
    /// kinds qualify is `ondin_core::query::outline_at`'s rule; the tolerance is
    /// `PEN_PICK_PX`.
    fn rail_at(&self, world: Point) -> Option<NodeId> {
        ondin_core::query::outline_at(
            &self.session.resolved,
            &self.session.doc,
            world,
            f64::from(PEN_PICK_PX) / self.points_per_world(),
        )
    }

    /// Whether `screen` is inside the box of the text node being edited — the
    /// I-beam's question and `text_mode_input`'s test for whether a click ends the
    /// session. Against the local box, so a rotated layer answers for its own rectangle.
    fn over_edited_text(&self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> bool {
        let Some(id) = self.text.as_ref().map(|s| s.id) else {
            return false;
        };
        let world = self.to_world(screen, rect, ppp);
        let Some(local) = self.text_local(id, world) else {
            return false;
        };
        self.session
            .preview_local_box(id)
            .is_some_and(|b| b.contains(local))
    }

    /// Move the camera by a pan drag's delta, shared by `update_drag` and
    /// [`Self::text_mode_input`].
    fn pan_by_drag(&mut self, ui: &egui::Ui, resp: &egui::Response) {
        let ppp = ui.ctx().pixels_per_point();
        let d = resp.drag_delta();
        self.session
            .camera
            .pan_pixels(Vec2::new((d.x * ppp) as f64, (d.y * ppp) as f64));
    }

    fn text_mode_input(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let Some(id) = self.text.as_ref().map(|s| s.id) else {
            self.mode = Mode::Normal;
            return;
        };

        // A click inside the node moves the caret; outside, the session ends.
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let world = self.to_world(p, rect, ppp);
            if !self.over_edited_text(p, rect, ppp) {
                self.finish_text_edit();
                return;
            }
            if let Some(local) = self.text_local(id, world)
                && let Some(s) = self.text.as_mut()
            {
                if crate::ui::double_clicked(ui, resp) {
                    s.editor.select_word_at(local);
                } else {
                    s.editor.click(local);
                }
            }
        }

        // The middle button pans and only the primary selects; not Space, a character here.
        if resp.drag_started_by(egui::PointerButton::Middle) {
            self.drag = Drag::Pan;
        } else if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let world = self.to_world(p, rect, ppp);
            if let Some(local) = self.text_local(id, world)
                && let Some(s) = self.text.as_mut()
            {
                s.editor.click(local);
            }
            self.drag = Drag::TextSelect;
        }
        if resp.dragged() {
            match self.drag {
                Drag::Pan => self.pan_by_drag(ui, resp),
                Drag::TextSelect => {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let world = self.to_world(p, rect, ppp);
                        if let Some(local) = self.text_local(id, world)
                            && let Some(s) = self.text.as_mut()
                        {
                            s.editor.drag_to(local);
                        }
                    }
                }
                _ => {}
            }
        }
        if resp.drag_stopped() {
            self.drag = Drag::None;
        }

        // Nothing typed into a panel or under a modal reaches the text.
        // `egui_wants_keyboard_input` over-approximates (any focus), as
        // `input::resolve` does; a modal holds the keyboard even when a click on
        // its body has taken the focus away.
        let events = if ui.ctx().egui_wants_keyboard_input() || self.modal_is_up() {
            Vec::new()
        } else {
            ui.input(|i| i.events.clone())
        };
        // Clipboard events carry no modifiers, so the frame's are read: `!alt`
        // because `AltGr+C` arrives as `Event::Copy` (`docs/shortcuts.md` §L2),
        // `!shift` because `Ctrl+Shift+C` (centre paragraph) does too.
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        let mut changed = false;
        let mut finish = false;
        // A cut or copy's text and toast; the clipboard write is deferred past the
        // loop, the removal is not.
        let mut copied: Option<(String, &'static str)> = None;
        for ev in events {
            let Some(s) = self.text.as_mut() else { break };
            match ev {
                egui::Event::Text(t) => {
                    s.editor.insert(&t);
                    changed = true;
                }
                egui::Event::Paste(t) if !alt => {
                    s.editor.insert(&t);
                    changed = true;
                }
                egui::Event::Copy if !alt && !shift => {
                    copied = non_empty(s.editor.selected_text()).map(|t| (t, "Copied text"))
                }
                egui::Event::Cut if !alt && !shift => {
                    if let Some(text) = s.editor.cut() {
                        copied = Some((text, "Cut"));
                        changed = true;
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    let by_word = modifiers.ctrl || modifiers.mac_cmd;
                    match key {
                        egui::Key::Escape => finish = true,
                        egui::Key::Enter => {
                            s.editor.insert("\n");
                            changed = true;
                        }
                        egui::Key::Backspace => {
                            if by_word {
                                s.editor.delete_word_back();
                            } else {
                                s.editor.backspace();
                            }
                            changed = true;
                        }
                        egui::Key::Delete => {
                            s.editor.delete_forward();
                            changed = true;
                        }
                        // All four yield to `Alt` — see [`arrow_moves_caret`].
                        egui::Key::ArrowLeft if arrow_moves_caret(modifiers) => {
                            s.editor.move_left(modifiers.shift, by_word);
                        }
                        egui::Key::ArrowRight if arrow_moves_caret(modifiers) => {
                            s.editor.move_right(modifiers.shift, by_word);
                        }
                        egui::Key::ArrowUp if arrow_moves_caret(modifiers) => {
                            s.editor.move_up(modifiers.shift);
                        }
                        egui::Key::ArrowDown if arrow_moves_caret(modifiers) => {
                            s.editor.move_down(modifiers.shift);
                        }
                        egui::Key::Home => s.editor.move_line_start(modifiers.shift),
                        egui::Key::End => s.editor.move_line_end(modifiers.shift),
                        egui::Key::A if modifiers.command => s.editor.select_all(),
                        // `Tab`/`Shift`+`Tab` nest and un-nest list items, claimed even
                        // when nothing moves: `took_tab` in the app's `ui` hands focus
                        // back so egui's focus ring cannot deafen the session.
                        egui::Key::Tab if !modifiers.command && !modifiers.alt => {
                            let delta = if modifiers.shift { -1 } else { 1 };
                            changed |= s.editor.step_list_level(delta);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        if let Some((text, said)) = copied {
            self.put_text_on_clipboard(ui.ctx(), text, said);
        }
        if finish {
            self.finish_text_edit();
            return;
        }
        if changed {
            self.preview_session();
        }
        ui.ctx().request_repaint();
    }

    /// Copy the text session's selection — the menu's *Copy* row (§6.4).
    pub(crate) fn copy_text_selection(&mut self, ctx: &egui::Context) {
        let Some(text) = self
            .text
            .as_ref()
            .and_then(|s| non_empty(s.editor.selected_text()))
        else {
            return;
        };
        self.put_text_on_clipboard(ctx, text, "Copied text");
    }

    /// Cut the text session's selection — the menu's *Cut* row (§6.4).
    pub(crate) fn cut_text_selection(&mut self, ctx: &egui::Context) {
        let Some(text) = self.text.as_mut().and_then(|s| s.editor.cut()) else {
            return;
        };
        self.put_text_on_clipboard(ctx, text, "Cut");
        self.preview_session();
    }

    /// Insert the system clipboard's text at the caret — the menu's *Paste* row
    /// (§6.4). Reads the clipboard directly, since a menu row gets no
    /// `Event::Paste`.
    pub(crate) fn paste_text_selection(&mut self) {
        let Some(text) = crate::app::system_clipboard_text() else {
            return;
        };
        let Some(s) = self.text.as_mut() else { return };
        s.editor.insert(&text);
        self.preview_session();
    }

    /// Select the whole buffer — the menu's *Select all* row (§6.4).
    pub(crate) fn select_all_text(&mut self) {
        if let Some(s) = self.text.as_mut() {
            s.editor.select_all();
        }
    }

    /// Put copied or cut text on the **system** clipboard, and drop both in-app
    /// payloads, or a later `Ctrl+V` would paste layers the OS clipboard no
    /// longer describes.
    fn put_text_on_clipboard(&mut self, ctx: &egui::Context, text: String, said: &str) {
        ctx.copy_text(text);
        self.clipboard = None;
        self.guide_clipboard = None;
        self.session.info(said);
    }

    /// Close the editing session: one `SetText` for the whole session (§9.3),
    /// or remove the node if it was created and left empty.
    pub(crate) fn finish_text_edit(&mut self) {
        let Some(session) = self.text.take() else {
            return;
        };
        self.mode = Mode::Normal;
        self.session.clear_preview();

        if session.editor.content().is_empty() && session.created {
            // `created`, not an empty `original`: a pre-existing node can be empty.
            self.session
                .commit(Transaction(vec![Operation::DeleteNode { id: session.id }]));
            self.session.selection.clear();
            self.session.info("Removed empty text");
            return;
        }
        // Only on success, so a refused commit keeps its own message.
        if session.changed()
            && self.session.commit(Transaction(vec![Operation::SetText {
                id: session.id,
                content: session.editor.content().to_string(),
                spans: session.editor.spans().clone(),
                para_spans: session.editor.para_spans().clone(),
            }]))
        {
            self.session.info("Text updated");
        }
    }

    /// Re-open the live editor against a changed style, so restyling from the
    /// inspector mid-session does not strand the caret. The editor's own spans win
    /// (they may hold uncommitted styling); for a write that clears spans use
    /// [`Self::text_session_restyled_after`].
    pub(crate) fn text_session_restyled(&mut self) {
        self.restyle_session(None, None);
    }

    /// [`Self::text_session_restyled`] for a commit that carries span lists. Per
    /// scope, a `SetTextSpans` in the transaction installs that op's list (keyed
    /// to the editor's bytes); a scope it does not mention keeps the editor's.
    pub(crate) fn text_session_restyled_after(&mut self, tx: &Transaction) {
        let Some(id) = self.text.as_ref().map(|s| s.id) else {
            return;
        };
        let (spans, para_spans) = stated_spans(tx, id);
        self.restyle_session(spans, para_spans);
        // The commit cleared the gesture preview; safe only after a commit.
        self.preview_session();
    }

    /// [`Self::text_session_restyled`] with the two span lists **given** — how a
    /// cancelled field scrub gets its old values back, since a scrub over a
    /// session writes into the editor, not a preview (§9.3).
    pub(crate) fn restyle_session_with(&mut self, spans: CharSpans, para_spans: ParaSpans) {
        self.restyle_session(Some(spans), Some(para_spans));
        self.preview_session();
    }

    /// Re-open the live editor against the document's style, each span list from
    /// the argument where given, else from the editor: the body behind the three
    /// entry points above. The canvas refresh is the caller's: `valve_paragraph`
    /// reaches this mid-drag, where re-previewing would overwrite the preview.
    fn restyle_session(&mut self, spans: Option<CharSpans>, para_spans: Option<ParaSpans>) {
        let Some(id) = self.text.as_ref().map(|s| s.id) else {
            return;
        };
        // As drawn, bound values resolved, as `begin_edit_text` opened it.
        let Some(kind) = self.session.doc.resolved_kind(id) else {
            return;
        };
        let kind = &*kind;
        let Some(parts) = TextParts::of(kind) else {
            return;
        };
        let stored = match kind {
            NodeKind::Text {
                content,
                spans,
                para_spans,
                ..
            } => (content.clone(), spans.clone(), para_spans.clone()),
            _ => return,
        };
        if let Some(s) = self.text.as_mut() {
            let spans = spans.unwrap_or_else(|| s.editor.spans().clone());
            let para_spans = para_spans.unwrap_or_else(|| s.editor.para_spans().clone());
            s.editor.restyle(TextParts {
                spans,
                para_spans,
                ..parts
            });
            // The base moves with the document's content, not the editor's (which
            // may hold uncommitted text), or `changed()` answers yes after any
            // outside write.
            s.original = stored;
        }
    }

    /// Put the live session's own state on the canvas — the `SetText` every
    /// session-scoped write ends with.
    pub(crate) fn preview_session(&mut self) {
        let Some(s) = self.text.as_ref() else {
            return;
        };
        let tx = Transaction(vec![Operation::SetText {
            id: s.id,
            content: s.editor.content().to_string(),
            spans: s.editor.spans().clone(),
            para_spans: s.editor.para_spans().clone(),
        }]);
        // The session's own layout: no second shaping per keystroke.
        let shaped = Some((s.id, s.editor.text_layout()));
        self.session.set_session_preview(&tx, shaped);
    }

    // --- overlays ---------------------------------------------------------

    /// Whether the **selection's** chrome is suppressed this frame — outline,
    /// handles, size badge, pivot marker and key layer's trace: a path, image or
    /// pen gesture edited in place ([`Self::editing_in_place`]), or a value being
    /// edited in the inspector (`ChromeHold`). Never hides the pen's own overlay.
    /// [`Self::draw_grid_tracks`] asks the two halves separately.
    pub(crate) fn chrome_hidden(&self) -> bool {
        self.editing_in_place() || self.chrome_hold.holding()
    }

    /// The first two of [`Self::chrome_hidden`]'s reasons: a path or an image
    /// being edited on the canvas, the pen's included.
    pub(crate) fn editing_in_place(&self) -> bool {
        self.pen.is_some() || self.edited_path().is_some() || self.edited_image().is_some()
    }

    /// Outline whatever the pointer is over but has not selected: the exact shape,
    /// so it says what a click will pick; a container outlines every leaf it holds.
    fn draw_hover_outline(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        if !self.hover_chrome_live() {
            return;
        }
        if self.tool == Tool::Text {
            let Some(p) = resp.hover_pos() else { return };
            let Some(id) = self.text_layer_at(self.to_world(p, rect, ppp)) else {
                return;
            };
            let stroke = egui::Stroke::new(3.0, color::SELECT);
            self.stroke_outlines(painter, id, rect, ppp, stroke, self.outline_view(stroke));
            return;
        }
        let Some(target) = self.hover_target(ui, rect, ppp, resp) else {
            return;
        };

        // 3px, readable over any artwork; a component's in its own hue.
        let stroke = egui::Stroke::new(3.0, chrome_hue(&self.session.doc, target));
        self.stroke_outlines(
            painter,
            target,
            rect,
            ppp,
            stroke,
            self.outline_view(stroke),
        );
    }

    /// Whether the chrome that answers a *resting* pointer is live this frame —
    /// the hover outline, the key layer's trace, the measure overlay. Not during
    /// a gesture, a text session, or while [`Self::chrome_hidden`] holds.
    fn hover_chrome_live(&self) -> bool {
        self.mode == Mode::Normal && self.drag.is_none() && !self.chrome_hidden()
    }

    /// The layer a resting pointer points *at* — what a click would take and a
    /// measure would measure to. `None` when the tool does not select, the pointer
    /// is off the canvas or over the selection's handles, or the target is selected.
    fn hover_target(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) -> Option<NodeId> {
        if !self.selecting() {
            return None;
        }
        let p = resp.hover_pos()?;
        if let Some(h) = self.selection_handles(rect, ppp)
            && h.armed(p, false).is_some()
        {
            return None;
        }
        let world = self.to_world(p, rect, ppp);
        let leaf = self.pick_at_pointer(ui, p, world, rect, ppp)?;
        let target = self.pick_preview(leaf, ui);
        (!self.session.selection.contains(target)).then_some(target)
    }

    /// The node tool's chrome: every anchor of the edited path, and the handles
    /// of the selected ones, replacing the selection box. A square marks a corner,
    /// a circle a smooth point; handles only for selected anchors, as
    /// [`Self::node_grab`] offers.
    fn draw_points(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        let Some(id) = self.edited_path() else { return };
        // A pen gesture in flight owns this path's picture.
        if self.pen.is_some() {
            return;
        }
        let Some((_, subpaths)) = self.preview_subpaths(id) else {
            return;
        };

        // Segments first, under the points; `node_grab` decides the trace and the
        // anchor's hover fill, so the chrome offers what the press takes.
        let grab = (self.drag.is_none() && !self.gesture_cancelled)
            .then(|| resp.hover_pos())
            .flatten()
            .and_then(|p| self.node_grab(self.to_world(p, rect, ppp)));
        let hovered = match grab {
            Some(NodeGrab::Segment(at, t)) => Some((at, t)),
            _ => None,
        };
        let hovered_anchor = match grab {
            Some(NodeGrab::Anchor(at)) => Some(at),
            _ => None,
        };
        // A dragged segment stays marked after the pointer leaves it.
        let dragging = match self.drag {
            Drag::Segment { at, .. } => Some(at),
            _ => None,
        };
        let traced = hovered
            .map(|(at, _)| (at, egui::Stroke::new(2.0, color::SELECT_DIM)))
            .into_iter()
            .chain(
                self.points
                    .segments(id)
                    .chain(dragging)
                    .map(|at| (at, egui::Stroke::new(2.5, color::SELECT))),
            );
        for (at, stroke) in traced {
            let Some(seg) = tools::segment_cubic(&subpaths, at) else {
                continue;
            };
            let mut path = ondin_core::kurbo::BezPath::new();
            path.move_to(seg.p0);
            path.curve_to(seg.p1, seg.p2, seg.p3);
            for sub in self.flatten_to_screen(&path, rect, ppp) {
                painter.add(sub.stroked(stroke));
            }
        }

        // Under the markers, since an anchor wins the press.
        self.draw_point_box(painter, rect, ppp);

        let ink = egui::Stroke::new(POINT_STROKE_PX, color::SELECT);
        // A selected segment lights its two ends too (`subject_points`).
        let subject = self.subject_points(id);
        for (s, sub) in subpaths.iter().enumerate() {
            for (a, an) in sub.anchors.iter().enumerate() {
                let at = self.to_screen(an.at, rect, ppp);
                let selected = subject.contains(&(s, a));
                // Handles first, so a marker is never drawn over by its stalk.
                if selected {
                    for side in Side::BOTH {
                        let h = an.handle(side);
                        if (h - an.at).hypot() <= HANDLE_DRAWN {
                            continue;
                        }
                        let to = self.to_screen(h, rect, ppp);
                        painter.line_segment([at, to], egui::Stroke::new(1.0, color::SELECT_DIM));
                        painter.circle_filled(to, HANDLE_DOT_PX, color::SELECT);
                    }
                }
                let fill = if selected {
                    color::SELECT
                } else {
                    color::POINT
                };
                let weight = match hovered_anchor == Some((s, a)) {
                    true => POINT_STROKE_HOVER_PX,
                    false => POINT_STROKE_PX,
                };
                let outline = egui::Stroke::new(weight, ink.color);
                if an.is_corner() {
                    let r = egui::Rect::from_center_size(at, egui::Vec2::splat(POINT_PX));
                    painter.rect_filled(r, egui::CornerRadius::ZERO, fill);
                    painter.rect_stroke(
                        r,
                        egui::CornerRadius::ZERO,
                        outline,
                        egui::StrokeKind::Inside,
                    );
                } else {
                    painter.circle_filled(at, POINT_PX * 0.5, fill);
                    // A circle has no `StrokeKind`; the tessellator reaches `r + w`,
                    // so the radius shrinks by the whole extra weight, not half.
                    let radius = POINT_PX * 0.5 - (weight - POINT_STROKE_PX);
                    painter.circle_stroke(at, radius, outline);
                }
            }
        }

        // The point a double-click would add, where `insert_point` would split: a
        // split line gives a corner, a split curve a smooth point.
        if let Some((seg, t)) = hovered
            && let Some(p) = tools::segment_point(&subpaths, seg, t)
        {
            let at = self.to_screen(p, rect, ppp);
            let ghost = color::SELECT.gamma_multiply(0.5);
            if tools::segment_is_straight(&subpaths, seg) {
                painter.rect_filled(
                    egui::Rect::from_center_size(at, egui::Vec2::splat(POINT_PX)),
                    egui::CornerRadius::ZERO,
                    ghost,
                );
            } else {
                painter.circle_filled(at, POINT_PX * 0.5, ghost);
            }
        }
    }

    /// The box over the selected points, with the four corner squares that scale
    /// them; the side bands and rotate ring are grabbed but not drawn
    /// ([`Self::node_grab`]). Read live, while the gesture uses the press's box.
    fn draw_point_box(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let Some(id) = self.edited_path() else { return };
        let Some(box_) = self.point_box(id) else {
            return;
        };
        let quad: Vec<egui::Pos2> = Handle::CORNERS
            .into_iter()
            .map(|h| self.to_screen(box_handle_at(box_, h), rect, ppp))
            .collect();
        painter.add(egui::Shape::closed_line(
            quad.clone(),
            egui::Stroke::new(1.0, color::SELECT_DIM),
        ));
        for at in quad {
            painter.rect_stroke(
                egui::Rect::from_center_size(at, egui::Vec2::splat(HANDLE_PX)),
                egui::CornerRadius::ZERO,
                egui::Stroke::new(HANDLE_STROKE_PX, color::SELECT),
                egui::StrokeKind::Middle,
            );
        }
    }

    /// Image editing's own chrome, drawn where `chrome_hidden` has taken the
    /// selection's away: the dashed crop window with hollow handles, and the solid
    /// outline of the whole picture behind it (the limit a handle stops at,
    /// `tools::crop_resized`), drawn only under `Crop`. Read through
    /// [`Self::shown_crop`], so it tracks a drag.
    fn draw_crop_box(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let Some((id, fill)) = self.edited_image() else {
            return;
        };
        let Some(world) = self.session.preview_world_transform(id) else {
            return;
        };
        let Some((_, limit, frame)) = self.shown_crop(id, fill) else {
            return;
        };
        let quad = |b: KRect| -> Vec<egui::Pos2> {
            Handle::CORNERS
                .into_iter()
                .map(|h| self.to_screen(world * box_handle_at(b, h), rect, ppp))
                .collect()
        };
        let picture = self.image_ghost_live(id, fill).then(|| quad(limit));
        for shape in crop_chrome(&quad(frame), picture.as_deref()) {
            painter.add(shape);
        }
    }

    /// Trace the key layer, in the hover outline's treatment but `color::KEY`:
    /// "key" is a distinction within a selection, so not a shade of `color::SELECT`.
    fn draw_key_outline(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if !self.hover_chrome_live() {
            return;
        }
        let Some(key) = self.session.selection.key() else {
            return;
        };
        // The visibility chain, which `stroke_outlines` does not walk.
        if !self.shown_visible(key) {
            return;
        }
        let stroke = egui::Stroke::new(3.0, color::KEY);
        self.stroke_outlines(painter, key, rect, ppp, stroke, self.outline_view(stroke));
    }

    /// A selected text node's **line box**, dashed in `SELECT_DIM` outside the
    /// trimmed box that carries the handles. Nothing when trim changed nothing
    /// ([`ondin_core::text::TextLayout::untrimmed_bounds`] by equality), which
    /// covers every `Fixed` node.
    fn draw_text_line_boxes(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        offset: egui::Vec2,
    ) {
        /// The guide extension's dash.
        const DASH: f32 = 3.0;
        const GAP: f32 = 3.0;

        let stroke = egui::Stroke::new(1.0, color::SELECT_DIM);
        for id in self.session.selection.ids() {
            let Some(layout) = self.session.preview_text_layout(*id) else {
                continue;
            };
            let line_box = layout.untrimmed_bounds();
            if line_box == layout.bounds() {
                continue;
            }
            let Some(world) = self.session.preview_world_transform(*id) else {
                continue;
            };
            let quad = self.local_quad(line_box, world, rect, ppp);
            // `Shape::dashed_line` takes a polyline, so repeat the first corner.
            let mut closed: Vec<egui::Pos2> = quad.iter().map(|p| *p + offset).collect();
            closed.push(closed[0]);
            for shape in egui::Shape::dashed_line(&closed, stroke, DASH, GAP) {
                painter.add(shape);
            }
        }
    }

    /// A railed text node's rail, drawn while the node is selected or edited —
    /// otherwise the curve is invisible once its layer is consumed. Outline only
    /// (the rail has no in-place edit; *Detach from path* is the way back), solid
    /// `SELECT_DIM` since a rail is authored geometry.
    fn draw_text_rails(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        offset: egui::Vec2,
    ) {
        let stroke = egui::Stroke::new(1.0, color::SELECT_DIM);
        // Plus a live session's node when unselected; filtered, or the usual case
        // draws a translucent rail twice.
        let live = self.text.as_ref().map(|s| s.id);
        let live = live.filter(|id| !self.session.selection.contains(*id));
        for id in self.session.selection.ids().iter().copied().chain(live) {
            let Some(NodeKind::Text {
                on_path: Some(rail),
                ..
            }) = self.session.display_node(id).map(|n| n.kind())
            else {
                continue;
            };
            let Some(world) = self.session.preview_world_transform(id) else {
                continue;
            };
            let mut in_world = rail.as_ref().clone();
            in_world.apply_affine(world);
            for sub in self.flatten_to_screen(&in_world, rect, ppp) {
                let mut shape = sub.stroked(stroke);
                shape.translate(offset);
                painter.add(shape);
            }
        }
    }

    /// A selected text node's baselines, solid `SELECT_DIM`, gated on *Snap to
    /// baselines*. Drawn for a rotated node too, though `baseline_lines` will not
    /// snap to a slanted one.
    fn draw_text_baselines(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        offset: egui::Vec2,
    ) {
        if !self.snap_baselines {
            return;
        }
        let stroke = egui::Stroke::new(1.0, color::SELECT_DIM);
        for id in self.session.selection.ids() {
            let (Some(layout), Some(world)) = (
                self.session.preview_text_layout(*id),
                self.session.preview_world_transform(*id),
            ) else {
                continue;
            };
            let box_ = layout.bounds();
            for local in &layout.baselines {
                let ends = [box_.min_x(), box_.max_x()]
                    .map(|x| self.to_screen(world * Point::new(x, *local), rect, ppp) + offset);
                painter.line_segment(ends, stroke);
            }
        }
    }

    /// The **measure overlay**: while `Alt` is held, the distances between what is
    /// selected and what the pointer is over, a layer or a ruler guide either way
    /// round; a hovered guide wins over the layer beneath. `Alt` is unambiguous
    /// since gestures own every other `Alt` meaning ([`Self::hover_chrome_live`]).
    /// World bounding boxes, so a rotated layer is measured by its upright box.
    fn draw_measures(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        if !ui.input(|i| i.modifiers.alt) || !self.hover_chrome_live() {
            return;
        }
        let Some(p) = resp.hover_pos() else { return };
        let selected_box = self
            .session
            .selection
            .ids()
            .iter()
            .filter_map(|id| self.session.preview_world_bounds(*id))
            .reduce(|a, b| a.union(b));

        // `Selection` never holds layers and guides at once.
        let measures = match selected_box {
            Some(from) => match self.measure_guide(p, rect, ppp) {
                Some((axis, at)) => measure::to_guide(from, axis, at),
                None => {
                    let to = self
                        .hover_target(ui, rect, ppp, resp)
                        .and_then(|id| self.session.preview_world_bounds(id));
                    match to {
                        Some(to) => measure::between(from, to),
                        None => return,
                    }
                }
            },
            None => {
                let guides = self.session.selection.guides();
                if guides.is_empty() {
                    return;
                }
                let selected = || guides.iter().filter_map(|g| self.measurable_guide(*g));

                match self.measure_guide(p, rect, ppp) {
                    Some(hovered) => {
                        // Two parallel guides share no band, so the line sits at
                        // the middle of `canvas_inner` (`measure::between_guides`).
                        let middle = self.to_world(self.canvas_inner(rect).center(), rect, ppp);
                        let at = match hovered.0 {
                            Axis::Vertical => middle.y,
                            Axis::Horizontal => middle.x,
                        };
                        selected()
                            .filter_map(|sel| measure::between_guides(sel, hovered, at))
                            .collect()
                    }
                    None => {
                        let Some(to) = self
                            .hover_target(ui, rect, ppp, resp)
                            .and_then(|id| self.session.preview_world_bounds(id))
                        else {
                            return;
                        };
                        selected()
                            .flat_map(|(axis, at)| measure::to_guide(to, axis, at))
                            .collect()
                    }
                }
            }
        };
        for m in measures {
            self.draw_measure(painter, rect, ppp, m);
        }
    }

    /// One measure: the line, a tick across each end (so a short measure shows
    /// where it stops once the pill covers its middle), and the number on a pill.
    /// Shared by [`Self::draw_measures`] and the equal-gap snap overlay.
    fn draw_measure(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        m: measure::Measure,
    ) {
        /// Half the length of the tick across each end.
        const TICK_PX: f32 = 3.0;
        const PAD: egui::Vec2 = egui::vec2(4.0, 2.0);

        let (a, b) = match m.axis {
            Axis::Horizontal => (Point::new(m.from, m.at), Point::new(m.to, m.at)),
            Axis::Vertical => (Point::new(m.at, m.from), Point::new(m.at, m.to)),
        };
        let (a, b) = (self.to_screen(a, rect, ppp), self.to_screen(b, rect, ppp));
        let stroke = egui::Stroke::new(1.0, color::MEASURE);
        painter.line_segment([a, b], stroke);
        let across = match m.axis {
            Axis::Horizontal => egui::vec2(0.0, TICK_PX),
            Axis::Vertical => egui::vec2(TICK_PX, 0.0),
        };
        for end in [a, b] {
            painter.line_segment([end - across, end + across], stroke);
        }

        let galley = painter.layout_no_wrap(
            measure::label(m.distance()),
            egui::FontId::proportional(9.5),
            color::MEASURE_INK,
        );
        let pill = egui::Rect::from_center_size(a + (b - a) * 0.5, galley.size() + PAD * 2.0);
        painter.rect_filled(pill, egui::CornerRadius::same(3), color::MEASURE);
        painter.galley(pill.min + PAD, galley, color::MEASURE_INK);
    }

    /// The world rectangle a chrome outline of `stroke` can put ink into: the
    /// viewport inflated by the ring's whole width, deliberate slack (as is
    /// dividing points by `Camera::zoom`'s device pixels).
    fn outline_view(&self, stroke: egui::Stroke) -> KRect {
        let view = self.session.camera.viewport(self.canvas_px).view;
        let pad = f64::from(stroke.width) / self.session.camera.zoom;
        view.inflate(pad, pad)
    }

    /// Stroke the exact outline of `id`, recursing into containers; `view` (from
    /// [`Self::outline_view`]) rejects off-screen subtrees. `bounds_cover_subtree`
    /// is `scene.rs`'s predicate, but with `preview_world_bounds`, not `ink_bounds`,
    /// since a ring traces the outline only. Hidden nodes (via `display_node`)
    /// skip with their subtree; a hidden ancestor of `id` is the caller's check.
    fn stroke_outlines(
        &self,
        painter: &egui::Painter,
        id: NodeId,
        rect: egui::Rect,
        ppp: f32,
        stroke: egui::Stroke,
        view: KRect,
    ) {
        let Some(node) = self.session.doc.get(id) else {
            return;
        };
        if !self.session.display_node(id).is_some_and(|n| n.visible()) {
            return;
        }
        let bounds_cover_subtree = node.children().is_empty()
            || node.clip()
            || matches!(
                node.kind(),
                NodeKind::Group | NodeKind::Root | NodeKind::Boolean { .. }
            );
        if bounds_cover_subtree
            && let Some(b) = self.session.preview_world_bounds(id)
            && !(b.min_x() <= view.max_x()
                && b.max_x() >= view.min_x()
                && b.min_y() <= view.max_y()
                && b.max_y() >= view.min_y())
        {
            return;
        }
        // `Resolved::local_path`, not `geometry::local_path`, so a boolean traces
        // its result. Committed geometry, previewed transform.
        if !matches!(node.kind(), NodeKind::Artboard { .. })
            && let Some(path) = self.session.resolved.local_path(&self.session.doc, id)
            && let Some(world) = self.session.preview_world_transform(id)
        {
            let mut in_world = path;
            in_world.apply_affine(world);
            for sub in self.flatten_to_screen(&in_world, rect, ppp) {
                painter.add(sub.stroked(stroke));
            }
            return;
        }
        // Text and artboards have a box but no outline; a boolean with no derived
        // outline is traced by its box, never its operands.
        match node.kind() {
            NodeKind::Text { .. } | NodeKind::Artboard { .. } | NodeKind::Boolean { .. } => {
                if let Some(quad) = self.selection_quad(id, rect, ppp) {
                    painter.add(egui::Shape::closed_line(quad.to_vec(), stroke));
                }
            }
            _ => {
                for child in node.children() {
                    self.stroke_outlines(painter, *child, rect, ppp, stroke, view);
                }
            }
        }
    }

    /// The wheel: scroll pans, `Shift`+scroll pans sideways, `Ctrl`+scroll zooms
    /// at the cursor. Read from raw `MouseWheel` events, so a zoom notch is a step,
    /// and gated on hover since `Context::input` is global. A trackpad pinch
    /// arrives as `Ctrl`+wheel (winit raises `PinchGesture` only on macOS/iOS), so
    /// `egui::Event::Zoom` is not handled.
    fn wheel_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, ppp: f32) {
        /// Zoom per pixel of wheel travel, as egui's own `scroll_zoom_speed`.
        const ZOOM_SPEED: f64 = 1.0 / 200.0;

        // The canvas's own overlay areas (origin fields, *Done* pill) must not
        // swallow the wheel; every other overlay keeps it.
        let Some(hover) = resp.hover_pos().or_else(|| {
            let p = ui.ctx().pointer_hover_pos()?;
            let [id] = *self.session.selection.ids() else {
                return None;
            };
            let under = ui.ctx().layer_id_at(p);
            (rect.contains(p)
                && (under == Some(origin_fields_layer(id)) || under == Some(crop_exit_layer(id))))
            .then_some(p)
        }) else {
            return;
        };
        // Read before taking the input lock — `Context::options` takes its own.
        let per_line = ui
            .ctx()
            .options(|o| o.input_options.line_scroll_speed)
            .max(1.0);

        let (pan, zoom) = ui.input(|i| {
            let (mut pan, mut zoom) = (egui::Vec2::ZERO, 0.0_f32);
            for ev in &i.events {
                let egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } = ev
                else {
                    continue;
                };
                let d = match unit {
                    egui::MouseWheelUnit::Point => *delta,
                    egui::MouseWheelUnit::Line => *delta * per_line,
                    egui::MouseWheelUnit::Page => *delta * rect.height(),
                };
                // `x + y` when modified: the platform may have swapped the axis.
                // `Alt` aliases `Ctrl` (`docs/shortcuts.md` §9).
                if modifiers.command || modifiers.alt {
                    zoom += d.x + d.y;
                } else if modifiers.shift {
                    pan.x += d.x + d.y;
                } else {
                    pan += d;
                }
            }
            (pan, zoom)
        });

        if pan != egui::Vec2::ZERO {
            self.session
                .camera
                .pan_pixels(Vec2::new((pan.x * ppp) as f64, (pan.y * ppp) as f64));
        }
        if zoom != 0.0 {
            let local = Vec2::new(
                ((hover.x - rect.min.x) * ppp) as f64,
                ((hover.y - rect.min.y) * ppp) as f64,
            );
            // Clamped: a trackpad can deliver a whole gesture in one frame.
            let factor = (zoom as f64 * ZOOM_SPEED).exp().clamp(0.2, 5.0);
            self.session.camera.zoom_by(factor, local, self.canvas_px);
        }
    }

    // --- frames: their name tag, and what a click on one may select ---------

    /// The node a click at `world` resolves to, before group policy.
    ///
    /// [`hit_test`] minus occupied frames, whose background passes presses to the
    /// marquee; an empty frame and the name tag ([`Self::frame_label_at`]) stay
    /// clickable. A frame inside a group or an instance is not skipped.
    pub(crate) fn pick_leaf(&self, world: Point) -> Option<NodeId> {
        let doc = &self.session.doc;
        hit_test(&self.session.resolved, doc, world, self.pick_slop())
            .iter()
            .copied()
            .find(|id| {
                !self.is_occupied_frame(*id)
                    || group_fence(doc, *id).is_some()
                    || ondin_core::component::instance_root(doc, *id).is_some()
            })
    }

    /// The picking allowance a hit test gets, in **world** units: [`PICK_SLOP_PX`]
    /// at the current zoom.
    fn pick_slop(&self) -> f64 {
        f64::from(PICK_SLOP_PX) / self.points_per_world()
    }

    /// Logical points per world unit — what every pointer allowance is divided by.
    /// Points, not `camera.zoom`'s device pixels, so a target does not shrink as
    /// the display scale grows. Framing (`reveal_selection`'s margin) stays in
    /// device pixels.
    pub(crate) fn points_per_world(&self) -> f64 {
        self.session.camera.zoom.max(1e-6) / f64::from(self.canvas_ppp.max(1e-3))
    }

    /// A frame with at least one layer in it — the case `pick_leaf` skips.
    fn is_occupied_frame(&self, id: NodeId) -> bool {
        self.session.doc.get(id).is_some_and(|n| {
            matches!(n.kind(), NodeKind::Artboard { .. }) && !n.children().is_empty()
        })
    }

    /// A selected frame whose box `world` falls in: dragging from inside it moves
    /// it rather than starting a marquee.
    fn selected_frame_at(&self, world: Point) -> Option<NodeId> {
        self.session.selection.ids().iter().copied().find(|id| {
            self.is_occupied_frame(*id)
                && self
                    .session
                    .resolved
                    .world_bounds(*id)
                    .is_some_and(|b| b.contains(world))
        })
    }

    /// An **occupied** frame whose border passes within [`FRAME_EDGE_PICK_PX`] of
    /// `world` — `context-menus.md` §2's C5. The edge, so the interior stays the
    /// marquee's. The band is the world box, so a rotated frame's is its
    /// axis-aligned bounds. `.rev()`: `artboards` is paint order, so the last of
    /// two nested matches is the inner one.
    fn frame_edge_at(&self, world: Point) -> Option<NodeId> {
        let slop = f64::from(FRAME_EDGE_PICK_PX) / self.points_per_world();
        self.artboards().into_iter().rev().find(|id| {
            // `Selection::set_one` filters nothing; only the name tag takes a locked frame.
            self.is_occupied_frame(*id)
                && self.shown_visible(*id)
                && !ondin_core::is_effectively_locked(&self.session.doc, *id)
                && self.session.resolved.world_bounds(*id).is_some_and(|b| {
                    b.inflate(slop, slop).contains(world)
                        && !b.inflate(-slop, -slop).contains(world)
                })
        })
    }

    /// Whether `id` is on screen: it and every ancestor switched on, read through
    /// [`crate::session::EditorSession::display_node`] so the eye previews.
    /// Visibility only, not `locked`.
    pub(crate) fn shown_visible(&self, id: NodeId) -> bool {
        let mut cursor = Some(id);
        while let Some(c) = cursor {
            let Some(node) = self.session.display_node(c) else {
                return false;
            };
            if !node.visible() {
                return false;
            }
            cursor = self.session.doc.get(c).and_then(|n| n.parent());
        }
        true
    }

    /// What a click at `screen` picks — the **one** chain every door uses
    /// (`context-menus.md` §2 C1, C5): the name tag (an occupied frame's handle,
    /// which [`Self::pick_leaf`] skips), then the leaf, then [`Self::frame_edge_at`]
    /// so a layer near a border wins. [`Self::begin_select_drag`] adds
    /// [`Self::selected_frame_at`], which this chain must not.
    fn pick_at_pointer(
        &self,
        ui: &egui::Ui,
        screen: egui::Pos2,
        world: Point,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<NodeId> {
        self.frame_label_at(ui, screen, rect, ppp)
            .or_else(|| self.pick_leaf(world))
            .or_else(|| self.frame_edge_at(world))
    }

    /// The frame whose name tag is under `screen`. `.rev()`: the list is paint
    /// order and [`Self::draw_frame_labels`] lets the last tag win the pixel.
    fn frame_label_at(
        &self,
        ui: &egui::Ui,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<NodeId> {
        self.tagged().into_iter().rev().find(|id| {
            self.frame_label(ui, *id, rect, ppp)
                .is_some_and(|(r, _)| r.contains(screen))
        })
    }

    /// Where a frame's name tag sits, and its text: above the top-left corner and
    /// *outside* the frame, so it never covers artwork.
    fn frame_label(
        &self,
        ui: &egui::Ui,
        id: NodeId,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<(egui::Rect, std::sync::Arc<egui::Galley>)> {
        const PT: f32 = LABEL_PT;
        const INSET_X: f32 = 5.0;
        const GAP_Y: f32 = 5.0;

        let node = self.session.doc.get(id)?;
        let is_main = node.component();
        let tagged = match node.kind() {
            NodeKind::Artboard { .. } => true,
            NodeKind::Group => is_main,
            _ => false,
        };
        if !tagged {
            return None;
        }
        // No tag inside an instance, nor on an instance inside a main
        // (`tag_withheld`); a main keeps its chip, its only handle.
        if !is_main && tag_withheld(&self.session.doc, id) {
            return None;
        }
        if !self.shown_visible(id) {
            return None;
        }
        let corner = self.selection_quad(id, rect, ppp)?[0];
        let doc = &self.session.doc;
        // A component set's tag is a tab: glyph, name, then the clash or (while
        // selected) the variant count. `PLACEHOLDER` leaves the ink to
        // `Painter::galley`, so one layout draws selected or not.
        let is_set = node.set().is_some();
        let clashing = ondin_core::variant::set_of(doc, id)
            .is_some_and(|s| ondin_core::variant::clashes(doc, s).contains(&id));
        let mut job = egui::text::LayoutJob::default();
        let glyph = |job: &mut egui::text::LayoutJob, g: &str, ink| {
            job.append(g, 0.0, egui::TextFormat::simple(theme::icon_font(PT), ink));
        };
        let words = |job: &mut egui::text::LayoutJob, t: &str, lead: f32, ink| {
            job.append(
                t,
                lead,
                egui::TextFormat::simple(egui::FontId::proportional(PT), ink),
            );
        };
        // A mode the node sets follows its name on every tag, dim; an inherited
        // one stays off the canvas.
        let mode = node.mode().and_then(|m| doc.variables().mode(m));
        let mode_word = |job: &mut egui::text::LayoutJob, ink| {
            if let Some(m) = mode {
                words(job, &format!("· {}", m.name), 4.0, ink);
            }
        };
        if is_set {
            glyph(&mut job, icon::SQUARES_FOUR, egui::Color32::PLACEHOLDER);
            words(&mut job, node.name(), 4.0, egui::Color32::PLACEHOLDER);
            mode_word(&mut job, theme::text::DIM);
            if let Some(clash) = crate::panels::clash_text(doc, id) {
                job.append(
                    icon::WARNING,
                    8.0,
                    egui::TextFormat::simple(theme::icon_font(PT), theme::text::STRONG),
                );
                words(&mut job, &clash, 3.0, theme::text::STRONG);
            } else if self.session.selection.contains(id) {
                let n = ondin_core::variant::variants(doc, id).len();
                let count = match n {
                    1 => "1 variant".to_string(),
                    n => format!("{n} variants"),
                };
                words(&mut job, &count, 8.0, theme::text::DIM);
            }
        } else if is_main {
            // A filled hexagon on a violet chip: the outline glyph holds the cell
            // unpainted and `draw_frame_labels` fills it (no filled Phosphor one).
            if clashing {
                glyph(&mut job, icon::WARNING, egui::Color32::PLACEHOLDER);
            }
            glyph(&mut job, icon::HEXAGON, egui::Color32::TRANSPARENT);
            words(&mut job, node.name(), 4.0, egui::Color32::PLACEHOLDER);
            mode_word(&mut job, color::COMPONENT_INK_DIM);
            if self.session.selection.contains(id) {
                let n = ondin_core::component::instances_of(doc, id).len();
                let count = match n {
                    1 => "1 instance".to_string(),
                    n => format!("{n} instances"),
                };
                words(&mut job, &count, 8.0, color::COMPONENT_INK_DIM);
            }
        } else if ondin_core::component::instance_root(doc, id) == Some(id) {
            // An instance: outline hexagon and a violet label, trailing its main's
            // name when renamed, a path when entered. Only while selected or
            // entered, since it picks like a group and the tag is no handle.
            if !self.session.selection.contains(id) && self.entered_instance() != Some(id) {
                return None;
            }
            glyph(&mut job, icon::HEXAGON, egui::Color32::PLACEHOLDER);
            let main_name = ondin_core::component::main_of(doc, id)
                .and_then(|m| doc.get(m))
                .map(|m| m.name().to_string())
                .unwrap_or_default();
            let inside = (self.entered_instance() == Some(id))
                .then(|| self.session.selection.single())
                .flatten()
                .filter(|s| *s != id && ondin_core::is_within(doc, *s, id))
                .and_then(|s| doc.get(s));
            match inside {
                Some(child) => {
                    words(&mut job, node.name(), 4.0, egui::Color32::PLACEHOLDER);
                    words(&mut job, "›", 5.0, color::COMPONENT_DIM);
                    words(&mut job, child.name(), 5.0, egui::Color32::PLACEHOLDER);
                }
                None => {
                    words(&mut job, node.name(), 4.0, egui::Color32::PLACEHOLDER);
                    mode_word(&mut job, color::COMPONENT_DIM);
                    if !main_name.is_empty() && main_name != node.name() {
                        words(&mut job, &main_name, 6.0, color::COMPONENT_DIM);
                    }
                }
            }
        } else {
            if clashing {
                glyph(&mut job, icon::WARNING, egui::Color32::PLACEHOLDER);
            }
            words(
                &mut job,
                node.name(),
                if clashing { 4.0 } else { 0.0 },
                egui::Color32::PLACEHOLDER,
            );
            mode_word(&mut job, color::text_a(179));
        }
        let galley = ui.painter().layout_job(job);
        let size = galley.size();
        if is_set {
            // Fused to the frame's top edge.
            let min = egui::pos2(corner.x, corner.y - size.y - SET_TAB_PAD.y * 2.0);
            let tab = egui::Rect::from_min_size(min, size + SET_TAB_PAD * 2.0);
            return Some((tab, galley));
        }
        if is_main {
            // A point wider at the right while it ends on the name, not the count.
            let tail = if self.session.selection.contains(id) {
                0.0
            } else {
                1.0
            };
            let min = egui::pos2(
                corner.x + INSET_X - CHIP_PAD.x,
                corner.y - GAP_Y - size.y - CHIP_PAD.y * 2.0,
            );
            return Some((
                egui::Rect::from_min_size(min, size + CHIP_PAD * 2.0 + egui::vec2(tail, 0.0)),
                galley,
            ));
        }
        let min = egui::pos2(corner.x + INSET_X, corner.y - GAP_Y - size.y);
        Some((egui::Rect::from_min_size(min, size), galley))
    }

    /// The instance the user is working inside — the nearest instance root at or
    /// above [`OndinApp::entered_group`] — whose tag is a path and whose edge is
    /// dashed. At or above, so the chrome stays while the user steps further in.
    fn entered_instance(&self) -> Option<NodeId> {
        self.entered_group
            .and_then(|g| ondin_core::component::instance_root(&self.session.doc, g))
    }

    /// The hue of the whole selection's chrome (handles, size badge): `COMPONENT`
    /// when every selected layer is a main or an instance, `SELECT` otherwise.
    fn selection_hue(&self) -> egui::Color32 {
        let ids = self.session.selection.ids();
        if !ids.is_empty()
            && ids
                .iter()
                .all(|id| chrome_hue(&self.session.doc, *id) == color::COMPONENT)
        {
            color::COMPONENT
        } else {
            color::SELECT
        }
    }

    /// Where the `+` of the one selected component set sits: the middle of its
    /// bottom edge, or `None` when the selection is not a single set on screen.
    fn set_plus(&self, rect: egui::Rect, ppp: f32) -> Option<(NodeId, egui::Pos2)> {
        let set = self.session.selection.single()?;
        if !ondin_core::variant::is_set(&self.session.doc, set) || !self.shown_visible(set) {
            return None;
        }
        let q = self.selection_quad(set, rect, ppp)?;
        Some((set, q[2] + (q[3] - q[2]) / 2.0))
    }

    /// The selected set whose `+` is under `screen` — a disc of [`SET_PLUS_R`]
    /// and the pick slop.
    fn set_plus_at(&self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<NodeId> {
        let (set, at) = self.set_plus(rect, ppp)?;
        (at.distance(screen) <= SET_PLUS_R + PICK_SLOP_PX).then_some(set)
    }

    /// A selected set's `+`: a disc on its bottom edge's middle, in the tab's
    /// ground and border; a click adds a variant. Drawn over the selection outline.
    fn draw_set_plus(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let Some((_, at)) = self.set_plus(rect, ppp) else {
            return;
        };
        painter.circle(
            at,
            SET_PLUS_R,
            theme::color::CARD,
            egui::Stroke::new(1.0, theme::color::CARD_BORDER),
        );
        let s = egui::Stroke::new(1.5, theme::text::STRONG);
        let arm = SET_PLUS_R * 0.5;
        painter.line_segment([at - egui::vec2(arm, 0.0), at + egui::vec2(arm, 0.0)], s);
        painter.line_segment([at - egui::vec2(0.0, arm), at + egui::vec2(0.0, arm)], s);
    }

    /// The neutral components chrome over the artwork:
    /// - **a selected main's instances** each get a hairline round their box;
    /// - **an entered instance** keeps a dashed boundary and a quiet tag saying
    ///   how to leave.
    fn draw_component_chrome(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let doc = &self.session.doc;
        let hair = egui::Stroke::new(1.0, theme::text::FAINT);
        let outline = |q: [egui::Pos2; 4]| {
            for i in 0..4 {
                painter.line_segment([q[i], q[(i + 1) % 4]], hair);
            }
        };
        for main in self.session.selection.ids() {
            if !doc.get(*main).is_some_and(|n| n.component()) {
                continue;
            }
            for inst in ondin_core::component::instances_of(doc, *main) {
                if self.shown_visible(inst)
                    && let Some(q) = self.selection_quad(inst, rect, ppp)
                {
                    outline(q);
                }
            }
        }
        let Some(entered) = self.entered_instance() else {
            return;
        };
        let Some(q) = self.selection_quad(entered, rect, ppp) else {
            return;
        };
        let edge = egui::Stroke::new(1.0, theme::text::DIM);
        painter.extend(egui::Shape::dashed_line(
            &[q[0], q[1], q[2], q[3], q[0]],
            edge,
            4.0,
            3.0,
        ));
        painter.text(
            q[3] + egui::vec2(5.0, 5.0),
            egui::Align2::LEFT_TOP,
            "Editing inside instance · Esc to exit",
            egui::FontId::proportional(LABEL_PT),
            theme::text::DIM,
        );
    }

    /// Every frame's name, drawn under the selection chrome.
    fn draw_frame_labels(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        for id in self.tagged() {
            let Some((label, galley)) = self.frame_label(ui, id, rect, ppp) else {
                continue;
            };
            // A main's violet chip and filled hexagon, selected or not.
            if self.session.doc.get(id).is_some_and(|n| n.component()) {
                painter.rect_filled(label, 3.0, color::COMPONENT);
                let at = label.min + CHIP_PAD;
                if let Some(cell) = galley.rows.first().and_then(|r| {
                    r.glyphs
                        .iter()
                        .find(|g| g.chr.to_string() == icon::HEXAGON)
                        .map(|g| g.logical_rect().translate(r.pos.to_vec2()))
                }) {
                    // A point above the cell's centre, which sat low.
                    crate::ui::paint_hexagon_filled(
                        painter,
                        at + cell.center().to_vec2() - egui::vec2(0.0, 1.0),
                        LABEL_PT,
                        color::COMPONENT_INK,
                    );
                }
                painter.galley(at, galley, color::COMPONENT_INK);
                continue;
            }
            // An instance's tag, violet selected or not.
            if ondin_core::component::instance_root(&self.session.doc, id) == Some(id) {
                painter.galley(label.min, galley, color::COMPONENT);
                continue;
            }
            let is_set = self.session.doc.get(id).is_some_and(|n| n.set().is_some());
            if is_set {
                // A set's tab and resting hairline, neutral selected or not.
                if let Some(q) = self.selection_quad(id, rect, ppp) {
                    let stroke = egui::Stroke::new(1.0, theme::color::CARD_BORDER);
                    for i in 0..4 {
                        painter.line_segment([q[i], q[(i + 1) % 4]], stroke);
                    }
                }
                painter.rect(
                    label,
                    egui::CornerRadius {
                        nw: 3,
                        ne: 3,
                        sw: 0,
                        se: 0,
                    },
                    theme::color::CARD,
                    egui::Stroke::new(1.0, theme::color::CARD_BORDER),
                    egui::StrokeKind::Inside,
                );
                painter.galley(label.min + SET_TAB_PAD, galley, theme::text::STRONG);
                continue;
            }
            let ink = if self.session.selection.contains(id) {
                color::SELECT
            } else {
                theme::text::MUTED
            };
            painter.galley(label.min, galley, ink);
        }
    }

    /// The layout columns and rows drawn over every frame that carries them.
    /// Chrome only — not in the scene, any export or the snapshot. Each track is
    /// clipped to the frame's box here, in local space so bands turn with a
    /// rotated frame.
    fn draw_layout_grids(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        // Present mode does not hide the grids, as `rulers::guides_on` for guides.

        // An outer gate, never a write to each `LayoutGrid::visible`, so turning
        // it back on restores every eye.
        if !self.show_layout_grids {
            return;
        }
        for id in self.artboards() {
            let Some(node) = self.session.doc.get(id) else {
                continue;
            };
            if node.grids().is_empty() || !self.shown_visible(id) {
                continue;
            }
            // The display node's kind, so the grid follows a previewed resize.
            let Some(NodeKind::Artboard { size, .. }) =
                self.session.display_node(id).map(|n| n.kind())
            else {
                continue;
            };
            let mut grids = node.grids().to_vec();
            // The in-flight scrub value (`OndinApp::grid_scrub`).
            if let Some((scrubbed, index, held)) = &self.in_flight.grid_scrub
                && scrubbed.contains(&id)
                && let Some(slot) = grids.get_mut(*index)
            {
                slot.value = *held;
            }
            // The colour a picker is dragging (`OndinApp::grid_paint`), after the
            // scrub so its stale colour does not win.
            if let Some((painted, index, color)) = &self.in_flight.grid_paint
                && painted.contains(&id)
                && let Some(slot) = grids.get_mut(*index)
            {
                slot.color = *color;
            }
            let size = *size;
            // Previewed, or a moved frame slides out from under its grid.
            let Some(world) = self.session.preview_world_transform(id) else {
                continue;
            };
            let box_ = KRect::new(0.0, 0.0, size.width, size.height);
            for grid in grids.iter().filter(|g| g.visible) {
                let ink = crate::ui::color_to_egui(grid.color);
                for track in ondin_core::grid_tracks(grid, size) {
                    let band = track.intersect(box_);
                    if band.width() <= 0.0 || band.height() <= 0.0 {
                        continue;
                    }
                    painter.add(egui::Shape::convex_polygon(
                        self.local_quad(band, world, rect, ppp).to_vec(),
                        ink,
                        egui::Stroke::NONE,
                    ));
                }
            }
        }
    }

    /// How far the chrome moves to sit on the Alt-drag copy (which will be
    /// selected on release), in screen points. Zero when no copy is in flight.
    fn alt_clone_offset(&self, rect: egui::Rect, ppp: f32) -> egui::Vec2 {
        let Some(clone) = &self.alt_clone else {
            return egui::Vec2::ZERO;
        };
        let origin = self.to_screen(Point::ZERO, rect, ppp);
        self.to_screen(Point::ZERO + clone.delta, rect, ppp) - origin
    }

    /// Outline the frame a live move would drop the selection into (membership
    /// follows the artwork, [`Self::move_tx`]). Frames only: the root has no box.
    fn draw_frame_drop_outline(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        let Drag::Move { anchor } = self.drag else {
            return;
        };
        // The release's delta, snapping included.
        let Some(pointer) = resp.interact_pointer_pos() else {
            return;
        };
        let world = self.to_world(pointer, rect, ppp);
        let shift = ui.input(|i| i.modifiers.shift);
        let (delta, _) = self.snapped_move(world - anchor, shift);

        let root = self.session.doc.root();
        let mut seen: Vec<NodeId> = Vec::new();
        for id in build::outermost(&self.session.doc, self.session.selection.ids()) {
            match self.move_destination(id, delta) {
                Some(to) if to != root && !seen.contains(&to) => seen.push(to),
                _ => {}
            }
        }
        let stroke = egui::Stroke::new(3.0, color::SELECT);
        for frame in seen {
            // Committed, or a selected frame's outline would ride the drag.
            if let Some(quad) = self.committed_quad(frame, rect, ppp) {
                painter.add(egui::Shape::closed_line(quad.to_vec(), stroke));
            }
        }
    }

    /// Where a dragged flex item will drop, dashed — the slot read off the preview
    /// ([`ondin_render::RenderOverrides::landings`]) so outline and release agree.
    fn draw_flex_landing(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        let Drag::Move { anchor } = self.drag else {
            return;
        };
        // Only for the item the release will reorder: an item leaving its frame
        // still has a slot in the preview.
        let Some(pointer) = resp.interact_pointer_pos() else {
            return;
        };
        let shift = ui.input(|i| i.modifiers.shift);
        let (delta, _) = self.snapped_move(self.to_world(pointer, rect, ppp) - anchor, shift);
        let Some((reordered, _)) = self.flex_reorder_of(delta) else {
            return;
        };
        let (_, _, ov) = self.session.render_inputs();
        for (_, world, bx) in ov.landings().iter().filter(|(id, _, _)| *id == reordered) {
            let corners = [
                Point::new(bx.x0, bx.y0),
                Point::new(bx.x1, bx.y0),
                Point::new(bx.x1, bx.y1),
                Point::new(bx.x0, bx.y1),
                Point::new(bx.x0, bx.y0),
            ]
            .map(|c| self.to_screen(*world * c, rect, ppp));
            painter.extend(egui::Shape::dashed_line(
                &corners,
                egui::Stroke::new(1.5, color::SELECT),
                4.0,
                3.0,
            ));
        }
    }

    /// A grid container's tracks, read-only: each edge a 1px `SELECT_DIM` line
    /// (dashed for an explicit track, dotted for implicit), each gap a
    /// [`color::GRID_GAP`] band. Not the layout grids of [`Self::draw_layout_grids`].
    ///
    /// For every selected container (from the preview,
    /// [`ondin_render::RenderOverrides::laid_grid`]) and a moved layer's container
    /// ([`ondin_core::build::drop_grid`], the grid the drop reads). Hidden with
    /// [`Self::chrome_hidden`] except under an edit hold of its own layout.
    /// Uncached, since the preview does not move the revision.
    fn draw_grid_tracks(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if self.editing_in_place() {
            return;
        }
        let held = self.chrome_hold.holding();
        let (doc, res, ov) = self.session.render_inputs();
        let mut subjects: Vec<NodeId> = self.session.selection.ids().to_vec();
        if matches!(self.drag, Drag::Move { .. }) {
            for id in self.session.selection.ids() {
                if let Some(parent) = doc.get(*id).and_then(|n| n.parent())
                    && !subjects.contains(&parent)
                {
                    subjects.push(parent);
                }
            }
        }
        // A selected container's lines ride an Alt-drag's copy.
        let copied = self.alt_clone_offset(rect, ppp);
        for id in subjects {
            if !self.shown_visible(id) {
                continue;
            }
            // Under an edit hold, hidden only while the preview restyles this
            // container but not its layout; the hold outlives the preview.
            if held && ov.get(id).is_some_and(|o| o.display.is_none()) {
                continue;
            }
            let selected = self.session.selection.ids().contains(&id);
            let offset = match selected {
                true => copied,
                false => egui::Vec2::ZERO,
            };
            // A moved item's container is placed as committed, as the drop reads it.
            let placed = match selected {
                true => self
                    .session
                    .preview_local_box(id)
                    .zip(self.session.preview_world_transform(id)),
                false => ondin_core::local_box(doc, res, id).zip(res.world_transform(id)),
            };
            let Some((bx, world)) = placed else {
                continue;
            };
            // Culled before the layout pass, which is then saved.
            let seen = |quad: [egui::Pos2; 4]| egui::Rect::from_points(&quad).intersects(rect);
            if !seen(self.local_quad(bx, world, rect, ppp).map(|p| p + offset)) {
                continue;
            }
            let grid = match selected {
                true => ov.laid_grid(doc, res, id),
                false => ondin_core::build::drop_grid(doc, res, id),
            };
            let Some(grid) = grid else {
                continue;
            };
            #[cfg(test)]
            GRIDS_LAID.with(|n| n.set(n.get() + 1));
            let lines = Self::track_lines(&grid, bx);
            for band in lines.gaps {
                let quad = self.local_quad(band, world, rect, ppp).map(|p| p + offset);
                if !seen(quad) {
                    continue;
                }
                painter.add(egui::Shape::convex_polygon(
                    quad.to_vec(),
                    color::GRID_GAP,
                    egui::Stroke::NONE,
                ));
            }
            let stroke = egui::Stroke::new(1.0, color::SELECT_DIM);
            for (a, b, explicit) in lines.lines {
                let [a, b] = [a, b].map(|p| self.to_screen(world * p, rect, ppp) + offset);
                let (dash, gap) = if explicit { (4.0, 3.0) } else { (1.5, 2.5) };
                if let Some(ends) = Self::dashed_in_view(a, b, rect, dash + gap) {
                    painter.extend(egui::Shape::dashed_line(&ends, stroke, dash, gap));
                }
            }
        }
    }

    /// A laid grid's lines and gaps in the container's own space: every track edge
    /// once, across the other axis's run of tracks (`bx` where that axis has none),
    /// marked `true` where an **explicit** track has it; and every space between
    /// neighbouring tracks as a band. Coinciding edges merge.
    fn track_lines(grid: &ondin_core::container::LaidGrid, bx: KRect) -> TrackLines {
        use ondin_core::container::LaidTracks;
        let run = |t: &LaidTracks, lo: f64, hi: f64| match (t.spans.first(), t.spans.last()) {
            (Some(first), Some(last)) => (first.0, last.1),
            _ => (lo, hi),
        };
        // The spans are in order, so an edge can only coincide with the last one.
        let edges = |t: &LaidTracks| {
            let first = usize::from(t.before);
            let explicit = first..first + usize::from(t.explicit);
            let mut out: Vec<(f64, bool)> = Vec::new();
            for (i, (a, b)) in t.spans.iter().enumerate() {
                for v in [*a, *b] {
                    match out.last_mut().filter(|(w, _)| (w - v).abs() < 1e-6) {
                        Some(edge) => edge.1 |= explicit.contains(&i),
                        None => out.push((v, explicit.contains(&i))),
                    }
                }
            }
            out
        };
        let gaps = |t: &LaidTracks| -> Vec<(f64, f64)> {
            t.spans
                .windows(2)
                .filter(|w| w[1].0 > w[0].1 + 1e-6)
                .map(|w| (w[0].1, w[1].0))
                .collect()
        };
        let (y0, y1) = run(&grid.rows, bx.y0, bx.y1);
        let (x0, x1) = run(&grid.columns, bx.x0, bx.x1);
        let mut out = TrackLines::default();
        for (x, explicit) in edges(&grid.columns) {
            out.lines
                .push((Point::new(x, y0), Point::new(x, y1), explicit));
        }
        for (y, explicit) in edges(&grid.rows) {
            out.lines
                .push((Point::new(x0, y), Point::new(x1, y), explicit));
        }
        out.gaps.extend(
            gaps(&grid.columns)
                .into_iter()
                .map(|(a, b)| KRect::new(a, y0, b, y1)),
        );
        out.gaps.extend(
            gaps(&grid.rows)
                .into_iter()
                .map(|(a, b)| KRect::new(x0, a, x1, b)),
        );
        out
    }

    /// The part of the screen segment `a`→`b` worth dashing in `view`: cut to the
    /// view grown by one dash `period`, its start pulled back to a whole number of
    /// periods from `a` so the pattern holds still under a pan. `None` where the
    /// segment misses. `dashed_line` emits a segment per dash, so an uncut line
    /// costs its screen length.
    fn dashed_in_view(
        a: egui::Pos2,
        b: egui::Pos2,
        view: egui::Rect,
        period: f32,
    ) -> Option<[egui::Pos2; 2]> {
        let view = view.expand(period);
        let d = b - a;
        // Liang–Barsky: the parameters at which the segment enters and leaves.
        let (mut t0, mut t1) = (0.0_f32, 1.0_f32);
        for (p, q) in [
            (-d.x, a.x - view.min.x),
            (d.x, view.max.x - a.x),
            (-d.y, a.y - view.min.y),
            (d.y, view.max.y - a.y),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else if p < 0.0 {
                t0 = t0.max(q / p);
            } else {
                t1 = t1.min(q / p);
            }
        }
        if t0 > t1 {
            return None;
        }
        let length = d.length();
        if length <= 0.0 {
            return Some([a, b]);
        }
        let start = (t0 * length / period).floor() * period / length;
        Some([a + d * start, a + d * t1])
    }

    /// The selection outline for `id`, from the **preview** layer so it tracks a
    /// gesture.
    fn selection_quad(&self, id: NodeId, rect: egui::Rect, ppp: f32) -> Option<[egui::Pos2; 4]> {
        self.quad_of(id, rect, ppp, true)
    }

    /// The same outline from the **committed** document, ignoring any gesture in
    /// flight.
    fn committed_quad(&self, id: NodeId, rect: egui::Rect, ppp: f32) -> Option<[egui::Pos2; 4]> {
        self.quad_of(id, rect, ppp, false)
    }

    /// A node's own box as four screen points, clockwise from its local
    /// top-left. Works for any node, selected or not, and for containers.
    fn quad_of(
        &self,
        id: NodeId,
        rect: egui::Rect,
        ppp: f32,
        preview: bool,
    ) -> Option<[egui::Pos2; 4]> {
        let local = match preview {
            true => self.session.preview_local_box(id)?,
            false => ondin_core::local_box(&self.session.doc, &self.session.resolved, id)?,
        };
        let world = match preview {
            true => self.session.preview_world_transform(id)?,
            false => self.session.resolved.world_transform(id)?,
        };
        Some(self.local_quad(local, world, rect, ppp))
    }

    /// A local-space rectangle through its world transform as four screen points,
    /// clockwise from the top-left; a rotated one stays a rotated quad.
    pub(crate) fn local_quad(
        &self,
        local: KRect,
        world: Affine,
        rect: egui::Rect,
        ppp: f32,
    ) -> [egui::Pos2; 4] {
        [
            (local.min_x(), local.min_y()),
            (local.max_x(), local.min_y()),
            (local.max_x(), local.max_y()),
            (local.min_x(), local.max_y()),
        ]
        .map(|(x, y)| self.to_screen(world * Point::new(x, y), rect, ppp))
    }

    /// The two ends of the selected line, in screen space — the chrome and the grab
    /// targets that replace a box for this one kind ([`lone_line`] says which).
    fn line_ends(&self, rect: egui::Rect, ppp: f32) -> Option<(NodeId, egui::Pos2, egui::Pos2)> {
        let (id, end) = lone_line(&self.session)?;
        let world = self.session.preview_world_transform(id)?;
        Some((
            id,
            self.to_screen(world * Point::ZERO, rect, ppp),
            self.to_screen(world * end, rect, ppp),
        ))
    }

    /// The selected railed text node and the screen point its type starts at,
    /// where the *slide along the rail* handle sits. Read from the preview layout;
    /// the gesture's arithmetic reads the committed one ([`Drag::RailOffset`]).
    fn rail_handle(&self, rect: egui::Rect, ppp: f32) -> Option<(NodeId, egui::Pos2)> {
        let id = self.session.selection.single()?;
        let layout = self.session.preview_text_layout(id)?;
        let at = layout.warp.as_ref()?.start_point()?;
        let world = self.session.preview_world_transform(id)?;
        Some((id, self.to_screen(world * at, rect, ppp)))
    }

    /// Whether `screen` is grabbing that handle.
    fn grab_rail_handle(&self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<NodeId> {
        // Refused on a locked selection, as the box handles are.
        if self.selection_locked() {
            return None;
        }
        let (id, at) = self.rail_handle(rect, ppp)?;
        (at.distance(screen) <= HANDLE_PICK_PX).then_some(id)
    }

    /// The transaction sliding `id`'s type along its rail so its start follows
    /// `world`: `committed + nearest / len`. Read from `Resolved`, which a preview
    /// does not touch, or the gesture would be fed its own output. On a closed rail
    /// one drag goes at most half way round.
    fn rail_offset_tx(&self, id: NodeId, world: Point) -> Transaction {
        let Some(NodeKind::Text { on_path_offset, .. }) =
            self.session.doc.get(id).map(|n| n.kind())
        else {
            return Transaction(vec![]);
        };
        let Some(layout) = self.session.resolved.text_layout(id) else {
            return Transaction(vec![]);
        };
        let (Some(warp), Some(node_world)) = (
            layout.warp.as_ref(),
            self.session.resolved.world_transform(id),
        ) else {
            return Transaction(vec![]);
        };
        let len = warp.length();
        if len <= 0.0 {
            return Transaction(vec![]);
        }
        let along = warp.nearest(node_world.inverse() * world).0;
        let offset = on_path_offset + along / len;
        // One lap on a closed rail, since `startOffset` need not wrap a negative.
        let offset = match warp.is_closed() {
            true => offset.rem_euclid(1.0),
            false => offset,
        };
        Transaction(vec![Operation::SetGeometry {
            id,
            geometry: ondin_core::GeometryPatch::TextPathOffset(offset),
        }])
    }

    /// Which end of the selected line `screen` is grabbing, if either; the nearer
    /// wins on a short line.
    fn grab_line_end(&self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<LineEnd> {
        // A line's endpoints are its handles: refused on a locked selection.
        if self.selection_locked() {
            return None;
        }
        let (_, start, end) = self.line_ends(rect, ppp)?;
        [(LineEnd::Start, start), (LineEnd::End, end)]
            .into_iter()
            .map(|(which, at)| (which, at.distance(screen)))
            .filter(|(_, d)| *d <= HANDLE_PICK_PX)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(which, _)| which)
    }

    /// The oriented box a **multi-selection's** chrome sits on: its frame transform
    /// and its extent in that frame's axes, shared by the handles and the size pill.
    ///
    /// Over the members' geometry quads, not ink bounds: a quad transforms
    /// exactly, so with `frame = G · F` the gesture cancels
    /// (`frame⁻¹ · (G · Wᵢ) = F⁻¹ · Wᵢ`) and the box does not grow as the set turns.
    fn selection_frame_box(&self) -> Option<(Affine, KRect)> {
        let frame = self.selection_frame();
        let extent = self.selection_frame_extent(frame, |id| {
            Some((
                self.session.preview_local_box(id)?,
                self.session.preview_world_transform(id)?,
            ))
        })?;
        Some((frame, extent))
    }

    /// A multi-selection's frame transform: the stored orientation with the gesture
    /// in flight composed on top — the one place those are combined.
    fn selection_frame(&self) -> Affine {
        self.drag.selection_frame().unwrap_or(Affine::IDENTITY) * self.session.selection.frame()
    }

    /// The selection's extent in `frame`'s axes over the members' **quads** from
    /// `quad`. The chrome passes the preview tree, the resize the committed one, or
    /// a scaled box compounds.
    fn selection_frame_extent(
        &self,
        frame: Affine,
        quad: impl Fn(NodeId) -> Option<(KRect, Affine)>,
    ) -> Option<KRect> {
        let inv = frame.inverse();
        let mut out: Option<KRect> = None;
        for id in self.session.selection.ids() {
            // A member with no box (an empty group) contributes nothing.
            let Some((local, world)) = quad(*id) else {
                continue;
            };
            let m = inv * world;
            for (x, y) in [
                (local.x0, local.y0),
                (local.x1, local.y0),
                (local.x1, local.y1),
                (local.x0, local.y1),
            ] {
                let p = m * Point::new(x, y);
                let cell = KRect::new(p.x, p.y, p.x, p.y);
                out = Some(match out {
                    Some(b) => b.union(cell),
                    None => cell,
                });
            }
        }
        out
    }

    /// The box a multi-selection **resizes** about: the handles' oriented box off
    /// the committed tree. `None` means nothing contributed.
    fn selection_resize_box(&self) -> Option<tools::SelectionBox> {
        let frame = self.session.selection.frame();
        let extent = self.selection_frame_extent(frame, |id| {
            Some((
                ondin_core::local_box(&self.session.doc, &self.session.resolved, id)?,
                self.session.resolved.world_transform(id)?,
            ))
        })?;
        Some(tools::SelectionBox { frame, extent })
    }

    /// The transform box for the selection: one layer's rotated box, or the
    /// **oriented** box over a set, from the preview. A lone line has none
    /// ([`Self::line_ends`] replaces it), refused on the kind ([`lone_line`]) so it
    /// cannot fail open.
    fn selection_handles(&self, rect: egui::Rect, ppp: f32) -> Option<Handles> {
        if lone_line(&self.session).is_some() {
            return None;
        }
        let (target, box_, world, local_pivot) = match self.session.selection.ids() {
            [] => return None,
            // One `preview_local_box` walk, reused by `preview_pivot_in`.
            [id] => {
                let box_ = self.session.preview_local_box(*id)?;
                (
                    Target::Layer(*id),
                    box_,
                    self.session.preview_world_transform(*id)?,
                    self.session.preview_pivot_in(*id, box_),
                )
            }
            many => {
                // Carried by the frame's transform, never a re-union of upright
                // bounds, which would re-shape as the set shears or turns.
                let _ = many;
                let (frame, box_) = self.selection_frame_box()?;
                (Target::Selection, box_, frame, None)
            }
        };
        let at = |h: Handle| {
            let (ux, uy) = h.unit();
            world
                * Point::new(
                    box_.min_x() + box_.width() * ux,
                    box_.min_y() + box_.height() * uy,
                )
        };
        Some(Handles {
            target,
            pivot: world * local_pivot.unwrap_or_else(|| box_.center()),
            world,
            corners: Handle::CORNERS.map(|h| (h, self.to_screen(at(h), rect, ppp))),
            locked: self.selection_locked(),
        })
    }

    /// Whether the selection holds an effectively locked layer, which refuses its
    /// handles, a move and its spacing bands. The keyboard nudge does not ask.
    pub(crate) fn selection_locked(&self) -> bool {
        let doc = &self.session.doc;
        self.session
            .selection
            .ids()
            .iter()
            .any(|id| ondin_core::is_effectively_locked(doc, *id))
    }

    /// Where the pivot marker sits on screen. Single selection only.
    fn pivot_screen(&self, rect: egui::Rect, ppp: f32) -> Option<(NodeId, egui::Pos2)> {
        let [id] = *self.session.selection.ids() else {
            return None;
        };
        let world = self.session.preview_world_transform(id)?;
        let local = self.session.preview_pivot(id)?;
        Some((id, self.to_screen(world * local, rect, ppp)))
    }

    /// Whether a press at `screen` grabs the pivot marker; asked ahead of the
    /// resize handles, or a pivot parked on a corner could never be grabbed back.
    fn on_pivot(&self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<NodeId> {
        // Drawn on a locked layer, but it does not move.
        if !self.show_pivot || self.mode != Mode::Normal || self.selection_locked() {
            return None;
        }
        self.pivot_screen(rect, ppp)
            .filter(|(_, at)| screen.distance(*at) <= PIVOT_PICK_PX)
            .map(|(id, _)| id)
    }

    /// The pivot a release at `world` should commit, snapped to the box's nine
    /// points within a few screen pixels; `None` (the centre) clears it.
    fn pivot_at(&self, id: NodeId, world: Point) -> Option<Pivot> {
        let local_box = self.session.preview_local_box(id)?;
        let to_world = self.session.preview_world_transform(id)?;
        let at = to_world.inverse() * world;

        // The pixel threshold in local units: by zoom and the basis column norms,
        // since a node can carry scale and shear.
        let c = to_world.as_coeffs();
        let zoom = self.points_per_world();
        let per_local_unit = |dx: f64, dy: f64| (dx * dx + dy * dy).sqrt().max(f64::EPSILON);
        let tol_x = PIVOT_SNAP_PX as f64 / (zoom * per_local_unit(c[0], c[1]));
        let tol_y = PIVOT_SNAP_PX as f64 / (zoom * per_local_unit(c[2], c[3]));
        let mut snapped = at;
        for (frac, axis_at, extent, min, tol) in [
            (&mut snapped.x, at.x, local_box.width(), local_box.x0, tol_x),
            (
                &mut snapped.y,
                at.y,
                local_box.height(),
                local_box.y0,
                tol_y,
            ),
        ] {
            for u in [0.0, 0.5, 1.0] {
                let candidate = min + extent * u;
                if (axis_at - candidate).abs() <= tol {
                    *frac = candidate;
                    break;
                }
            }
        }
        let node = self.session.display_node(id)?;
        // Whole percents through the origin fields' functions, so both store the
        // same class of value; the snap's 0/50/100 survive rounding.
        let (px, py) = origin_percent(snapped, local_box);
        origin_pivot(
            node.kind(),
            local_box,
            px.unwrap_or(50.0).round(),
            py.unwrap_or(50.0).round(),
        )
    }

    /// The name of the box position `at` (local space) is sitting on, if it is on
    /// one of the nine — what the drag badge reads out.
    fn pivot_label(&self, id: NodeId, at: Point) -> Option<&'static str> {
        let b = self.session.preview_local_box(id)?;
        let third = |v: f64, min: f64, extent: f64| {
            if extent <= f64::EPSILON {
                return Some(1);
            }
            let u = (v - min) / extent;
            [0.0_f64, 0.5, 1.0]
                .iter()
                .position(|c| (u - c).abs() < 1e-6)
        };
        let (col, row) = (
            third(at.x, b.x0, b.width())?,
            third(at.y, b.y0, b.height())?,
        );
        Some(match (row, col) {
            (0, 0) => "Top left",
            (0, 1) => "Top",
            (0, 2) => "Top right",
            (1, 0) => "Left",
            (1, 1) => "Centre",
            (1, 2) => "Right",
            (2, 0) => "Bottom left",
            (2, 1) => "Bottom",
            _ => "Bottom right",
        })
    }

    fn draw_canvas(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        // The mode's own artwork, re-installed every frame, downstream of every way
        // the state can change; before `render_inputs` takes the borrow.
        let ghost = self.image_ghost();
        self.session.set_chrome_ghost(ghost);

        // Split the borrow so the read-only inputs and the mutable renderer coexist.
        let camera = self.session.camera;
        let px = self.canvas_px;
        let backdrop = self.session.preview_canvas_background();
        let (doc, res, overrides) = self.session.render_inputs();
        if let Some(tex) = self
            .canvas
            .render(doc, res, overrides, &camera, px, backdrop)
        {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex, rect, uv, egui::Color32::WHITE);
        }
        // An adjusted picture is showing a preview: ask for the frame that
        // promotes it, or the soft picture stays until the pointer moves.
        if self.canvas.images().wants_frame() {
            ui.ctx().request_repaint();
        }
        // The eyedropper reads what was just drawn.
        if self.dropper_frame(ui, resp, rect, ppp) {
            ui.ctx().request_repaint();
        }

        // Overlays in paint order: the pixel grid over the artwork, then labels
        // and tracks under everything that answers the pointer, then hover under
        // the selection chrome.
        self.draw_pixel_grid(painter, rect, ppp);

        self.draw_layout_grids(painter, rect, ppp);

        self.draw_frame_labels(ui, painter, rect, ppp);

        self.draw_component_chrome(painter, rect, ppp);

        self.draw_grid_tracks(painter, rect, ppp);
        self.draw_spacing_bands(painter, rect, ppp);

        self.draw_hover_outline(ui, painter, rect, ppp, resp);

        self.draw_frame_drop_outline(ui, painter, rect, ppp, resp);
        self.draw_flex_landing(ui, painter, rect, ppp, resp);

        // During an Alt-drag, a dim outline of the original under the ghost copy.
        if self.alt_clone.is_some() {
            let staying = egui::Stroke::new(1.0, color::SELECT_DIM);
            for id in self.session.selection.ids() {
                if let Some(quad) = self.committed_quad(*id, rect, ppp) {
                    painter.add(egui::Shape::closed_line(quad.to_vec(), staying));
                }
            }
        }

        // Selection outlines from the preview; `chrome_hidden` covers this down to
        // the size badge, a set's `+` excepted.
        let offset = self.alt_clone_offset(rect, ppp);
        let stroke = egui::Stroke::new(1.2, color::SELECT);
        let chrome = !self.chrome_hidden();

        // Text line box, rail, then baselines, under the box and handles.
        if chrome {
            self.draw_text_line_boxes(painter, rect, ppp, offset);
            self.draw_text_rails(painter, rect, ppp, offset);
            self.draw_text_baselines(painter, rect, ppp, offset);
        }
        // A lone line is outlined along itself, never boxed; the kind is asked
        // separately, so a failed transform lookup draws nothing rather than a box.
        let lone = lone_line(&self.session).is_some();
        match self.line_ends(rect, ppp).filter(|_| chrome) {
            Some((_, a, b)) => {
                painter.line_segment([a + offset, b + offset], stroke);
                for at in [a, b] {
                    painter.rect_stroke(
                        egui::Rect::from_center_size(at + offset, egui::Vec2::splat(HANDLE_PX)),
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(HANDLE_STROKE_PX, color::SELECT),
                        egui::StrokeKind::Middle,
                    );
                }
            }
            None if chrome && !lone => {
                for id in self.session.selection.ids() {
                    if let Some(quad) = self.selection_quad(*id, rect, ppp) {
                        // Each box in its own layer's hue.
                        let hue = chrome_hue(&self.session.doc, *id);
                        painter.add(egui::Shape::closed_line(
                            quad.map(|p| p + offset).to_vec(),
                            egui::Stroke {
                                color: hue,
                                ..stroke
                            },
                        ));
                    }
                }
            }
            None => {}
        }

        // The rail handle: a filled dot where the type starts.
        if chrome
            && self.mode == Mode::Normal
            && let Some((_, at)) = self.rail_handle(rect, ppp)
        {
            painter.circle_filled(at + offset, HANDLE_PX * 0.5, color::SELECT);
        }

        // Transform handles (not while editing text): hollow, so the corner shows.
        if self.mode == Mode::Normal
            && chrome
            && let Some(h) = self.selection_handles(rect, ppp)
        {
            // A multi-selection's box, dimmer than the members' own outlines.
            let hue = self.selection_hue();
            if h.target == Target::Selection {
                let quad: Vec<egui::Pos2> = h.corners.iter().map(|(_, p)| *p + offset).collect();
                let dim = if hue == color::COMPONENT {
                    color::COMPONENT_DIM
                } else {
                    color::SELECT_DIM
                };
                painter.add(egui::Shape::closed_line(quad, egui::Stroke::new(1.0, dim)));
            }
            for (_, pos) in h.corners {
                painter.rect_stroke(
                    egui::Rect::from_center_size(pos + offset, egui::Vec2::splat(HANDLE_PX)),
                    egui::CornerRadius::ZERO,
                    egui::Stroke::new(HANDLE_STROKE_PX, hue),
                    egui::StrokeKind::Middle,
                );
            }
        }

        // The key layer, over the selection boxes.
        self.draw_key_outline(painter, rect, ppp);

        // Not under `chrome`: its hit route (`set_plus_at`) works either way.
        self.draw_set_plus(painter, rect, ppp);

        if self.mode == Mode::Normal && chrome {
            self.draw_pivot(ui, painter, rect, ppp, resp);
        }

        // One readout under the selection: a create drag's size, else the origin
        // fields while the crosshair is armed over one layer, else the size badge.
        if self.mode == Mode::Normal && chrome {
            let armed = self.show_pivot && self.session.selection.ids().len() == 1;
            if matches!(self.drag, Drag::Create { .. }) {
                let _ = self.draw_create_badge(ui, painter, rect, ppp, resp);
            } else if armed {
                let _ = self.origin_fields(ui, rect, ppp);
            } else {
                let _ = self.draw_size_badge(painter, rect, ppp, offset);
            }
        }

        // Outside the `chrome` guard: the chrome is hidden for that whole mode.
        let _ = self.crop_exit_pill(ui, rect, ppp);

        // Over the selection chrome, so no hairline crosses the number.
        self.draw_measures(ui, painter, rect, ppp, resp);

        // Snap lines for the live gesture; the pen's come from the hover position.
        if let Some(p) = resp
            .interact_pointer_pos()
            .or_else(|| resp.hover_pos().filter(|_| self.pen.is_some()))
        {
            let world = self.to_world(p, rect, ppp);
            let (shift, ctrl) =
                ui.input(|i| (i.modifiers.shift, i.modifiers.ctrl || i.modifiers.command));
            // `snap::Marks` for every arm (only a move makes gaps).
            let marks: snap::Marks = match &self.drag {
                Drag::Move { anchor } => self.snapped_move(world - *anchor, shift).1,
                Drag::Resize { target, handle } => {
                    self.snapped_resize(*target, *handle, world).1.into()
                }
                Drag::Points { id, grab, anchor } => {
                    let base = self.anchor_position(*id, *anchor);
                    self.snapped_point(*id, base, world - *grab, shift).1.into()
                }
                // A bend moves control points, which nothing snaps.
                Drag::Segment { id, at, t, grab } if !ctrl => {
                    let base = self.segment_position(*id, *at, *t);
                    self.snapped_point(*id, base, world - *grab, shift).1.into()
                }
                Drag::PointBox {
                    id, handle, grab, ..
                } => self.snapped_point_box(*id, *handle, *grab, world).1.into(),
                _ if self.pen.is_some() => self.pen_point(world, shift).1.into(),
                _ => snap::Marks::default(),
            };
            let guide_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(226, 108, 160));
            for g in marks.guides {
                let (a, b) = match g.axis {
                    Axis::Vertical => (Point::new(g.at, g.from), Point::new(g.at, g.to)),
                    Axis::Horizontal => (Point::new(g.from, g.at), Point::new(g.to, g.at)),
                };
                painter.line_segment(
                    [self.to_screen(a, rect, ppp), self.to_screen(b, rect, ppp)],
                    guide_stroke,
                );
            }
            // The gaps an equal-spacing snap made, drawn as Alt-hover measures.
            for m in marks.gaps {
                self.draw_measure(painter, rect, ppp, m);
            }
        }

        // As chrome, since the renderer's ghost can be culled with the root.
        if let Some(area) = self.create_outline_box(ui, rect, ppp, resp) {
            painter.rect_stroke(
                self.world_rect_to_screen(area, rect, ppp),
                egui::CornerRadius::ZERO,
                egui::Stroke::new(1.5, color::SELECT),
                egui::StrokeKind::Middle,
            );
        }

        if let Drag::Marquee { origin, .. } = &self.drag
            && let Some(p) = resp.interact_pointer_pos()
        {
            let end = self.to_world(p, rect, ppp);
            let area = KRect::new(
                origin.x.min(end.x),
                origin.y.min(end.y),
                origin.x.max(end.x),
                origin.y.max(end.y),
            );
            let r = self.world_rect_to_screen(area, rect, ppp);
            let [sr, sg, sb, _] = color::SELECT.to_srgba_unmultiplied();
            painter.rect_filled(
                r,
                egui::CornerRadius::ZERO,
                egui::Color32::from_rgba_unmultiplied(sr, sg, sb, 28),
            );
            painter.rect_stroke(
                r,
                egui::CornerRadius::ZERO,
                egui::Stroke::new(1.0, color::SELECT),
                egui::StrokeKind::Middle,
            );
        }

        // The in-progress pen path, drawn as the stroke it will become.
        if let Some(pen) = &self.pen {
            let ink = color_to_egui(PEN_STROKE_COLOR);
            let width = (PEN_STROKE_WIDTH * self.session.camera.zoom) as f32 / ppp;
            let seg = egui::Stroke::new(width.max(1.0), ink);
            let outline = tools::pen_outline(&pen.anchors, false);
            // One polyline, or a thick stroke shows notches at butt ends.
            for sub in self.flatten_to_screen(&outline, rect, ppp) {
                painter.add(sub.stroked(seg));
            }

            // The segment the next click would add, at half opacity, to where the
            // anchor would land, or to the first anchor when closing.
            let held = ui.input(|i| i.modifiers.shift);
            let pointer = resp
                .hover_pos()
                .map(|h| self.pen_point(self.to_world(h, rect, ppp), held).0);
            let closing = pointer.and_then(|w| {
                tools::pen_close_snap(
                    &pen.anchors,
                    w,
                    self.points_per_world(),
                    f64::from(PEN_PICK_PX),
                )
            });
            if let (Some(last), Some(to)) = (pen.anchors.last(), closing.or(pointer)) {
                let pending = tools::pen_outline(&[*last, PenAnchor::corner(to)], false);
                let ghost = egui::Stroke::new(seg.width, ink.gamma_multiply(0.45));
                for sub in self.flatten_to_screen(&pending, rect, ppp) {
                    painter.add(sub.stroked(ghost));
                }
            }

            // The handles of the anchor being shaped.
            if let Some(last) = pen.anchors.last()
                && !last.is_corner()
            {
                let a = self.to_screen(last.arriving(), rect, ppp);
                let b = self.to_screen(last.leaving(), rect, ppp);
                let at = self.to_screen(last.at, rect, ppp);
                let handle = egui::Stroke::new(1.0, color::SELECT_DIM);
                painter.line_segment([a, at], handle);
                painter.line_segment([at, b], handle);
                painter.circle_filled(a, HANDLE_DOT_PX, color::SELECT);
                painter.circle_filled(b, HANDLE_DOT_PX, color::SELECT);
            }
            // The anchors placed so far, always circles since nothing is settled.
            for (i, a) in pen.anchors.iter().enumerate() {
                let at = self.to_screen(a.at, rect, ppp);
                paint_anchor(painter, at, color::POINT);
                // The armed close target wears a ring.
                if i == 0 && closing.is_some() {
                    painter.circle_stroke(
                        at,
                        SNAP_RING_PX,
                        egui::Stroke::new(SNAP_RING_STROKE_PX, color::SELECT),
                    );
                }
            }

            // Another path's open end, which a press would join, ringed likewise.
            if closing.is_none()
                && let Some(p) = resp.hover_pos()
                && let Some(edit) = self.pen_join_target(self.to_world(p, rect, ppp))
                && let Some(end) = edit.endpoint()
            {
                let at = self.to_screen(end, rect, ppp);
                paint_anchor(painter, at, color::POINT);
                painter.circle_stroke(
                    at,
                    SNAP_RING_PX,
                    egui::Stroke::new(SNAP_RING_STROKE_PX, color::SELECT),
                );
            }
            ui.ctx().request_repaint();
        } else if self.drag.is_none()
            && !self.gesture_cancelled
            && let Some(p) = resp.hover_pos()
            // The standalone pen reaches any open end; the others via `pen_verb`.
            // Only with nothing in hand, as `draw_points` gates hover.
            && let Some(edit) = match self.tool {
                Tool::Pen => self.pen_resume_target(self.to_world(p, rect, ppp)),
                _ => match self.pen_verb(self.to_world(p, rect, ppp)) {
                    Some(PenVerb::Continue(edit)) => Some(*edit),
                    _ => None,
                },
            }
            && let Some(end) = edit.endpoint()
        {
            // An open endpoint the next press would resume.
            let at = self.to_screen(end, rect, ppp);
            paint_anchor(painter, at, color::POINT);
            painter.circle_stroke(
                at,
                SNAP_RING_PX,
                egui::Stroke::new(SNAP_RING_STROKE_PX, color::SELECT),
            );
        }

        self.draw_points(painter, rect, ppp, resp);

        // Image editing's crop window; not inside `draw_points`, which returns
        // unless the node tool has a path.
        self.draw_crop_box(painter, rect, ppp);

        // Text caret and selection highlight. The phase is asked unconditionally:
        // `CaretBlink` notices a caret move by being asked.
        let (now, focused) = ui.input(|i| (i.time, i.focused));
        let caret = self.text.as_mut().map(|s| s.caret_phase(now, focused));
        if let Some(session) = &self.text
            && let Some(world) = self.session.preview_world_transform(session.id)
        {
            // Several convex quads per line on a curve: `convex_polygon` mis-draws
            // a concave one.
            for q in session.editor.selection_quads() {
                painter.add(egui::Shape::convex_polygon(
                    self.poly(&q, world, rect, ppp),
                    egui::Color32::from_rgba_unmultiplied(109, 140, 217, 70),
                    egui::Stroke::NONE,
                ));
            }
            // Only the caret blinks; the highlight shows even unfocused.
            if caret.is_some_and(|(on, _)| on) {
                let caret_width = 1.5 / self.session.camera.zoom.max(f64::EPSILON);
                for q in session.editor.caret_quads(caret_width) {
                    painter.add(egui::Shape::convex_polygon(
                        self.poly(&q, world, rect, ppp),
                        color::ACCENT,
                        egui::Stroke::NONE,
                    ));
                }
            }
        }
        // Repaint at the next blink flip; `None` (unfocused) asks for nothing.
        if let Some((_, Some(after))) = caret {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(after));
        }

        self.draw_drop_indicator(ui, painter, rect, ppp);

        // Over everything.
        self.draw_rulers_and_guides(painter, rect, ppp, resp.hover_pos());

        if resp.dragged() {
            ui.ctx().request_repaint();
        }
    }

    /// The transform-origin marker: a crosshair on the point rotation and mirroring
    /// turn about. Editable while the Transform panel's toggle is on and one layer
    /// is selected; read-only on the hovered, unselected, undragged layer whose
    /// pivot has been moved (Select tool only); absent otherwise.
    fn draw_pivot(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        // While dragged the marker follows the pointer, snapped as the release will be.
        if let Drag::Pivot { id } = self.drag {
            let Some(p) = resp.interact_pointer_pos() else {
                return;
            };
            let Some(local) = self.pivot_at(id, self.to_world(p, rect, ppp)).map_or_else(
                || self.session.preview_local_box(id).map(|b| b.center()),
                |pv| {
                    self.session
                        .preview_local_box(id)
                        .map(|b| geometry::pivot_point(Some(pv), b))
                },
            ) else {
                return;
            };
            let Some(world) = self.session.preview_world_transform(id) else {
                return;
            };
            let at = self.to_screen(world * local, rect, ppp);
            paint_pivot(painter, at, Marker::Editable);
            if let Some(name) = self.pivot_label(id, local) {
                paint_pivot_badge(painter, at, name);
            }
            ui.ctx().request_repaint();
            return;
        }

        // Editable: the toggle is on and there is exactly one layer to own it.
        if self.show_pivot
            && let Some((id, at)) = self.pivot_screen(rect, ppp)
        {
            // The previewed pivot, not the committed one, so the badge tracks typing.
            let placed = self.session.previewed_pivot(id).flatten().is_some();
            paint_pivot(painter, at, Marker::Editable);
            if placed
                && let Some(local) = self.session.preview_pivot(id)
                && let Some(name) = self.pivot_label(id, local)
            {
                paint_pivot_badge(painter, at, name);
            }
            return;
        }

        // Read-only: hover only, on a placed pivot of a layer neither selected nor moving.
        if !self.drag.is_none() || !self.selecting() {
            return;
        }
        let Some(p) = resp.hover_pos() else { return };
        let Some(leaf) = self.pick_leaf(self.to_world(p, rect, ppp)) else {
            return;
        };
        let id = self.pick_preview(leaf, ui);
        if self.session.selection.contains(id) {
            return;
        }
        if !self
            .session
            .doc
            .get(id)
            .is_some_and(|n| n.pivot().is_some())
        {
            return;
        }
        let (Some(local), Some(world)) = (
            self.session.preview_pivot(id),
            self.session.preview_world_transform(id),
        ) else {
            return;
        };
        let at = self.to_screen(world * local, rect, ppp);
        paint_pivot(painter, at, Marker::ReadOnly);
        if let Some(name) = self.pivot_label(id, local) {
            paint_pivot_badge(painter, at, name);
        }
    }

    /// Image editing's pointer exit: a *Done* pill in the size badge's slot (free
    /// because `chrome_hidden` holds for the mode). "Done" because every drag
    /// commits on release. An `Area` like [`Self::origin_fields`], so egui routes
    /// the press itself (`overlay_claim`); upright, as a widget cannot rotate.
    fn crop_exit_pill(&mut self, ui: &mut egui::Ui, rect: egui::Rect, ppp: f32) -> Option<()> {
        /// The badge's own padding, and the hover wash over its ground.
        const PAD: egui::Vec2 = egui::vec2(9.0, 3.0);
        // A white wash: the ground is the accent, so there is no darker tier.
        let hover = egui::Color32::from_white_alpha(26);
        let (id, _) = self.edited_image()?;
        let (edge, _) = self.badge_placement(rect, ppp, egui::Vec2::ZERO)?;

        let galley = ui.painter().layout_no_wrap(
            "Done".to_owned(),
            egui::FontId::proportional(9.5),
            color::BADGE_INK,
        );
        let size = galley.size() + PAD * 2.0;
        // `HANDLE_PICK_PX` below the box, the size badge's gap, so the pill stays off the handles.
        let centre = badge_frame(edge, size, HANDLE_PICK_PX).origin;

        let mut done = false;
        egui::Area::new(crop_exit_layer(id).id)
            .order(crop_exit_layer(id).order)
            .fixed_pos(centre - size * 0.5)
            .show(ui.ctx(), |ui| {
                // Allocated: the `Area`'s own response never reports a hover.
                let (r, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                let radius = egui::CornerRadius::same(4);
                ui.painter().rect_filled(r, radius, color::BADGE);
                if resp.hovered() {
                    ui.painter().rect_filled(r, radius, hover);
                }
                ui.painter().galley(r.min + PAD, galley, color::BADGE_INK);
                // No pointing hand (§9.2): the wash above is the hover answer.
                done = resp.clicked();
            });
        if done {
            // Through `choose_tool`, as `Enter` and `Escape` do.
            self.choose_tool(Tool::Select);
        }
        Some(())
    }

    /// The transform origin's two numbers, replacing the size badge in its slot
    /// (`badge_placement` is shared) while the crosshair is armed. They live in an
    /// `egui::Area`, which shields the canvas from the press
    /// (`an_overlay_takes_the_press_from_the_canvas_beneath`). A lone line gets
    /// them too, with the degenerate axis dimmed ([`origin_percent`]).
    fn origin_fields(&mut self, ui: &mut egui::Ui, rect: egui::Rect, ppp: f32) -> Option<()> {
        let [id] = *self.session.selection.ids() else {
            return None;
        };
        let layer = origin_fields_layer(id);
        let (edge, _) = self.badge_placement(rect, ppp, egui::Vec2::ZERO)?;
        let box_ = self.session.preview_local_box(id)?;
        let at = self.session.preview_pivot_in(id, box_)?;
        let (px, py) = origin_percent(at, box_);

        // Two pills of one width centred on the slot, upright (a widget cannot be rotated).
        let pill = egui::vec2(ORIGIN_FIELD_W, crate::ui::BADGE_FIELD_H);
        let pair = egui::vec2(pill.x * 2.0 + ORIGIN_FIELD_GAP, pill.y);
        let centre = badge_frame(edge, pair, HANDLE_PICK_PX).origin;
        let top_left = centre - pair * 0.5;

        let mut edit: Option<(egui::Response, f64, f64)> = None;
        egui::Area::new(layer.id)
            .order(layer.order)
            .fixed_pos(top_left)
            .show(ui.ctx(), |ui| {
                // As tall as the pills: `horizontal` centres in the theme's 24pt
                // `interact_size`, which would drop them below the badge's line.
                ui.spacing_mut().interact_size.y = crate::ui::BADGE_FIELD_H;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ORIGIN_FIELD_GAP;
                    for (on_y, label, shown) in [(false, "X", px), (true, "Y", py)] {
                        // A degenerate axis has one position, so it is shown inert with a reason.
                        let mut v = shown.unwrap_or(50.0);
                        let scoped = ui.scope(|ui| {
                            if shown.is_none() {
                                ui.disable();
                            }
                            crate::ui::badge_field(
                                ui,
                                label,
                                ORIGIN_FIELD_W,
                                &mut v,
                                // Unclamped, like the marker: a pivot may sit outside its box.
                                crate::ui::Scrub::whole(0.5),
                                |d| d.suffix("%").custom_formatter(crate::ui::number(2)),
                            )
                        });
                        // A disabled `Ui` reports no hover, so the tooltip needs its
                        // own claimant.
                        if shown.is_none() {
                            ui.interact(
                                scoped.inner.rect,
                                ui.id().with(("degenerate-origin", on_y)),
                                egui::Sense::empty(),
                            )
                            .on_hover_text(if on_y {
                                "This layer has no height for the origin to sit along"
                            } else {
                                "This layer has no width for the origin to sit along"
                            });
                            continue;
                        }
                        let resp = scoped.inner.on_hover_text(if on_y {
                            "Transform origin, down the layer's own box — 50% is its centre"
                        } else {
                            "Transform origin, across the layer's own box — 50% is its centre"
                        });
                        // valve-gate-not-a-commit: `inspector::interacting` spelled inline; the commit is `edit_valve`'s.
                        if !resp.dragged()
                            && !resp.drag_stopped()
                            && !resp.lost_focus()
                            && !resp.changed()
                        {
                            continue;
                        }
                        let (x_pct, y_pct) = if on_y {
                            (px.unwrap_or(50.0), v)
                        } else {
                            (v, py.unwrap_or(50.0))
                        };
                        edit = Some((resp, x_pct, y_pct));
                    }
                });
            });
        // Out of the closure: the valve needs `&mut self`.
        let (resp, x_pct, y_pct) = edit?;
        let kind = self.session.display_node(id).map(|n| n.kind().clone())?;
        let pivot = origin_pivot(&kind, box_, x_pct, y_pct);
        self.edit_valve(&resp, Transaction(vec![Operation::SetPivot { id, pivot }]));
        Some(())
    }

    /// The edge a badge hangs from and the box whose numbers it quotes; shared by
    /// the size badge and the origin fields, which take turns in one slot. A set's
    /// edge comes off the oriented box `selection_frame_box` answers.
    fn badge_placement(
        &self,
        rect: egui::Rect,
        ppp: f32,
        offset: egui::Vec2,
    ) -> Option<(BadgeEdge, KRect)> {
        match self.session.selection.ids() {
            [] => None,
            [id] => {
                let quad = self.selection_quad(*id, rect, ppp)?;
                let local = self.session.preview_local_box(*id)?;
                // The edge from the quad itself, so shear costs nothing.
                Some((badge_edge(&quad.map(|p| p + offset))?, local))
            }
            many => {
                // The box the handles sit on; the numbers are its extent.
                let _ = many;
                let (frame, box_) = self.selection_frame_box()?;
                let quad = [
                    (box_.x0, box_.y0),
                    (box_.x1, box_.y0),
                    (box_.x1, box_.y1),
                    (box_.x0, box_.y1),
                ]
                .map(|(x, y)| self.to_screen(frame * Point::new(x, y), rect, ppp) + offset);
                Some((badge_edge(&quad)?, box_))
            }
        }
    }

    /// The `W × H` pill under the selection.
    ///
    /// A single selection reports its own width and height; a set the oriented
    /// box's extent. The pill turns with the shape. A locked layer's badge carries
    /// the filled lock; `offset` moves it onto the Alt-drag copy
    /// ([`Self::alt_clone_offset`]).
    fn draw_size_badge(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        offset: egui::Vec2,
    ) -> Option<()> {
        // No badge on a lone line; refused on the kind, so a failed transform
        // lookup cannot fail open.
        if lone_line(&self.session).is_some() {
            return None;
        }

        let ids = self.session.selection.ids();
        let (edge, box_) = self.badge_placement(rect, ppp, offset)?;
        // W × H of the layer, not of the edge the pill landed on.
        let label = size_label(box_);
        // `any`: if any member refuses to move, the set does.
        let locked = ids
            .iter()
            .any(|id| ondin_core::is_effectively_locked(&self.session.doc, *id));

        paint_size_pill(painter, edge, label, locked, self.selection_hue());
        Some(())
    }

    /// The box the in-flight create drag would commit, in world units; one answer
    /// for the outline and the pill. `None` with no create, no pointer, or the
    /// Line tool.
    fn create_outline_box(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) -> Option<KRect> {
        let Drag::Create { start, shape, .. } = &self.drag else {
            return None;
        };
        if *shape == Tool::Line {
            return None;
        }
        let p = resp.interact_pointer_pos()?;
        let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let (from, to) = self.create_box(*shape, *start, self.to_world(p, rect, ppp), shift, alt);
        Some(KRect::new(
            from.x.min(to.x),
            from.y.min(to.y),
            from.x.max(to.x),
            from.y.max(to.y),
        ))
    }

    /// The size pill under a shape being *drawn*: `draw_size_badge` reads
    /// `selection.ids()`, and a shape being drawn is not selected until it commits.
    fn draw_create_badge(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) -> Option<()> {
        let box_ = self.create_outline_box(ui, rect, ppp, resp)?;
        let label = size_label(box_);
        // Upright, so the edge is stated rather than searched for.
        let screen = self.world_rect_to_screen(box_, rect, ppp);
        let edge = BadgeEdge {
            mid: egui::pos2(screen.center().x, screen.bottom()),
            dir: egui::vec2(1.0, 0.0),
            out: egui::vec2(0.0, 1.0),
        };
        paint_size_pill(painter, edge, label, false, color::BADGE);
        Some(())
    }

    /// Announce a drag of files over the window and what a release would do: place
    /// a new layer, or replace the picture of the shape it lands on. It asks the
    /// release's own questions (`drop_point`, `image_drop_target`). The border goes
    /// round the target frame, falling back to [`Self::canvas_inner`]. The count is
    /// of files, since bytes are not read until the drop.
    fn draw_drop_indicator(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        /// The border drawn inside the canvas edge, when there is no frame to
        /// put one round instead.
        const BORDER: f32 = 2.0;
        let (files, svgs) = ui.input(|i| {
            let hovered = &i.raw.hovered_files;
            (
                hovered.len(),
                hovered.iter().filter(|f| hovers_svg(f)).count(),
            )
        });
        if files == 0 {
            return;
        }
        // One gate for outline and words, `drop_images_at`'s own: a single picture on
        // the shape. An SVG makes layers instead.
        let at = self.drop_point(ui.ctx());
        let replacing = at
            .filter(|_| files == 1 && svgs == 0)
            .and_then(|p| self.image_drop_target(Some(p)));
        // The hover outline's weight and hue, so it does not read as selected.
        let anchor = match replacing {
            Some((id, _)) => {
                let stroke = egui::Stroke::new(3.0, color::SELECT);
                self.stroke_outlines(painter, id, rect, ppp, stroke, self.outline_view(stroke));
                self.session.preview_world_bounds(id).map(|b| b.center())
            }
            None => None,
        };
        // Where the picture goes: the shape, the drop point, or the view centre.
        let world = anchor.or(at).unwrap_or_else(|| self.viewport_centre());
        // `canvas_inner`, not `rect`, so neither mark crosses the rulers.
        let inner = self.canvas_inner(rect);
        let frame = self
            .target_artboard(world)
            .and_then(|(id, _)| self.committed_quad(id, rect, ppp))
            .filter(|quad| inner.contains_rect(egui::Rect::from_points(quad)));
        match frame {
            Some(quad) => {
                painter.add(egui::Shape::closed_line(
                    quad.to_vec(),
                    egui::Stroke::new(3.0, color::SELECT),
                ));
            }
            None => {
                painter.rect_stroke(
                    inner,
                    egui::CornerRadius::ZERO,
                    egui::Stroke::new(BORDER, color::SELECT),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let name = replacing
            .and_then(|(id, _)| self.session.doc.get(id))
            .map(|n| n.name().to_string());
        paint_drop_label(
            painter,
            inner,
            self.to_screen(world, rect, ppp),
            &drop_label(files, svgs, name.as_deref()),
        );
    }

    /// Flatten a world-space path to screen points at a screen-pixel tolerance.
    /// Each [`Polyline`] records whether its subpath closes: `Shape::line` leaves
    /// the closing edge undrawn.
    fn flatten_to_screen(
        &self,
        path: &ondin_core::kurbo::BezPath,
        rect: egui::Rect,
        ppp: f32,
    ) -> Vec<Polyline> {
        use ondin_core::kurbo::PathEl;
        const SCREEN_TOLERANCE: f64 = 0.25;
        let tolerance = SCREEN_TOLERANCE / self.session.camera.zoom.max(f64::EPSILON);
        let mut out: Vec<Polyline> = Vec::new();
        ondin_core::kurbo::flatten(path, tolerance, |el| match el {
            PathEl::MoveTo(p) => out.push(Polyline {
                pts: vec![self.to_screen(p, rect, ppp)],
                closed: false,
            }),
            PathEl::LineTo(p) => {
                if let Some(sub) = out.last_mut() {
                    sub.pts.push(self.to_screen(p, rect, ppp));
                }
            }
            PathEl::ClosePath => {
                if let Some(sub) = out.last_mut() {
                    sub.closed = true;
                }
            }
            _ => {}
        });
        out
    }

    /// A world-space rect as an axis-aligned screen rect.
    fn world_rect_to_screen(&self, b: KRect, rect: egui::Rect, ppp: f32) -> egui::Rect {
        egui::Rect::from_min_max(
            self.to_screen(Point::new(b.min_x(), b.min_y()), rect, ppp),
            self.to_screen(Point::new(b.max_x(), b.max_y()), rect, ppp),
        )
    }

    /// A local polygon's corners in screen space, correct under rotation (§15 D407).
    fn poly(&self, ps: &[Point], world: Affine, rect: egui::Rect, ppp: f32) -> Vec<egui::Pos2> {
        ps.iter()
            .map(|p| self.to_screen(world * *p, rect, ppp))
            .collect()
    }
}

/// The layer name a picture takes among siblings: the one it arrived with (a
/// filename is identity), or the next free `"Image N"` for a paste.
///
/// `CreateNode { name: None }` would number from the kind's stem ("Rectangle 4"),
/// but an image is a fill on a Rect; `"Image"` stays out of core's
/// `GENERATED_NAMES`, which inverts `NodeKind::default_name`.
fn image_layer_name(doc: &Document, parent: NodeId, given: Option<String>) -> String {
    given.unwrap_or_else(|| ondin_core::free_name(&doc.child_names(parent, &[]), "Image", 1))
}

/// Image editing's chrome: the dashed crop window, its four hollow corner
/// handles, and the outline of the whole picture behind it when there is one.
/// Both quads are screen corners, clockwise from the top-left.
fn crop_chrome(window: &[egui::Pos2], picture: Option<&[egui::Pos2]>) -> Vec<egui::Shape> {
    let mut out = Vec::new();
    if let Some(picture) = picture {
        out.push(egui::Shape::closed_line(
            picture.to_vec(),
            egui::Stroke::new(1.0, color::SELECT_DIM),
        ));
    }
    // Closed by repeating the first corner: `dashed_line` takes a polyline.
    let mut closed = window.to_vec();
    if let Some(first) = closed.first().copied() {
        closed.push(first);
    }
    out.extend(egui::Shape::dashed_line(
        &closed,
        egui::Stroke::new(CROP_STROKE_PX, color::SELECT),
        CROP_DASH_PX,
        CROP_GAP_PX,
    ));
    for at in window {
        out.push(egui::Shape::rect_stroke(
            egui::Rect::from_center_size(*at, egui::Vec2::splat(HANDLE_PX)),
            egui::CornerRadius::ZERO,
            egui::Stroke::new(HANDLE_STROKE_PX, color::SELECT),
            egui::StrokeKind::Middle,
        ));
    }
    out
}

/// A path anchor's mark: a filled disc with a hairline rim, [`POINT_PX`] across.
///
/// One drawing for both tools, so a path does not change appearance when it
/// commits. The pen's are always circles: the node tool's square marks a corner.
fn paint_anchor(painter: &egui::Painter, at: egui::Pos2, fill: egui::Color32) {
    let radius = POINT_PX * 0.5;
    painter.circle_filled(at, radius, fill);
    painter.circle_stroke(
        at,
        radius,
        egui::Stroke::new(POINT_STROKE_PX, color::SELECT),
    );
}

/// The marker: one Phosphor crosshair in the weight [`Marker`] picks, no ground.
fn paint_pivot(painter: &egui::Painter, at: egui::Pos2, marker: Marker) {
    painter.text(
        at,
        egui::Align2::CENTER_CENTER,
        marker.glyph(),
        theme::icon_font(PIVOT_GLYPH_PX),
        color::SELECT,
    );
}

/// The name of the box position the marker is on, above it: "very nearly the
/// top-left corner" looks identical to "the top-left corner" and behaves differently.
fn paint_pivot_badge(painter: &egui::Painter, at: egui::Pos2, name: &str) {
    const PAD: egui::Vec2 = egui::vec2(6.0, 2.0);
    let font = egui::FontId::proportional(9.5);
    let galley = painter.layout_no_wrap(name.to_owned(), font, color::BADGE_INK);
    let box_ = egui::Rect::from_center_size(
        at - egui::vec2(0.0, PIVOT_GLYPH_PX * 0.5 + 9.0),
        galley.size() + PAD * 2.0,
    );
    painter.rect_filled(box_, egui::CornerRadius::same(4), color::BADGE);
    painter.galley(box_.min + PAD, galley, color::BADGE_INK);
}

/// The desktop cursor in physical screen pixels, or `None` off Windows.
///
/// The one thing a Windows file drag will not tell us, and the app's only Win32
/// call. Physical pixels, like `Window::inner_position`
/// ([`screen_pos_of_desktop_px`]). On a locked desktop it fails, and `None` puts
/// the drop on the viewport centre.
#[cfg(windows)]
fn os_cursor_desktop_px() -> Option<(f32, f32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `GetCursorPos` writes a `POINT` through the pointer it is given and
    // touches nothing else; `point` is a live, correctly typed, exclusively
    // borrowed local for the length of the call.
    let ok = unsafe { GetCursorPos(&mut point) };
    (ok != 0).then_some((point.x as f32, point.y as f32))
}

/// Other platforms' drags carry their point, so `drop_point` reads it off the events.
#[cfg(not(windows))]
fn os_cursor_desktop_px() -> Option<(f32, f32)> {
    None
}

/// A desktop pixel in egui's screen space — [`os_cursor_screen_pos`]'s arithmetic.
/// `inner_rect` is the client area in points. The physical reading is scaled
/// first and the origin subtracted after; the other order is right only at 100%.
fn screen_pos_of_desktop_px(
    desktop_px: (f32, f32),
    inner_rect: egui::Rect,
    pixels_per_point: f32,
) -> egui::Pos2 {
    egui::pos2(desktop_px.0, desktop_px.1) / pixels_per_point - inner_rect.min.to_vec2()
}

/// Where the pointer really is in egui's screen space, asked of the OS. `None`
/// off Windows, and with no `inner_rect` (a minimized window takes no drop).
fn os_cursor_screen_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    let desktop_px = os_cursor_desktop_px()?;
    let inner_rect = ctx.input(|i| i.viewport().inner_rect)?;
    Some(screen_pos_of_desktop_px(
        desktop_px,
        inner_rect,
        ctx.pixels_per_point(),
    ))
}

/// What releasing the drag would do, as the future tense of `drop_images_at`'s
/// messages. It names the shape in full, since the outline can be off screen and
/// a destructive drop must not surprise.
fn drop_label(files: usize, svgs: usize, replacing: Option<&str>) -> String {
    match (files, replacing) {
        (_, Some(name)) => format!("Replace image in {name}"),
        (1, None) if svgs == 1 => "Import SVG".to_string(),
        (n, None) if svgs == n => format!("Import {n} SVGs"),
        (n, None) if svgs > 0 => format!("Place {n} files"),
        (1, None) => "Place image".to_string(),
        (n, None) => format!("Place {n} images"),
    }
}

/// Whether a file hovering over the window is SVG, by its name or its MIME type: the
/// bytes arrive only with the drop.
fn hovers_svg(file: &egui::HoveredFile) -> bool {
    file.mime == "image/svg+xml"
        || file
            .path
            .as_deref()
            .and_then(|p| p.extension())
            .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

/// The drop's pill, laid out on `at` but kept inside `canvas`.
///
/// Kept inside by hand: [`egui::Rect::clamp`] panics on a rect narrower than the
/// pill.
fn paint_drop_label(painter: &egui::Painter, canvas: egui::Rect, at: egui::Pos2, text: &str) {
    const PAD: egui::Vec2 = egui::vec2(9.0, 5.0);
    const GAP: f32 = 8.0;
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(11.0),
        color::BADGE_INK,
    );
    let size = galley.size() + PAD * 2.0;
    // Half the room to move in, never negative, so the clamp's bounds cannot cross.
    let room = ((canvas.size() - size) * 0.5 - egui::Vec2::splat(GAP)).max(egui::Vec2::ZERO);
    let c = canvas.center();
    let at = egui::pos2(
        at.x.clamp(c.x - room.x, c.x + room.x),
        at.y.clamp(c.y - room.y, c.y + room.y),
    );
    let box_ = egui::Rect::from_center_size(at, size);
    painter.rect_filled(box_, egui::CornerRadius::same(4), color::BADGE);
    painter.galley(box_.min + PAD, galley, color::BADGE_INK);
}

/// Whether an arrow key in a live text session moves the **caret**, or belongs
/// to the keymap chord that shares it.
///
/// `Alt`+arrows are letter spacing and line height (`docs/shortcuts.md` §10),
/// resolved by `input::resolve`; without this gate both claimants fire. It yields
/// to `Alt` without `Ctrl`, matching the keymap's `alt_only`: a plain
/// `!modifiers.alt` would hand `Ctrl+Alt+←` to neither
/// (`the_two_claimants_of_an_arrow_key_never_both_fire`).
fn arrow_moves_caret(modifiers: egui::Modifiers) -> bool {
    !(modifiers.alt && !modifiers.command)
}

/// The frame `id` **is**, or the nearest one above it; `None` on the bare canvas.
///
/// Self first, then ancestors: a selected frame answers with itself.
fn enclosing_artboard(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut cursor = Some(id);
    while let Some(c) = cursor {
        if matches!(
            doc.get(c).map(|n| n.kind()),
            Some(NodeKind::Artboard { .. })
        ) {
            return Some(c);
        }
        cursor = doc.get(c).and_then(|n| n.parent());
    }
    None
}

/// The parent and index a panel *Paste* aims at: the slot immediately **above**
/// `id` among its siblings (`+ 1`, as `Node::children` ascends in z-order).
/// `None` for the root and a row whose parent has gone.
pub(crate) fn sibling_slot_above(doc: &Document, id: NodeId) -> Option<(NodeId, usize)> {
    let parent = doc.get(id)?.parent()?;
    let at = doc.get(parent)?.children().iter().position(|c| *c == id)?;
    Some((parent, at + 1))
}

/// Whether `id`'s canvas tag is withheld because it sits **strictly** inside a
/// component (a main or an instance root answers for its own position):
/// - **anything inside an instance**: the instance's tag is its label, and an
///   inner tag would be a pick past its group-like picking;
/// - **an instance inside a main**, which picks like a group and never needed the
///   tag as a handle. A plain frame inside a main keeps its tag: occupied, it is
///   the frame's one handle besides its edge.
fn tag_withheld(doc: &Document, id: NodeId) -> bool {
    let instance = ondin_core::component::instance_root(doc, id) == Some(id);
    let mut at = doc.get(id).and_then(|n| n.parent());
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if n.link().is_some() || (instance && n.component()) {
            return true;
        }
        at = n.parent();
    }
    false
}

/// The colour a component's selection chrome takes: `COMPONENT` for a main or an
/// instance root, `SELECT` for everything else, sets and inner layers included.
pub(crate) fn chrome_hue(doc: &Document, id: NodeId) -> egui::Color32 {
    let component = doc.get(id).is_some_and(|n| n.component())
        || ondin_core::component::instance_root(doc, id) == Some(id);
    if component {
        color::COMPONENT
    } else {
        color::SELECT
    }
}

/// The one selection that gets **endpoint handles instead of a transform box**: a
/// lone `Line`, with its far end. A line inside a multi-selection keeps the union
/// box. This answer suppresses the transform chrome, which all hangs off
/// [`OndinApp::selection_handles`]. Read through the preview layer.
pub(crate) fn lone_line(session: &EditorSession) -> Option<(NodeId, Point)> {
    let [id] = *session.selection.ids() else {
        return None;
    };
    match *session.display_node(id)?.kind() {
        NodeKind::Line { end } => Some((id, end)),
        _ => None,
    }
}

/// The local origin a pasted text layer takes so its box is centred on `centre`.
/// `TextLayout::origin` is non-zero for the default text (`default_text_parts`
/// trims `CapToBaseline`); centring on `size` alone leaves every paste low.
fn centred_paste_origin(centre: Point, size: ondin_core::kurbo::Size, box_offset: Vec2) -> Vec2 {
    (centre - size.to_vec2() / 2.0 - box_offset).to_vec2()
}

/// The sizing a pasted text layer needs when its one auto-width line would outrun
/// the page, or `None` when auto width fits. `AutoHeight`, not `Fixed`; exactly
/// fits stays auto (`>=` would wrap a paste that fits).
fn wrapped_paste_sizing(shaped_width: f64, available: f64) -> Option<ondin_core::TextSizing> {
    (shaped_width > available).then_some(ondin_core::TextSizing::AutoHeight(available))
}

/// An owned copy of `s`, or `None` when empty: `Context::copy_text("")` still
/// counts as a copy and would clear the user's clipboard.
fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// Rebuild the create gesture from the one we took by value, so `create_tx`
/// (which reads `self.drag`) can see it during the commit step.
fn drag_create_again(drag: &Drag, id: NodeId, start: Point) -> Drag {
    match drag {
        Drag::Create {
            parent,
            parent_world,
            shape,
            sides,
            last,
            ..
        } => Drag::Create {
            id,
            parent: *parent,
            parent_world: *parent_world,
            start,
            shape: *shape,
            // The live count, not the tool default, or release drops the mid-drag adjustment.
            sides: *sides,
            last: *last,
        },
        _ => Drag::None,
    }
}

/// The cursor over nothing: an arrow, or the Scale tool's own glyph, which shares
/// every gesture with Select and so needs the cursor as its only warning.
fn rest_cursor(tool: Tool) -> (egui::CursorIcon, Option<Bitmap>) {
    match tool {
        Tool::Scale => (egui::CursorIcon::Default, Some(Bitmap::Scale)),
        _ => (egui::CursorIcon::Default, None),
    }
}

/// The stock resize cursor for a handle facing `out` **on screen**.
///
/// Snapped to the nearest eighth of a turn; the direction is the handle turned
/// through the node's rotation ([`Handles::screen_direction`]).
pub(crate) fn resize_icon(out: egui::Vec2) -> egui::CursorIcon {
    if out.length_sq() < f32::EPSILON {
        return egui::CursorIcon::Default;
    }
    // Eighths of a turn from east, clockwise (screen y points down); opposite
    // handles share a cursor.
    let octant = (out.y.atan2(out.x) / (std::f32::consts::TAU / 8.0)).round() as i32;
    match octant.rem_euclid(4) {
        0 => egui::CursorIcon::ResizeHorizontal,
        1 => egui::CursorIcon::ResizeNwSe,
        2 => egui::CursorIcon::ResizeVertical,
        _ => egui::CursorIcon::ResizeNeSw,
    }
}

/// The angle a badge's label is laid at: along its edge, turned end-for-end if
/// upside down. Half turns, not quarters, keep it parallel to the edge.
fn badge_text_angle(dir: egui::Vec2) -> f32 {
    const HALF: f32 = std::f32::consts::PI;
    let a = dir.angle();
    a - (a / HALF).round() * HALF
}

/// The layer the origin fields for `id` live in, shared by `origin_fields` and
/// `wheel_input`. Keyed on the node, so a half-typed value cannot carry across a
/// selection change.
fn origin_fields_layer(id: NodeId) -> egui::LayerId {
    egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new(("origin-fields", id)),
    )
}

/// The layer image editing's *Done* pill lives in; the pill and the wheel handler
/// must agree on the id, as for [`origin_fields_layer`].
fn crop_exit_layer(id: NodeId) -> egui::LayerId {
    egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("crop-exit", id)))
}

/// Width of one origin pill, and the gap between the pair.
///
/// One number for both, so `X` and `Y` match (see [`crate::ui::badge_field`]). It
/// leaves 42pt for the digits, the widest being `-31.25%` at 41.44pt.
const ORIGIN_FIELD_W: f32 = 69.0;
const ORIGIN_FIELD_GAP: f32 = 5.0;

/// Below this an axis has no extent for the origin fields to be a percentage *of*.
///
/// The threshold `geometry::pivot_for` divides by, so both agree on degeneracy.
const ORIGIN_MIN_EXTENT: f64 = 1e-9;

/// The transform origin as a percentage of the node's own box, per axis.
///
/// A percentage rather than local units: the stored [`Pivot`] is a fraction for an
/// authored box and an absolute point for an emergent one, and
/// `geometry::pivot_for` decides storage. `None` on an axis with no extent (a flat
/// line), which the caller dims.
fn origin_percent(at: Point, box_: KRect) -> (Option<f64>, Option<f64>) {
    let axis = |v: f64, min: f64, extent: f64| {
        (extent > ORIGIN_MIN_EXTENT).then(|| (v - min) / extent * 100.0)
    };
    (
        axis(at.x, box_.x0, box_.width()),
        axis(at.y, box_.y0, box_.height()),
    )
}

/// The [`Pivot`] the origin fields' two percentages ask for, or `None` for the
/// box's centre, which is the absence of a pivot ([`geometry::pivot_placed_at`]).
///
/// Both axes at 50 clears the field, as the marker dragged back to the middle
/// does. An axis with no extent contributes its own single position.
fn origin_pivot(kind: &NodeKind, box_: KRect, x_pct: f64, y_pct: f64) -> Option<Pivot> {
    let axis = |pct: f64, min: f64, extent: f64| {
        if extent > ORIGIN_MIN_EXTENT {
            min + extent * (pct / 100.0)
        } else {
            min
        }
    };
    let at = Point::new(
        axis(x_pct, box_.x0, box_.width()),
        axis(y_pct, box_.y0, box_.height()),
    );
    geometry::pivot_placed_at(kind, box_, at)
}

/// What the size pill says about a box: whole units, `W × H`.
///
/// Whole units are a report; the W/H fields hold the exact value. One function for
/// a selected and a drawn shape, so the number does not twitch on release.
fn size_label(box_: KRect) -> String {
    format!("{} × {}", box_.width().round(), box_.height().round())
}

/// Paint the size pill under `oriented`, laid out in `frame`.
///
/// Shared by a selection's badge and a shape being drawn. `hue` is the pill's
/// ground (`color::BADGE`, or `color::COMPONENT`) and picks the ink.
fn paint_size_pill(
    painter: &egui::Painter,
    edge: BadgeEdge,
    label: String,
    locked: bool,
    hue: egui::Color32,
) {
    /// Gap below the box, padding inside the pill, and the lock-to-text gap. The
    /// gap cannot go below `HANDLE_PICK_PX`, or the pill covers a handle's grab area.
    const GAP: f32 = HANDLE_PICK_PX;
    const PAD: egui::Vec2 = egui::vec2(7.0, 3.0);
    const LOCK_GAP: f32 = 5.0;
    const LOCK_PT: f32 = 10.0;

    let ink = if hue == color::COMPONENT {
        color::COMPONENT_INK
    } else {
        color::BADGE_INK
    };
    let galley = painter.layout_no_wrap(label, egui::FontId::proportional(9.5), ink);
    let lock_w = if locked { LOCK_PT + LOCK_GAP } else { 0.0 };
    let size = egui::vec2(
        galley.size().x + lock_w + PAD.x * 2.0,
        galley.size().y + PAD.y * 2.0,
    );

    let pill = badge_frame(edge, size, GAP);
    painter.add(egui::Shape::convex_polygon(
        pill.rounded_rect(size, 4.0),
        hue,
        egui::Stroke::NONE,
    ));

    let mut x = -size.x * 0.5 + PAD.x;
    if locked {
        crate::ui::paint_lock_filled(
            painter,
            pill.to_screen(egui::pos2(x + LOCK_PT * 0.5, 0.0)),
            LOCK_PT,
            ink,
            pill.angle,
        );
        x += LOCK_PT + LOCK_GAP;
    }
    painter.add(
        egui::epaint::TextShape::new(
            pill.to_screen(egui::pos2(x, -size.y * 0.5 + PAD.y)),
            galley,
            ink,
        )
        .with_angle(pill.angle),
    );
}

/// Where the size badge sits: laid along `edge`, pushed `gap` clear of it.
///
/// Half the pill's height is the whole offset only because the label is parallel
/// to the edge ([`badge_text_angle`] reduces by half turns); with quarter turns
/// it would be the half-extent projected onto `edge.out`.
fn badge_frame(edge: BadgeEdge, pill: egui::Vec2, gap: f32) -> Frame2 {
    Frame2::rotated(
        badge_text_angle(edge.dir),
        edge.mid + edge.out * (gap + pill.y * 0.5),
    )
}

/// The edge of a selection's outline a badge hangs from ([`badge_edge`] chooses
/// it, [`badge_frame`] places a pill on it): midpoint, direction, and which way is
/// *out* of the shape.
#[derive(Clone, Copy)]
struct BadgeEdge {
    mid: egui::Pos2,
    /// Along the edge, unit length. The pill's text is laid parallel to this,
    /// through [`badge_text_angle`], which only ever flips it end-for-end.
    dir: egui::Vec2,
    /// The outward normal, unit length — the direction the pill is pushed in.
    out: egui::Vec2,
}

/// Which edge of `quad` the badge hangs from: the one whose outward normal points
/// most steeply down the screen.
///
/// Chosen from the quad, not the layer's angle, so a sheared box works. The normal
/// rather than the lowest midpoint: it switches at 45° whatever the aspect ratio,
/// which protects the label's angle. The pill hangs off the edge's midpoint, so it
/// can sit beside a lower corner. At exactly 45° the first edge found wins
/// (`the_badge_label_lies_along_its_edge_at_every_angle`). `None` for a degenerate
/// outline.
fn badge_edge(quad: &[egui::Pos2; 4]) -> Option<BadgeEdge> {
    let centroid = quad.iter().fold(egui::Vec2::ZERO, |a, p| a + p.to_vec2()) / 4.0;
    let mut best: Option<BadgeEdge> = None;
    for i in 0..4 {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        let along = b - a;
        if along.length() < 1e-6 {
            continue;
        }
        let mid = a + along * 0.5;
        // Perpendicular, turned to face away from the middle of the shape.
        let mut out = egui::vec2(-along.y, along.x).normalized();
        if out.dot(mid.to_vec2() - centroid) < 0.0 {
            out = -out;
        }
        // Screen y points down, so the largest `out.y` faces most downward; a tie keeps the first.
        if best.is_none_or(|w| out.y > w.out.y) {
            best = Some(BadgeEdge {
                mid,
                dir: along.normalized(),
                out,
            });
        }
    }
    best
}

/// `corner` pulled onto the diagonal from `start`, so the box it makes with
/// `start` is square — a Shift-drawn rect, circle or regular polygon.
///
/// The larger extent wins, so the shape encloses the pointer; each axis keeps its
/// sign.
fn squared_corner(start: Point, corner: Point) -> Point {
    let d = corner - start;
    let side = d.x.abs().max(d.y.abs());
    Point::new(start.x + side.copysign(d.x), start.y + side.copysign(d.y))
}

/// The two ends of a shape drawn from its centre: `corner` and its mirror through
/// `start` (an Alt-drawn shape). `Alt`+`Shift` squares first, then mirrors; the
/// other order centres the square off the press.
fn centred_ends(start: Point, corner: Point) -> (Point, Point) {
    (start - (corner - start), corner)
}

/// The box a *click* with a shape tool makes: [`tools::CLICK_CREATE_SIZE`]
/// square, with the click at its top-left corner, or — under Alt — at its centre.
///
/// Alt only moves the box: a click authors no extent, so doubling it (as a drag's
/// Alt does) would give a 200-unit shape. Half a side, doubled back by
/// [`centred_ends`], keeps one rule.
fn click_box(start: Point, alt: bool) -> (Point, Point) {
    let reach = tools::CLICK_CREATE_SIZE * if alt { 0.5 } else { 1.0 };
    let corner = start + Vec2::new(reach, reach);
    match alt {
        true => centred_ends(start, corner),
        false => (start, corner),
    }
}

/// What a click selects, given the modifiers and the group chain above the leaf:
/// the whole click-to-select policy as a table. After the `Ctrl`-only arm:
///
/// - no groups above the leaf: the leaf;
/// - `Ctrl+Alt` with two or more groups: one level into the outermost;
/// - `Ctrl+Alt` with exactly one: one level in is the leaf;
/// - no modifier: the outermost group.
fn pick_from_chain(ctrl: bool, alt: bool, chain: &[NodeId], leaf: NodeId) -> NodeId {
    // `Ctrl` alone means this exact thing, the deepest layer; the chain is unread.
    if ctrl && !alt {
        return leaf;
    }
    match (ctrl && alt, chain) {
        (_, []) => leaf,
        (true, [_outermost, next, ..]) => *next,
        (true, _) => leaf,
        (false, [outermost, ..]) => *outermost,
    }
}

/// The nearest group (or boolean) above `id`, if any — the container a move may
/// not take `id` out of ([`OndinApp::move_destination`]).
fn group_fence(doc: &ondin_core::Document, id: NodeId) -> Option<NodeId> {
    let mut up = doc.get(id)?.parent();
    while let Some(p) = up {
        let node = doc.get(p)?;
        if matches!(node.kind(), NodeKind::Group | NodeKind::Boolean { .. }) {
            return Some(p);
        }
        up = node.parent();
    }
    None
}

/// Whether frame `f` is one of `moving`, or inside one — the frames a move can
/// never land in (`OndinApp::moving_with`).
fn within_moving(
    doc: &ondin_core::Document,
    f: NodeId,
    moving: &std::collections::HashSet<NodeId>,
) -> bool {
    std::iter::successors(Some(f), |a| doc.get(*a).and_then(|n| n.parent()))
        .any(|a| moving.contains(&a))
}

/// What a move says when a main dropped over a component set of another kind was
/// kept out of it (one kind per set). The layers panel's drop says the same.
pub(crate) const SET_OF_ANOTHER_KIND: &str =
    "Not added to the set — a component set holds one kind of layer, and its variants are another";

/// What a move says when mains of two kinds dropped together over a component
/// set were kept out of it — an empty set fits either alone, and taking both
/// would make it two kinds (`variant::takes_together`).
pub(crate) const SET_OF_MIXED_KINDS: &str =
    "Not added to the set — a component set holds one kind of layer, and these mains mix kinds";

/// Which of `frames` owns a layer whose world bounds are `bounds`: the topmost
/// one covering more than half of it, or `None` for "no frame — the canvas".
///
/// Area, not the centre point: a centre test flips while a large layer is still
/// mostly outside. `frames` is in paint order, so the topmost qualifying one wins.
fn frame_covering(bounds: KRect, frames: &[(NodeId, KRect)]) -> Option<NodeId> {
    let area = bounds.width() * bounds.height();
    if area <= 0.0 {
        return None;
    }
    let mut chosen = None;
    for (id, frame) in frames {
        let overlap = frame.intersect(bounds);
        if overlap.width() > 0.0
            && overlap.height() > 0.0
            && overlap.width() * overlap.height() > area * 0.5
        {
            chosen = Some(*id);
        }
    }
    chosen
}

fn overlaps(a: KRect, b: KRect) -> bool {
    a.min_x() <= b.max_x()
        && a.max_x() >= b.min_x()
        && a.min_y() <= b.max_y()
        && a.max_y() >= b.min_y()
}

/// The character and paragraph span lists `tx` states for `id`, each `None` where
/// the transaction says nothing about that scope.
///
/// [`OndinApp::text_session_restyled_after`]'s decision. Per scope: a character
/// write must leave the editor's paragraph list alone.
fn stated_spans(tx: &Transaction, id: NodeId) -> (Option<CharSpans>, Option<ParaSpans>) {
    let mut spans = None;
    let mut para_spans = None;
    for op in &tx.0 {
        match op {
            Operation::SetTextSpans { id: on, spans: s } if *on == id => spans = Some(s.clone()),
            Operation::SetParagraphSpans { id: on, spans: s } if *on == id => {
                para_spans = Some(s.clone());
            }
            _ => {}
        }
    }
    (spans, para_spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A node of `kind` on the starter document's first artboard.
    fn add(s: &mut EditorSession, kind: NodeKind) -> NodeId {
        let id = s.ids.mint();
        let parent = s.doc.get(s.doc.root()).unwrap().children()[0];
        let index = s.doc.get(parent).unwrap().children().len();
        assert!(s.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: None,
        }])));
        id
    }

    /// Alt while drawing puts the press point at the centre, not at a corner.
    #[test]
    fn alt_grows_the_drawn_box_out_from_the_press_point() {
        let start = Point::new(100.0, 100.0);
        let (from, to) = centred_ends(start, Point::new(130.0, 120.0));
        assert_eq!(from, Point::new(70.0, 80.0));
        assert_eq!(to, Point::new(130.0, 120.0));
        assert_eq!(from.midpoint(to), start);
        assert_eq!((to.x - from.x, to.y - from.y), (60.0, 40.0));
    }

    /// `Alt`+`Shift` squares first and mirrors second, so the square is centred on
    /// the press.
    #[test]
    fn alt_and_shift_centre_the_square_on_the_press_rather_than_near_it() {
        let start = Point::new(100.0, 100.0);
        let pointer = Point::new(130.0, 120.0);
        let (from, to) = centred_ends(start, squared_corner(start, pointer));
        assert_eq!(to.x - from.x, to.y - from.y, "not square");
        assert_eq!(from.midpoint(to), start, "square is not on the press point");
        assert_eq!(
            (from, to),
            (Point::new(70.0, 70.0), Point::new(130.0, 130.0))
        );
    }

    /// A click with a shape tool makes a `tools::CLICK_CREATE_SIZE` square with the press at its top-left.
    #[test]
    fn a_click_makes_a_default_square_from_the_press_corner() {
        let at = Point::new(40.0, 60.0);
        let (from, to) = click_box(at, false);
        assert_eq!(from, at);
        assert_eq!(
            (to.x - from.x, to.y - from.y),
            (tools::CLICK_CREATE_SIZE, tools::CLICK_CREATE_SIZE)
        );
    }

    /// Alt centres the default box on the press and leaves its size alone (the centre and size assertions each catch a different wrong version).
    #[test]
    fn alt_centres_the_default_box_without_resizing_it() {
        let at = Point::new(40.0, 60.0);
        let (from, to) = click_box(at, true);
        assert_eq!(from.midpoint(to), at, "not centred on the press");
        assert_eq!(
            (to.x - from.x, to.y - from.y),
            (tools::CLICK_CREATE_SIZE, tools::CLICK_CREATE_SIZE),
            "Alt resized the default box, and it has no extent to resize"
        );
    }

    /// Only a lone line trades its box for endpoint handles; `lone_line` is the one answer all the suppression sites ask.
    #[test]
    fn only_a_line_on_its_own_trades_its_box_for_endpoint_handles() {
        let mut s = EditorSession::new();
        let end = Point::new(30.0, 10.0);
        let line = add(&mut s, NodeKind::Line { end });
        let rect = add(
            &mut s,
            NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
        );

        assert_eq!(lone_line(&s), None, "nothing is selected");

        s.selection.set_one(line);
        assert_eq!(
            lone_line(&s),
            Some((line, end)),
            "a lone line answers itself and the far end its handles go on"
        );

        s.selection.set_one(rect);
        assert_eq!(lone_line(&s), None, "a rect on its own keeps its box");

        s.selection.set(vec![line, rect]);
        assert_eq!(
            lone_line(&s),
            None,
            "a line picked with something else scales with the union box like \
             anything else, and taking the box away would leave the set with no \
             chrome at all"
        );
    }

    /// A character write states the character list and nothing else, for the edited node only.
    #[test]
    fn a_transaction_states_only_the_scopes_it_carries() {
        let mut ids = ondin_core::IdSource::new(1);
        let id = ids.mint();
        let other = ids.mint();
        let style = ondin_core::TextStyle::default();
        let paragraph = ondin_core::ParagraphStyle::default();

        let mut chars = CharSpans::default();
        chars.set(0..2, ondin_core::CharAttr::Size(24.0), &style);
        let mut paras = ParaSpans::default();
        paras.set(
            0..2,
            ondin_core::ParaAttr::Indent(ondin_core::Length::Px(8.0)),
            &paragraph,
        );

        // The character pair, as `char_attrs_tx` builds it.
        let (c, p) = stated_spans(
            &Transaction(vec![
                Operation::SetTextStyle {
                    id,
                    style: style.clone(),
                    spans: None,
                },
                Operation::SetTextSpans {
                    id,
                    spans: chars.clone(),
                },
            ]),
            id,
        );
        assert_eq!(c, Some(chars.clone()), "the character list is stated");
        assert_eq!(p, None, "and the paragraph list is left to the editor");

        // The paragraph pair says the mirror image.
        let (c, p) = stated_spans(
            &Transaction(vec![Operation::SetParagraphSpans {
                id,
                spans: paras.clone(),
            }]),
            id,
        );
        assert_eq!(c, None);
        assert_eq!(p, Some(paras.clone()));

        // A sizing change says nothing about either.
        let (c, p) = stated_spans(
            &Transaction(vec![Operation::SetGeometry {
                id,
                geometry: ondin_core::GeometryPatch::TextSizing(ondin_core::TextSizing::Auto),
            }]),
            id,
        );
        assert_eq!((c, p), (None, None));

        // Another node's spans are not this session's.
        let (c, p) = stated_spans(
            &Transaction(vec![
                Operation::SetTextSpans {
                    id: other,
                    spans: chars,
                },
                Operation::SetParagraphSpans {
                    id: other,
                    spans: paras,
                },
            ]),
            id,
        );
        assert_eq!((c, p), (None, None));
    }

    /// A rect's flattened path closes, and a closed `Polyline` strokes its closing edge.
    #[test]
    fn a_rect_outline_closes() {
        use ondin_core::kurbo::PathEl;

        let kind = NodeKind::Rect {
            size: ondin_core::kurbo::Size::new(80.0, 40.0),
            corner_radii: ondin_core::kurbo::RoundedRectRadii::from_single_radius(0.0),
        };
        let path = ondin_core::geometry::local_path(&kind).expect("a rect has an outline");
        let (mut subpaths, mut closes) = (0, 0);
        ondin_core::kurbo::flatten(&path, 0.1, |el| match el {
            PathEl::MoveTo(_) => subpaths += 1,
            PathEl::ClosePath => closes += 1,
            _ => {}
        });
        assert_eq!((subpaths, closes), (1, 1), "one subpath, and it closes");

        let pts = vec![
            egui::pos2(0.0, 0.0),
            egui::pos2(80.0, 0.0),
            egui::pos2(80.0, 40.0),
            egui::pos2(0.0, 40.0),
        ];
        let stroke = egui::Stroke::new(2.0, color::SELECT);
        let open = Polyline {
            pts: pts.clone(),
            closed: false,
        }
        .stroked(stroke);
        let shut = Polyline { pts, closed: true }.stroked(stroke);
        match (open, shut) {
            (egui::Shape::Path(o), egui::Shape::Path(c)) => {
                assert!(!o.closed, "an open subpath stays open");
                assert!(c.closed, "a closed subpath strokes its last edge");
            }
            other => panic!("expected two paths, got {other:?}"),
        }
    }

    /// Build the screen-space handle set for a box under a given rotation.
    fn handles_rotated(degrees: f64) -> Handles {
        handles_for(degrees, KRect::new(0.0, 0.0, 200.0, 40.0)) // deliberately wide
    }

    fn handles_for(degrees: f64, local: KRect) -> Handles {
        let world = Affine::rotate(degrees.to_radians());
        Handles {
            target: Target::Layer(NodeId::from_wire("1:1").unwrap()),
            pivot: world * local.center(),
            world,
            corners: Handle::CORNERS.map(|h| {
                let (ux, uy) = h.unit();
                let p = world * Point::new(local.width() * ux, local.height() * uy);
                (h, egui::pos2(p.x as f32, p.y as f32))
            }),
            locked: false,
        }
    }

    /// Only the Scale tool takes a cursor of its own at rest, since it shares every gesture with Select.
    #[test]
    fn only_the_scale_tool_takes_a_cursor_of_its_own_at_rest() {
        assert_eq!(rest_cursor(Tool::Scale).1, Some(Bitmap::Scale));
        for tool in [Tool::Select, Tool::Hand] {
            assert_eq!(rest_cursor(tool).1, None, "{tool:?}");
        }
        assert_ne!(rest_cursor(Tool::Scale).1, rest_cursor(Tool::Select).1);
    }

    /// Select and Scale differ only in what a handle drag means; no drawing tool answers `selects`.
    #[test]
    fn scale_shares_selects_gestures_and_only_its_verb_differs() {
        use crate::tools::Scaling;
        assert!(Tool::Select.selects() && Tool::Scale.selects());
        assert_eq!(Tool::Select.scaling(), Scaling::Geometry);
        assert_eq!(Tool::Scale.scaling(), Scaling::Photographic);
        assert!(!Tool::Scale.drags_a_box());
        for tool in [
            Tool::Hand,
            Tool::Frame,
            Tool::Rect,
            Tool::Ellipse,
            Tool::Polygon,
            Tool::Star,
            Tool::Line,
            Tool::Pen,
            Tool::Text,
        ] {
            assert!(!tool.selects(), "{tool:?}");
            assert_eq!(tool.scaling(), Scaling::Geometry, "{tool:?}");
        }
    }

    /// Unrotated, each corner asks for its diagonal (opposite corners share one) and each side for its axis.
    #[test]
    fn every_handle_gets_the_cursor_that_matches_its_direction() {
        use egui::CursorIcon::*;
        let h = handles_rotated(0.0);
        for (handle, want) in [
            (Handle::TopLeft, ResizeNwSe),
            (Handle::BottomRight, ResizeNwSe),
            (Handle::TopRight, ResizeNeSw),
            (Handle::BottomLeft, ResizeNeSw),
            (Handle::Top, ResizeVertical),
            (Handle::Bottom, ResizeVertical),
            (Handle::Left, ResizeHorizontal),
            (Handle::Right, ResizeHorizontal),
        ] {
            assert_eq!(resize_icon(h.screen_direction(handle)), want, "{handle:?}");
        }
    }

    /// The rotate band is outside the shape only; just inside a corner the gesture is a move.
    #[test]
    fn rotation_is_armed_outside_the_box_and_never_within_it() {
        let h = handles_for(0.0, KRect::new(0.0, 0.0, 200.0, 120.0));
        // The top-left corner sits at (0, 0); +x/+y heads into the shape.
        let inside = egui::pos2(14.0, 14.0);
        assert!(h.contains(inside), "the probe is not actually inside");
        assert_eq!(h.rotate_zone(inside), None, "rotate leaked inside the box");
        assert_eq!(h.armed(inside, false), None, "bare interior arms nothing");

        let outside = egui::pos2(-10.0, -10.0);
        assert!(!h.contains(outside));
        assert_eq!(
            h.armed(outside, false),
            Some(Armed::Rotate(Handle::TopLeft)),
            "just outside the corner must rotate"
        );
    }

    /// Ctrl turns a side into a skew; corners and the rotate band beyond them are unchanged.
    #[test]
    fn ctrl_turns_a_side_into_a_skew_and_leaves_corners_alone() {
        let h = handles_for(0.0, KRect::new(0.0, 0.0, 200.0, 120.0));
        for (probe, plain, held) in [
            // A side: resize without Ctrl, skew with it.
            (
                egui::pos2(100.0, -3.0),
                Armed::Resize(Handle::Top),
                Armed::Skew(Handle::Top),
            ),
            (
                egui::pos2(203.0, 60.0),
                Armed::Resize(Handle::Right),
                Armed::Skew(Handle::Right),
            ),
            // A corner, and the rotate band past it: the same either way.
            (
                egui::pos2(2.0, 2.0),
                Armed::Resize(Handle::TopLeft),
                Armed::Resize(Handle::TopLeft),
            ),
            (
                egui::pos2(-14.0, -3.0),
                Armed::Rotate(Handle::TopLeft),
                Armed::Rotate(Handle::TopLeft),
            ),
        ] {
            assert_eq!(h.armed(probe, false), Some(plain), "plain, at {probe:?}");
            assert_eq!(h.armed(probe, true), Some(held), "with Ctrl, at {probe:?}");
        }
    }

    /// The skew cursor runs along the side, the resize cursor's opposite on every side.
    #[test]
    fn the_skew_cursor_runs_along_the_side_a_resize_pushes() {
        use egui::CursorIcon::{ResizeHorizontal, ResizeVertical};
        let h = handles_for(0.0, KRect::new(0.0, 0.0, 200.0, 120.0));
        for (handle, across, along) in [
            (Handle::Top, ResizeVertical, ResizeHorizontal),
            (Handle::Bottom, ResizeVertical, ResizeHorizontal),
            (Handle::Left, ResizeHorizontal, ResizeVertical),
            (Handle::Right, ResizeHorizontal, ResizeVertical),
        ] {
            assert_eq!(
                resize_icon(h.screen_direction(handle)),
                across,
                "{handle:?} resize"
            );
            assert_eq!(
                resize_icon(h.screen_tangent(handle)),
                along,
                "{handle:?} skew"
            );
        }
    }

    /// Zones around a corner in priority order: the corner, the side, then the rotate band.
    #[test]
    fn corners_beat_sides_and_sides_beat_the_rotate_band() {
        let h = handles_for(0.0, KRect::new(0.0, 0.0, 200.0, 120.0));
        for (probe, want) in [
            (egui::pos2(2.0, 2.0), Armed::Resize(Handle::TopLeft)),
            // Just outside and just inside the side, so a thin shape stays resizable.
            (egui::pos2(100.0, -3.0), Armed::Resize(Handle::Top)),
            (egui::pos2(100.0, 3.0), Armed::Resize(Handle::Top)),
            (egui::pos2(203.0, 60.0), Armed::Resize(Handle::Right)),
            // Past the corner the side band has ended and the rotate band begun.
            (egui::pos2(-14.0, -3.0), Armed::Rotate(Handle::TopLeft)),
        ] {
            assert_eq!(h.armed(probe, false), Some(want), "at {probe:?}");
        }
    }

    /// The pill top sits at least `HANDLE_PICK_PX` below the box, through the real
    /// `paint_size_pill`.
    #[test]
    fn the_size_pill_hangs_clear_of_the_bottom_handles_grab_radius() {
        let box_ = egui::Rect::from_min_max(egui::pos2(100.0, 80.0), egui::pos2(300.0, 200.0));
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            paint_size_pill(
                ui.painter(),
                // The upright box's bottom edge, as `draw_create_badge` states it.
                BadgeEdge {
                    mid: egui::pos2(box_.center().x, box_.bottom()),
                    dir: egui::vec2(1.0, 0.0),
                    out: egui::vec2(0.0, 1.0),
                },
                size_label(KRect::new(0.0, 0.0, 200.0, 120.0)),
                false,
                color::BADGE,
            );
        });

        let ground = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Path(p) if p.fill == color::BADGE => Some(p.visual_bounding_rect()),
                _ => None,
            })
            .expect("the pill paints a BADGE-filled polygon");
        let text = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.job.text.clone()),
                _ => None,
            })
            .expect("the pill paints its label");

        assert_eq!(text, "200 × 120", "the pill quotes the box in whole units");
        assert!(
            ground.top() - box_.bottom() >= HANDLE_PICK_PX,
            "pill top is {} below the box, which does not clear the {HANDLE_PICK_PX}px handle grab",
            ground.top() - box_.bottom()
        );
        assert!(
            (ground.center().x - box_.center().x).abs() < 0.5,
            "pill centre {} against box centre {}",
            ground.center().x,
            box_.center().x
        );
    }

    /// The pill is centred on a real edge's midpoint, `GAP` clear along that edge's
    /// axes, at any turn and lean (sheared cases included), on the lowest edge.
    #[test]
    fn the_size_badge_hangs_off_a_real_edge_at_any_angle_or_lean() {
        const GAP: f32 = 11.0;
        let pill = egui::vec2(48.0, 16.0);
        // A 200×120 box centred on the screen origin, so the turn is pure.
        let local = [
            egui::pos2(-100.0, -60.0),
            egui::pos2(100.0, -60.0),
            egui::pos2(100.0, 60.0),
            egui::pos2(-100.0, 60.0),
        ];

        for (degrees, lean) in [
            (0.0_f32, 0.0_f32),
            (30.0, 0.0),
            (90.0, 0.0),
            (145.0, 0.0),
            (270.0, 0.0),
            // Sheared cases.
            (-48.56, 44.97),
            (-54.89, -67.96),
            (-57.80, -70.52),
        ] {
            let angle = degrees.to_radians();
            let (sin, cos) = angle.sin_cos();
            let k = lean.to_radians().tan();
            // Lean first, then turn, the order the gestures compose in.
            let place = |p: egui::Pos2| {
                let s = egui::pos2(p.x + k * p.y, p.y);
                egui::pos2(s.x * cos - s.y * sin, s.x * sin + s.y * cos)
            };
            let quad: [egui::Pos2; 4] = local.map(place);
            let edge = badge_edge(&quad).expect("a real quad has edges");
            let badge = badge_frame(edge, pill, GAP);
            let what = format!("{degrees}° turned, {lean}° leaning");

            // `edge.mid` must be the midpoint of one of the four edges.
            let mids: Vec<egui::Pos2> = (0..4)
                .map(|i| quad[i] + (quad[(i + 1) % 4] - quad[i]) * 0.5)
                .collect();
            assert!(
                mids.iter().any(|m| m.distance(edge.mid) < 1e-3),
                "{what}: the badge hangs off {:?}, which is no edge's midpoint",
                edge.mid
            );

            // `GAP` clear at the nearest corner and no further; on the corners, as
            // the centre distance would repeat `badge_frame`.
            let clearances: Vec<f32> = badge
                .rounded_rect(pill, 4.0)
                .iter()
                .map(|p| (*p - edge.mid).dot(edge.out))
                .collect();
            let nearest = clearances.iter().copied().fold(f32::MAX, f32::min);
            assert!(
                nearest >= GAP - 1e-3,
                "{what}: nearest pill corner {nearest} from the edge, inside GAP {GAP}"
            );
            assert!(
                nearest <= GAP + 6.0,
                "{what}: the pill floats {nearest} off an edge it should hug at {GAP}"
            );
            // Centred on it: no drift along the edge.
            let along = (badge.origin - edge.mid).dot(edge.dir);
            assert!(
                along.abs() < 1e-3,
                "{what}: badge slid {along} along the edge it is measuring"
            );
            // Outward means away from the shape, not into it.
            let centroid = quad.iter().fold(egui::Vec2::ZERO, |a, p| a + p.to_vec2()) / 4.0;
            assert!(
                (badge.origin.to_vec2() - centroid).length()
                    > (edge.mid.to_vec2() - centroid).length(),
                "{what}: the badge moved toward the middle of the shape"
            );
            // The lowest edge, not merely one of the four (flip: take the first edge → fails here).
            assert!(
                badge.origin.y > centroid.y,
                "{what}: the badge is above the middle of the shape at {:?}",
                badge.origin
            );
            let steepest = (0..4)
                .filter_map(|i| {
                    let along = quad[(i + 1) % 4] - quad[i];
                    (along.length() > 1e-6).then(|| {
                        let mid = quad[i] + along * 0.5;
                        let n = egui::vec2(-along.y, along.x).normalized();
                        let n = if n.dot(mid.to_vec2() - centroid) < 0.0 {
                            -n
                        } else {
                            n
                        };
                        n.y
                    })
                })
                .fold(f32::MIN, f32::max);
            assert!(
                edge.out.y >= steepest - 1e-4,
                "{what}: hung off an edge facing {} when one faces {steepest}",
                edge.out.y
            );
        }
    }

    /// The badge never tips past a readable slant and hangs under a real edge: swept
    /// a whole turn in 5° steps, since `badge_edge` hops edges at the boundaries.
    #[test]
    fn the_size_badge_stays_legible_and_hops_edges_as_the_shape_turns() {
        const GAP: f32 = 11.0;
        const QUARTER: f32 = std::f32::consts::FRAC_PI_2;
        let pill = egui::vec2(48.0, 16.0);
        // Not square, so a wrong edge shows up as a wrong width.
        let local = [
            egui::pos2(-100.0, -60.0),
            egui::pos2(100.0, -60.0),
            egui::pos2(100.0, 60.0),
            egui::pos2(-100.0, 60.0),
        ];

        let mut seen_widths = Vec::new();
        let mut edges = Vec::new();
        for step in 0..72 {
            let degrees = step as f32 * 5.0;
            let angle = degrees.to_radians();
            let (sin, cos) = angle.sin_cos();
            let turn = |p: egui::Pos2| egui::pos2(p.x * cos - p.y * sin, p.x * sin + p.y * cos);
            let quad: Vec<egui::Pos2> = local.iter().copied().map(turn).collect();

            let corners0: [egui::Pos2; 4] = [quad[0], quad[1], quad[2], quad[3]];
            let laid = badge_text_angle(badge_edge(&corners0).expect("edges").dir);
            // Within an eighth of a turn of upright, never upside down.
            assert!(
                laid.abs() <= QUARTER / 2.0 + 1e-4,
                "{degrees}°: badge laid out at {}°, past the readable range",
                laid.to_degrees()
            );
            // A whole number of quarter turns from the shape's angle: parallel to an edge.
            let turns = (angle - laid) / QUARTER;
            assert!(
                (turns - turns.round()).abs() < 1e-4,
                "{degrees}°: {turns} quarter turns off the shape"
            );

            let frame = Frame2::rotated(laid, egui::Pos2::ZERO);
            let oriented = frame.bounds_of(&quad);
            // Square-on in the reduced frame: 200×120 or 120×200.
            let (w, h) = (oriented.width(), oriented.height());
            assert!(
                ((w - 200.0).abs() < 1e-2 && (h - 120.0).abs() < 1e-2)
                    || ((w - 120.0).abs() < 1e-2 && (h - 200.0).abs() < 1e-2),
                "{degrees}°: the box is {w}×{h} in the badge's frame, not square-on"
            );
            seen_widths.push(w.round());

            // For a pure turn the edge is the bottom of `oriented` in this reduced frame.
            let corners: [egui::Pos2; 4] = [quad[0], quad[1], quad[2], quad[3]];
            let edge = badge_edge(&corners).expect("edges");
            let badge = badge_frame(edge, pill, GAP);
            // Clear of the edge it hangs from, not the box's bottom: at a diagonal
            // two edges tie.
            for p in badge.rounded_rect(pill, 4.0) {
                assert!(
                    (p - edge.mid).dot(edge.out) > 0.0,
                    "{degrees}°: the pill overlaps the shape at {p:?}"
                );
            }

            // Centred on a real edge, `GAP` + half a pill clear, in screen space.
            let midpoints: Vec<egui::Pos2> = (0..4)
                .map(|i| {
                    let (a, b) = (quad[i], quad[(i + 1) % 4]);
                    egui::pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
                })
                .collect();
            let (nearest, d) = (0..4)
                .map(|i| (i, badge.origin.distance(midpoints[i])))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert!(
                (d - (GAP + pill.y * 0.5)).abs() < 1e-2,
                "{degrees}°: badge {d} from edge {nearest}, want {}",
                GAP + pill.y * 0.5
            );

            // The outward normal points most steeply down, not the midpoint lowest;
            // `quad` runs clockwise on screen, so it is `(e.y, -e.x)`.
            let down = |i: usize| {
                let e = (quad[(i + 1) % 4] - quad[i]).normalized();
                egui::vec2(e.y, -e.x).y
            };
            let steepest = (0..4).map(down).fold(f32::MIN, f32::max);
            // By steepness, not index: on the diagonals two edges tie.
            assert!(
                (down(nearest) - steepest).abs() < 1e-3,
                "{degrees}°: badge on edge {nearest}, facing {} where {steepest} was available",
                down(nearest)
            );
            edges.push(nearest);
        }
        // Both extents show up over a turn: the pill moves between long and short edges.
        assert!(seen_widths.contains(&200.0) && seen_widths.contains(&120.0));
        // Four hops in a turn, one per quarter.
        assert_eq!(
            edges.windows(2).filter(|w| w[0] != w[1]).count(),
            4,
            "{edges:?}"
        );
    }

    /// The label lies along its edge, with no angle where it jumps across it; the
    /// 44/46 pair would have swung 88° under a quarter-turn reduction.
    #[test]
    fn the_badge_label_lies_along_its_edge_at_every_angle() {
        let at = |d: f32| badge_text_angle(egui::Vec2::angled(d.to_radians())).to_degrees();

        // Always parallel: the answer differs from the input by whole half turns.
        for step in 0..144 {
            let d = step as f32 * 2.5 - 180.0;
            let turns = (d - at(d)) / 180.0;
            assert!(
                (turns - turns.round()).abs() < 1e-3,
                "{d}° gave {}, which is {turns} half turns — not along the edge",
                at(d)
            );
            assert!(
                at(d).abs() <= 90.0 + 1e-3,
                "{d}° gave {}, past upside down",
                at(d)
            );
        }

        // Either side of 45° the label barely moves.
        assert!((at(44.0) - 44.0).abs() < 1e-3, "{}", at(44.0));
        assert!((at(46.0) - 46.0).abs() < 1e-3, "{}", at(46.0));
        assert!(
            (at(46.0) - at(44.0)).abs() < 3.0,
            "2° of shape turned the label {}°",
            (at(46.0) - at(44.0)).abs()
        );

        // Upright edges are left exactly alone.
        for d in [0.0, 180.0, -180.0, 360.0] {
            assert!(at(d).abs() < 1e-3, "{d}° gave {}", at(d));
        }
        // A vertical edge is the steep limit, one answer rather than a tremble.
        assert_eq!(at(90.0), at(90.0));
        assert!(at(90.0).abs() <= 90.0 + 1e-3);
    }

    /// A Shift-drawn line lands on one of eight directions exactly: `dx` and `dy`
    /// are whole numbers and equal (or one is zero), as `axial_corner` does.
    #[test]
    fn a_constrained_line_lands_on_an_exact_eighth_of_a_turn() {
        const OCTANT: f64 = std::f64::consts::TAU / 8.0;
        let start = Point::new(100.0, 100.0);
        // The step and projected length, with the grid on — `axial_corner`'s body.
        let drawn = |world: Point| {
            let d = world - start;
            let angle = (d.y.atan2(d.x) / OCTANT).round() * OCTANT;
            let step = Vec2::new(angle.cos().round(), angle.sin().round());
            let t = (d.dot(step) / step.dot(step)).round();
            start + step * t
        };

        // Near-horizontal snaps flat, and keeps the distance travelled along it.
        assert_eq!(drawn(Point::new(200.0, 105.0)), Point::new(200.0, 100.0));
        // Near-vertical, both ways.
        assert_eq!(drawn(Point::new(103.0, 200.0)), Point::new(100.0, 200.0));
        assert_eq!(drawn(Point::new(97.0, 20.0)), Point::new(100.0, 20.0));
        // Near-diagonal: (100, 90) projects to 95 along both axes.
        assert_eq!(drawn(Point::new(200.0, 190.0)), Point::new(195.0, 195.0));
        // All eight directions are reachable, and each is exact.
        for k in 0..8 {
            let a = k as f64 * OCTANT;
            let aim = start + Vec2::new(a.cos(), a.sin()) * 141.0;
            let d = drawn(aim) - start;
            assert_eq!(d.x, d.x.round(), "octant {k}: {d:?} is off the grid");
            assert_eq!(d.y, d.y.round(), "octant {k}: {d:?} is off the grid");
            // Axis-aligned or exactly diagonal, never in between.
            assert!(
                d.x == 0.0 || d.y == 0.0 || d.x.abs() == d.y.abs(),
                "octant {k}: {d:?} is not an eighth of a turn"
            );
            assert!(d.hypot() > 1.0, "octant {k}: collapsed to nothing");
        }
        // A drag that has not moved makes no line and no NaN out of `atan2`.
        assert_eq!(drawn(start), start);
    }

    /// Shift while drawing gives a square, in whichever direction the drag went,
    /// and the square encloses the pointer rather than falling short of it.
    #[test]
    fn a_constrained_draw_is_square_in_every_direction() {
        let start = Point::new(100.0, 100.0);
        // Down-right, wider than tall: the width wins and the height grows.
        assert_eq!(
            squared_corner(start, Point::new(180.0, 130.0)),
            Point::new(180.0, 180.0)
        );
        // Taller than wide: the height wins.
        assert_eq!(
            squared_corner(start, Point::new(130.0, 180.0)),
            Point::new(180.0, 180.0)
        );
        // Up-left, and the mixed diagonals: the signs are the drag's, the
        // magnitude is the larger one.
        assert_eq!(
            squared_corner(start, Point::new(20.0, 70.0)),
            Point::new(20.0, 20.0)
        );
        assert_eq!(
            squared_corner(start, Point::new(180.0, 70.0)),
            Point::new(180.0, 20.0)
        );
        assert_eq!(
            squared_corner(start, Point::new(70.0, 180.0)),
            Point::new(20.0, 180.0)
        );
        // A drag along one axis alone still yields a square, not a sliver.
        assert_eq!(
            squared_corner(start, Point::new(180.0, 100.0)),
            Point::new(180.0, 180.0)
        );
        // Degenerate: no drag, no shape, and no NaN out of `copysign`.
        assert_eq!(squared_corner(start, start), start);
    }

    /// Frame membership follows the artwork: more than half in means in, and
    /// the same rule run backwards is what lets a shape leave.
    #[test]
    fn a_layer_belongs_to_the_frame_that_covers_most_of_it() {
        let frame = NodeId::from_wire("1:1").unwrap();
        let frames = [(frame, KRect::new(0.0, 0.0, 100.0, 100.0))];
        // A 40-wide shape straddling the right edge. Its own area is 40×20.
        let straddling = |x0: f64| KRect::new(x0, 40.0, x0 + 40.0, 60.0);
        assert_eq!(
            frame_covering(straddling(70.0), &frames),
            Some(frame),
            "30 of 40 inside is in"
        );
        assert_eq!(
            frame_covering(straddling(85.0), &frames),
            None,
            "15 of 40 inside has left the frame"
        );
        // Exactly half is out: the rule is more than half, so a boundary shape does not flicker.
        assert_eq!(frame_covering(straddling(80.0), &frames), None);
        assert_eq!(frame_covering(straddling(79.0), &frames), Some(frame));
        // Clear of the frame entirely.
        assert_eq!(
            frame_covering(KRect::new(300.0, 300.0, 340.0, 320.0), &frames),
            None
        );
    }

    /// Overlapping frames: the topmost one that qualifies wins, matching what a
    /// click on the same spot would pick.
    #[test]
    fn the_topmost_qualifying_frame_wins() {
        let (under, over) = (
            NodeId::from_wire("1:1").unwrap(),
            NodeId::from_wire("1:2").unwrap(),
        );
        let frames = [
            (under, KRect::new(0.0, 0.0, 100.0, 100.0)),
            (over, KRect::new(0.0, 0.0, 100.0, 100.0)),
        ];
        let shape = KRect::new(10.0, 10.0, 30.0, 30.0);
        assert_eq!(frame_covering(shape, &frames), Some(over));
    }

    /// A nested frame is listed after its parent (`OndinApp::artboards` walks
    /// depth-first), so the topmost-qualifying rule picks the inner one.
    #[test]
    fn a_layer_inside_a_nested_frame_belongs_to_the_inner_one() {
        let (page, card) = (
            NodeId::from_wire("1:1").unwrap(),
            NodeId::from_wire("1:2").unwrap(),
        );
        // Paint order: the page, then the card sitting in a corner of it.
        let frames = [
            (page, KRect::new(0.0, 0.0, 400.0, 400.0)),
            (card, KRect::new(0.0, 0.0, 100.0, 100.0)),
        ];
        assert_eq!(
            frame_covering(KRect::new(10.0, 10.0, 30.0, 30.0), &frames),
            Some(card),
            "inside both, so it belongs to the smaller one it is really in"
        );
        // And the rest of the page still belongs to the page.
        assert_eq!(
            frame_covering(KRect::new(200.0, 200.0, 240.0, 240.0), &frames),
            Some(page)
        );
    }

    /// A zero-area layer has no half to be inside of, so no frame adopts it.
    #[test]
    fn a_degenerate_box_belongs_to_no_frame() {
        let frame = NodeId::from_wire("1:1").unwrap();
        let frames = [(frame, KRect::new(0.0, 0.0, 100.0, 100.0))];
        assert_eq!(
            frame_covering(KRect::new(50.0, 50.0, 50.0, 50.0), &frames),
            None
        );
    }

    /// The rotate ring is reachable out to the distance it claims.
    #[test]
    fn the_rotate_band_reaches_as_far_as_it_says() {
        let h = handles_for(0.0, KRect::new(0.0, 0.0, 200.0, 120.0));
        let inner = (HANDLE_PICK_PX + 1.0) / std::f32::consts::SQRT_2;
        let outer = (HANDLE_PICK_PX + ROTATE_RING_PX - 1.0) / std::f32::consts::SQRT_2;
        for d in [inner, outer] {
            assert_eq!(
                h.rotate_zone(egui::pos2(-d, -d)),
                Some(Handle::TopLeft),
                "the ring is not continuous at {d}"
            );
        }
        let past = (HANDLE_PICK_PX + ROTATE_RING_PX + 2.0) / std::f32::consts::SQRT_2;
        assert_eq!(h.rotate_zone(egui::pos2(-past, -past)), None);
    }

    /// A corner's cursor is not measured from the box centre: on a flat 200×40 box
    /// that would ask for a horizontal resize on a two-axis handle.
    #[test]
    fn a_corner_stays_diagonal_however_flat_the_box_is() {
        let h = handles_rotated(0.0);
        assert_eq!(
            resize_icon(h.screen_direction(Handle::BottomRight)),
            egui::CursorIcon::ResizeNwSe
        );
    }

    /// Cursors rotate with the node: at 90° the down-right corner takes the other diagonal.
    #[test]
    fn cursors_follow_the_nodes_rotation() {
        use egui::CursorIcon::*;
        assert_eq!(
            resize_icon(handles_rotated(90.0).screen_direction(Handle::BottomRight)),
            ResizeNeSw
        );
        // 45° puts that same corner straight down.
        assert_eq!(
            resize_icon(handles_rotated(45.0).screen_direction(Handle::BottomRight)),
            ResizeVertical
        );
    }
}

/// The layout grid overlay through `draw_layout_grids` on a headless app: the wiring
/// from `Node::grids` to the painter, the scrub buffer and the suppressing switches.
#[cfg(test)]
mod layout_grid_tests {
    use super::*;
    use ondin_core::{GridAxis, LayoutGrid, kurbo::Size};

    const FRAME: Size = Size::new(400.0, 200.0);

    /// A document that is one 400×200 frame at the origin, with `grids` on it.
    fn app_with_grids(ctx: &egui::Context, grids: Vec<LayoutGrid>) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard { size: FRAME },
                transform: None,
                name: None,
            },
            Operation::SetLayoutGrids {
                id: frame,
                grids: ondin_core::keyed_by_position(grids),
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        (app, frame)
    }

    /// Every band the overlay painted, as screen rects; filtered by shape, not
    /// colour, which a document may override.
    fn bands(app: &OndinApp, ctx: &egui::Context) -> Vec<egui::Rect> {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        ctx.run_ui(Default::default(), |ui| {
            app.draw_layout_grids(ui.painter(), area, 1.0);
        })
        .shapes
        .into_iter()
        .filter_map(|c| match c.shape {
            egui::Shape::Path(p) => Some(p.visual_bounding_rect()),
            _ => None,
        })
        .collect()
    }

    /// Four columns reach the painter and divide the frame's width.
    #[test]
    fn a_frames_columns_reach_the_painter_and_divide_its_box() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let grid = LayoutGrid {
            count: 4,
            gutter: 0.0,
            margin: 0.0,
            ..LayoutGrid::new(GridAxis::Columns)
        };
        let (app, frame) = app_with_grids(&ctx, vec![grid]);
        let out = bands(&app, &ctx);
        assert_eq!(out.len(), 4, "four columns, four bands");

        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let quad = app
            .selection_quad(frame, area, 1.0)
            .expect("the frame's box");
        let (left, right) = (quad[0].x, quad[1].x);
        assert!(
            (out[0].left() - left).abs() < 0.5 && (out[3].right() - right).abs() < 0.5,
            "the run spans the frame: {} to {} against {left} to {right}",
            out[0].left(),
            out[3].right()
        );
        let widths: Vec<f32> = out.iter().map(|r| r.width()).collect();
        assert!(
            widths.windows(2).all(|w| (w[0] - w[1]).abs() < 0.5),
            "four equal columns, got {widths:?}"
        );
    }

    /// The bands follow a gesture that has not committed, including a rotation.
    /// flip: read `resolved.world_transform` → fails at "the run spans the frame"
    #[test]
    fn the_bands_follow_a_frame_that_is_still_being_dragged() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let grid = LayoutGrid {
            count: 4,
            gutter: 0.0,
            margin: 0.0,
            ..LayoutGrid::new(GridAxis::Columns)
        };
        let (mut app, frame) = app_with_grids(&ctx, vec![grid]);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        // The invariant, asked of whatever state the app is in.
        let spans = |app: &OndinApp| {
            let out = bands(app, &ctx);
            assert_eq!(out.len(), 4, "four columns, four bands");
            let quad = app
                .selection_quad(frame, area, 1.0)
                .expect("the frame's box");
            let left = quad.iter().fold(f32::MAX, |a, p| a.min(p.x));
            let right = quad.iter().fold(f32::MIN, |a, p| a.max(p.x));
            let run_l = out.iter().fold(f32::MAX, |a, r| a.min(r.left()));
            let run_r = out.iter().fold(f32::MIN, |a, r| a.max(r.right()));
            (run_l, run_r, left, right)
        };

        let (l, r, ql, qr) = spans(&app);
        assert!(
            (l - ql).abs() < 0.5 && (r - qr).abs() < 0.5,
            "committed: the run spans the frame, {l}..{r} against {ql}..{qr}"
        );
        let resting = ql;

        // Mid-drag: moved and turned, and nothing committed.
        let moved = Affine::translate((137.0, 42.0)) * Affine::rotate(0.4);
        app.session
            .set_preview(&Transaction(vec![Operation::SetTransform {
                id: frame,
                transform: moved,
            }]));
        let before = app.session.revision();
        let (l, r, ql, qr) = spans(&app);
        assert!(
            (l - ql).abs() < 0.5 && (r - qr).abs() < 0.5,
            "mid-gesture: the run spans the frame, {l}..{r} against {ql}..{qr}"
        );
        // The frame really moved, or the assertion above is trivially true.
        assert!(
            (ql - resting).abs() > 5.0,
            "the preview left the frame at {resting}, so this test asserted nothing"
        );
        assert_eq!(
            app.session.revision(),
            before,
            "the fixture committed the gesture, so this is not a preview at all"
        );
    }

    /// A hidden grid and a hidden frame draw nothing; present mode is a chrome
    /// switch and the grid still draws.
    /// flip: reinstate `if self.present { return; }` → fails at "present mode is a chrome switch"
    #[test]
    fn a_hidden_grid_and_a_hidden_frame_draw_nothing_and_present_mode_draws_anyway() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let visible = LayoutGrid::new(GridAxis::Columns);
        let hidden = LayoutGrid {
            visible: false,
            ..visible
        };

        let (app, _) = app_with_grids(&ctx, vec![hidden]);
        assert!(bands(&app, &ctx).is_empty(), "the grid's own eye is off");

        let (mut app, frame) = app_with_grids(&ctx, vec![visible]);
        assert!(!bands(&app, &ctx).is_empty(), "the control case draws");
        app.present = true;
        assert!(
            !bands(&app, &ctx).is_empty(),
            "present mode is a chrome switch, not an overlay switch — the grid is \
             on the canvas and a designer editing with the panels out of the way \
             is still aligning to it (§15 D750)"
        );
        app.present = false;

        assert!(app.session.commit(Transaction(vec![Operation::SetVisible {
            id: frame,
            visible: false,
        }])));
        assert!(
            bands(&app, &ctx).is_empty(),
            "a switched-off frame's grid is chrome over nothing, like its name tag"
        );
    }

    /// *View ▸ Show layout grid* is an outer gate that hides the bands without
    /// writing `LayoutGrid::visible`, and back on restores them.
    /// flip: write over `LayoutGrid::visible` → fails at "the switch left the document alone"
    #[test]
    fn the_workspace_switch_hides_every_band_and_spends_no_per_grid_state() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let visible = LayoutGrid::new(GridAxis::Columns);
        let hidden = LayoutGrid {
            visible: false,
            ..visible
        };

        // One of each, so a switch that wrote the document would come back with both on.
        let (mut app, frame) = app_with_grids(
            &ctx,
            vec![visible, LayoutGrid { ..hidden }, {
                let mut rows = LayoutGrid::new(GridAxis::Rows);
                rows.visible = true;
                rows
            }],
        );
        let grids_unchanged = |app: &OndinApp| {
            app.session
                .doc
                .get(frame)
                .expect("the frame")
                .grids()
                .iter()
                .map(|g| g.visible)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            grids_unchanged(&app),
            vec![true, false, true],
            "the fixture is not in the state this test is about"
        );

        let lit = bands(&app, &ctx).len();
        assert!(
            lit > 0,
            "the control case draws — two of the three grids have their eye on"
        );

        app.set_view_switch(crate::input::ViewSwitch::LayoutGrid, false);
        assert!(
            bands(&app, &ctx).is_empty(),
            "the switch takes the bands away"
        );
        assert_eq!(
            grids_unchanged(&app),
            vec![true, false, true],
            "the switch left the document alone — it is an outer gate, not a write \
             over `LayoutGrid::visible` (§15 D387's eye is per grid and saved)"
        );

        app.set_view_switch(crate::input::ViewSwitch::LayoutGrid, true);
        assert_eq!(
            bands(&app, &ctx).len(),
            lit,
            "and back on restores exactly what the eyes said, because it never \
             touched them"
        );

        // The other two gates still hold with the switch on.
        app.set_view_switch(crate::input::ViewSwitch::LayoutGrid, true);
        assert!(app.session.commit(Transaction(vec![Operation::SetVisible {
            id: frame,
            visible: false,
        }])));
        assert!(
            bands(&app, &ctx).is_empty(),
            "the frame's own eye still wins with the workspace switch on"
        );
    }

    /// The overlay reads the scrub buffer, so the bands move with the number
    /// under the hand.
    /// flip: drop the buffer read → fails at "the in-flight count is what is drawn"
    #[test]
    fn the_bands_follow_a_scrub_that_has_not_been_committed() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let grid = LayoutGrid {
            count: 4,
            gutter: 0.0,
            margin: 0.0,
            ..LayoutGrid::new(GridAxis::Columns)
        };
        let (mut app, frame) = app_with_grids(&ctx, vec![grid]);
        assert_eq!(bands(&app, &ctx).len(), 4);

        app.in_flight.grid_scrub = Some((vec![frame], 0, LayoutGrid { count: 7, ..grid }));
        assert_eq!(
            bands(&app, &ctx).len(),
            7,
            "the in-flight count is what is drawn"
        );
        // The scrub has committed nothing yet.
        assert_eq!(app.session.doc.get(frame).unwrap().grids()[0].count, 4);

        // A buffer naming a different frame is not this frame's.
        app.in_flight.grid_scrub = Some((
            vec![app.session.doc.root()],
            0,
            LayoutGrid { count: 7, ..grid },
        ));
        assert_eq!(bands(&app, &ctx).len(), 4);
    }
}

/// A selected grid container's track lines: `track_lines`' arithmetic, and that
/// `draw_grid_tracks` puts them over the right box while selected.
#[cfg(test)]
mod grid_track_tests {
    use super::*;
    use ondin_core::container::{Display, Grid, LaidGrid, LaidTracks};
    use ondin_core::kurbo::Size;

    const AREA: egui::Rect =
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));

    /// A 400×200 frame at the origin laying out `100px 1fr` columns — gap 10,
    /// padding 20, so columns at 20–120 and 130–380 — with one 20×20 rect in it.
    fn app_with_grid(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let (frame, item) = (ids.mint(), ids.mint());
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 200.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: item,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetDisplay {
                id: frame,
                display: Some(Display::Grid(Grid {
                    columns: ondin_core::container::parse_tracks("100px 1fr").unwrap(),
                    column_gap: 10.0,
                    padding: [20.0; 4],
                    ..Default::default()
                })),
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        // The canvas the size of `AREA`: the lines are cut to the view.
        app.canvas_px = (800, 600);
        (app, frame)
    }

    /// The screen x of every vertical line the overlay drew, and how many gap
    /// bands — the only `convex_polygon`s it makes.
    fn drawn(app: &OndinApp, ctx: &egui::Context) -> (Vec<f32>, usize) {
        let shapes = ctx
            .run_ui(Default::default(), |ui| {
                app.draw_grid_tracks(ui.painter(), AREA, 1.0);
            })
            .shapes;
        let mut xs: Vec<f32> = Vec::new();
        let mut bands = 0;
        for c in shapes {
            match c.shape {
                egui::Shape::LineSegment { points: [a, b], .. } if (a.x - b.x).abs() < 0.01 => {
                    if !xs.iter().any(|x| (x - a.x).abs() < 0.01) {
                        xs.push(a.x);
                    }
                }
                egui::Shape::Path(_) => bands += 1,
                _ => {}
            }
        }
        xs.sort_by(f32::total_cmp);
        (xs, bands)
    }

    fn screen_x(app: &OndinApp, x: f64) -> f32 {
        app.to_screen(Point::new(x, 0.0), AREA, 1.0).x
    }

    /// A selected grid draws a line per column edge and a band in its gap; an
    /// unselected one nothing until an item moves; under an Alt-drag a copied
    /// container's lines ride the copy and a copied item's container stays.
    /// flip: apply the offset to every subject → fails at "an item's container stays"
    #[test]
    fn a_selected_grid_draws_its_column_edges_and_gap() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame) = app_with_grid(&ctx);
        assert_eq!(drawn(&app, &ctx), (Vec::new(), 0), "nothing selected");
        app.session.selection.set(vec![frame]);
        let (xs, bands) = drawn(&app, &ctx);
        let want: Vec<f32> = [20.0, 120.0, 130.0, 380.0]
            .map(|x| screen_x(&app, x))
            .to_vec();
        assert_eq!(xs.len(), 4, "four column edges: {xs:?}");
        for (x, w) in xs.iter().zip(&want) {
            assert!((x - w).abs() < 0.01, "{xs:?} against {want:?}");
        }
        assert_eq!(bands, 1, "one column gap, and no row gap");

        // An item draws nothing until moved, then its container's lines.
        let item = app.session.doc.get(frame).unwrap().children()[0];
        app.session.selection.set(vec![item]);
        assert_eq!(drawn(&app, &ctx), (Vec::new(), 0), "an item at rest");
        app.drag = Drag::Move {
            anchor: Point::ZERO,
        };
        let moving = drawn(&app, &ctx).0;
        assert_eq!(moving.len(), 4, "its container's, while it moves");

        // An Alt-drag carries a copy 50 right.
        app.alt_clone = Some(AltClone {
            copies: Vec::new(),
            delta: Vec2::new(50.0, 0.0),
        });
        assert_eq!(drawn(&app, &ctx).0, moving, "an item's container stays");
        app.session.selection.set(vec![frame]);
        let copied = drawn(&app, &ctx).0;
        let shift = screen_x(&app, 70.0) - screen_x(&app, 20.0);
        assert!(
            (copied[0] - (moving[0] + shift)).abs() < 0.01,
            "a copied container's ride the copy: {copied:?} from {moving:?}"
        );
    }

    /// The lines follow a resize's preview: 600 wide moves the `1fr` column's end to 580.
    /// flip: read `build::laid_grid` → fails at "the fr column's end follows the drag"
    #[test]
    fn the_lines_follow_a_resize_preview() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame) = app_with_grid(&ctx);
        app.session.selection.set(vec![frame]);
        app.session
            .set_preview(&Transaction(vec![Operation::SetGeometry {
                id: frame,
                geometry: ondin_core::GeometryPatch::Size(Size::new(600.0, 200.0)),
            }]));
        let (xs, _) = drawn(&app, &ctx);
        let last = *xs.last().expect("lines");
        assert!(
            (last - screen_x(&app, 580.0)).abs() < 0.01,
            "the fr column's end follows the drag: {xs:?}"
        );
    }

    /// `track_lines` draws each edge once, dashed wherever an explicit track has it,
    /// and a band in each gap (implicit rows before and after the template).
    /// flip: drop the `edge.1 |= …` merge → fails at "an edge an explicit track shares is dashed"
    #[test]
    fn track_lines_merge_edges_and_mark_the_explicit_ones() {
        let tracks = |spans: Vec<(f64, f64)>, before, explicit| LaidTracks {
            spans,
            before,
            explicit,
        };
        let grid = LaidGrid {
            columns: tracks(vec![(20.0, 120.0), (130.0, 210.0), (220.0, 380.0)], 0, 3),
            rows: tracks(
                vec![(0.0, 20.0), (20.0, 70.0), (80.0, 120.0), (120.0, 140.0)],
                1,
                2,
            ),
            areas: Vec::new(),
        };
        let out = OndinApp::track_lines(&grid, KRect::new(0.0, 0.0, 400.0, 200.0));
        let vertical: Vec<(f64, f64, f64, bool)> = out
            .lines
            .iter()
            .filter(|(a, b, _)| a.x == b.x)
            .map(|(a, b, e)| (a.x, a.y, b.y, *e))
            .collect();
        assert_eq!(
            vertical.iter().map(|v| v.0).collect::<Vec<_>>(),
            vec![20.0, 120.0, 130.0, 210.0, 220.0, 380.0]
        );
        assert!(
            vertical.iter().all(|v| v.1 == 0.0 && v.2 == 140.0 && v.3),
            "every column edge crosses the rows' run, dashed: {vertical:?}"
        );
        let horizontal: Vec<(f64, bool)> = out
            .lines
            .iter()
            .filter(|(a, b, _)| a.y == b.y)
            .map(|(a, b, e)| {
                assert_eq!((a.x, b.x), (20.0, 380.0), "across the columns' run");
                (a.y, *e)
            })
            .collect();
        assert_eq!(
            horizontal,
            vec![
                (0.0, false),
                (20.0, true),
                (70.0, true),
                (80.0, true),
                (120.0, true),
                (140.0, false),
            ],
            "an edge an explicit track shares is dashed, each edge once"
        );
        assert_eq!(
            out.gaps,
            vec![
                KRect::new(120.0, 0.0, 130.0, 140.0),
                KRect::new(210.0, 0.0, 220.0, 140.0),
                KRect::new(20.0, 70.0, 380.0, 80.0),
            ]
        );
    }

    /// Every segment the overlay drew, as its two screen ends.
    fn segments(app: &OndinApp, ctx: &egui::Context) -> Vec<[egui::Pos2; 2]> {
        ctx.run_ui(Default::default(), |ui| {
            app.draw_grid_tracks(ui.painter(), AREA, 1.0);
        })
        .shapes
        .into_iter()
        .filter_map(|c| match c.shape {
            egui::Shape::LineSegment { points, .. } => Some(points),
            _ => None,
        })
        .collect()
    }

    /// The lines follow their container moved 50 right and turned a quarter in the preview.
    /// flip: read `resolved.world_transform` → fails at "moved 50 right"
    #[test]
    fn the_lines_follow_their_container_moved_and_turned() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame) = app_with_grid(&ctx);
        app.session.selection.set(vec![frame]);
        let at_rest = drawn(&app, &ctx).0;
        app.session
            .set_preview(&Transaction(vec![Operation::SetTransform {
                id: frame,
                transform: Affine::translate((50.0, 0.0)),
            }]));
        let moved = drawn(&app, &ctx).0;
        let shift = screen_x(&app, 70.0) - screen_x(&app, 20.0);
        assert_eq!(moved.len(), 4);
        for (m, r) in moved.iter().zip(&at_rest) {
            assert!((m - (r + shift)).abs() < 0.01, "moved 50 right: {moved:?}");
        }

        app.session
            .set_preview(&Transaction(vec![Operation::SetTransform {
                id: frame,
                // 305 right to stay in view and put the turned row edges off every
                // column edge.
                transform: Affine::translate((305.0, 0.0))
                    * Affine::rotate(std::f64::consts::FRAC_PI_2),
            }]));
        // None sits where a column was.
        let upright = drawn(&app, &ctx).0;
        assert!(
            at_rest
                .iter()
                .all(|c| upright.iter().all(|u| (u - c).abs() > 0.01)),
            "no column edge left upright: {upright:?}"
        );
        let y = |v: f64| app.to_screen(Point::new(0.0, v), AREA, 1.0).y;
        let mut across: Vec<f32> = Vec::new();
        for [a, b] in segments(&app, &ctx) {
            if (a.y - b.y).abs() < 0.01 && !across.iter().any(|v| (v - a.y).abs() < 0.01) {
                across.push(a.y);
            }
        }
        for want in [20.0, 120.0, 130.0, 380.0] {
            assert!(
                across.iter().any(|v| (v - y(want)).abs() < 0.01),
                "a column edge laid across at {want}: {across:?}"
            );
        }
    }

    /// Explicit edges are dashed (4pt) and implicit ones dotted (1.5pt).
    /// flip: swap dash and dot → fails at "the columns dashed"
    #[test]
    fn explicit_edges_are_dashed_and_implicit_ones_dotted() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame) = app_with_grid(&ctx);
        app.session.selection.set(vec![frame]);
        let (mut upright, mut level) = (0.0_f32, 0.0_f32);
        for [a, b] in segments(&app, &ctx) {
            let length = (b - a).length();
            if (a.x - b.x).abs() < 0.01 {
                upright = upright.max(length);
            } else {
                level = level.max(length);
            }
        }
        assert!(
            (upright - 4.0).abs() < 0.01,
            "the columns dashed: {upright}"
        );
        assert!(
            (level - 1.5).abs() < 0.01,
            "the implicit row dotted: {level}"
        );
    }

    /// Under an inspector edit's hold, the lines stay for a scrub of the container's
    /// own layout and go for any other, and return on release.
    /// flip: the hold hides every subject → fails at "a gap scrub keeps them"; ignore the hold → "an opacity scrub hides them"
    #[test]
    fn a_layout_scrub_keeps_the_lines_and_any_other_hides_them() {
        use crate::app::ChromeHold;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame) = app_with_grid(&ctx);
        app.session.selection.set(vec![frame]);
        app.chrome_hold = ChromeHold::Scrubbing;
        let Some(Display::Grid(g)) = app.session.doc.get(frame).unwrap().display().cloned() else {
            panic!("the fixture's grid");
        };
        app.session
            .set_preview(&Transaction(vec![Operation::SetDisplay {
                id: frame,
                display: Some(Display::Grid(Grid {
                    column_gap: 40.0,
                    ..g
                })),
            }]));
        let xs = drawn(&app, &ctx).0;
        assert!(
            xs.iter().any(|x| (x - screen_x(&app, 160.0)).abs() < 0.01),
            "a gap scrub keeps them: {xs:?}"
        );
        app.session
            .set_preview(&Transaction(vec![Operation::SetOpacity {
                id: frame,
                opacity: 0.5,
            }]));
        assert_eq!(
            drawn(&app, &ctx),
            (Vec::new(), 0),
            "an opacity scrub hides them"
        );
        app.session.clear_gesture_preview();
        assert_eq!(drawn(&app, &ctx).0.len(), 4, "and they are back on release");
    }

    /// While an item is moved, its container's lines are the grid the drop reads:
    /// the drawn column under the moved centre is the one the drop writes.
    /// flip: read the container from `ov.laid_grid` → fails at "the drawn column under the centre is the one the drop writes"
    #[test]
    fn a_moved_items_lines_are_the_grid_its_drop_reads() {
        use ondin_core::container::parse_tracks;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let frame = ids.mint();
        let mut doc = Document::new(root);
        let mut ops = vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 100.0),
            },
            transform: None,
            name: None,
        }];
        let mut items = Vec::new();
        for (i, w) in [200.0, 20.0, 20.0].into_iter().enumerate() {
            let id = ids.mint();
            items.push(id);
            ops.push(Operation::CreateNode {
                id,
                parent: frame,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(w, 20.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
        }
        ops.push(Operation::SetDisplay {
            id: frame,
            display: Some(Display::Grid(Grid {
                columns: parse_tracks("auto auto auto").unwrap(),
                ..Default::default()
            })),
        });
        doc.apply(&Transaction(ops)).expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        let b = items[1];
        let centre = app.session.resolved.world_bounds(b).unwrap().center();
        assert!(
            (centre.x - 263.33).abs() < 0.01,
            "b in the second: {centre:?}"
        );

        let delta = Vec2::new(-110.0, 0.0);
        app.session.selection.set(vec![b]);
        let preview = app.move_preview_tx(delta).expect("a preview");
        app.session.set_preview(&preview);
        app.drag = Drag::Move {
            anchor: Point::ZERO,
        };
        let xs = drawn(&app, &ctx).0;
        assert_eq!(xs.len(), 4, "three columns' edges: {xs:?}");
        let cx = screen_x(&app, centre.x + delta.x);
        let under = xs.windows(2).position(|w| w[0] <= cx && cx < w[1]);
        assert_eq!(
            under,
            Some(0),
            "the drawn column under the centre is the one the drop writes: {xs:?} at {cx}"
        );

        app.session.clear_gesture_preview();
        app.drag = Drag::None;
        let tx = app.move_tx(delta);
        app.session.commit(tx);
        let laid = ondin_core::build::laid_grid(&app.session.doc, frame).unwrap();
        let area = laid.areas.iter().find(|(n, _)| *n == b).unwrap().1;
        assert_eq!(area[0], 1, "and the drop wrote the first column");
    }

    /// A grid's lines cost what is on screen: a few hundred shapes at 256×, dash
    /// phase fixed under a pan, and no layout pass (`GRIDS_LAID`) for an
    /// off-screen container.
    /// flip: `dashed_in_view` returns `[a, b]` → fails at "a few hundred at 256×"; cut starts at `t0` → "whole periods from the top"; container cull off → "and no layout pass for it either"
    #[test]
    fn the_lines_are_cut_to_the_view_and_hold_their_dashes_still() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame) = app_with_grid(&ctx);
        app.session.selection.set(vec![frame]);
        let count = |app: &OndinApp| {
            ctx.run_ui(Default::default(), |ui| {
                app.draw_grid_tracks(ui.painter(), AREA, 1.0);
            })
            .shapes
            .len()
        };
        app.session.camera.zoom = 256.0;
        app.session.camera.center = Point::new(20.0, 100.0);
        let zoomed = count(&app);
        assert!(
            zoomed > 0 && zoomed < 1500,
            "a few hundred at 256×: {zoomed}"
        );

        // The first column edge, x 20, runs down from the row's top.
        let grid = ondin_core::build::laid_grid(&app.session.doc, frame).unwrap();
        let top = grid.rows.spans[0].0;
        for pan in [0.0, 0.0013] {
            app.session.camera.center = Point::new(20.0, 100.0 + pan);
            let from = app.to_screen(Point::new(20.0, top), AREA, 1.0);
            let starts: Vec<f32> = segments(&app, &ctx)
                .into_iter()
                .filter(|[a, b]| (a.x - from.x).abs() < 0.01 && (b.x - from.x).abs() < 0.01)
                .map(|[a, b]| a.y.min(b.y))
                .collect();
            assert!(starts.len() > 10, "the edge is drawn: {starts:?}");
            for s in starts {
                let periods = (s - from.y) / 7.0;
                assert!(
                    (periods - periods.round()).abs() < 0.01,
                    "whole periods from the top at pan {pan}: {s} from {}",
                    from.y
                );
            }
        }

        app.session.camera.zoom = 1.0;
        let laid = || GRIDS_LAID.with(|n| n.get());
        let before = laid();
        count(&app);
        assert_eq!(laid(), before + 1, "the control: on screen, laid once");
        app.session.camera.center = Point::new(1.0e6, 1.0e6);
        let before = laid();
        assert_eq!(count(&app), 0, "nothing off screen");
        assert_eq!(laid(), before, "and no layout pass for it either");
    }
}

#[cfg(test)]
mod pivot_marker_tests {
    use super::*;

    /// The galley the marker paints, at the origin.
    fn marker(weight: Marker) -> std::sync::Arc<egui::Galley> {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx.run_ui(Default::default(), |ui| {
            paint_pivot(ui.painter(), egui::pos2(100.0, 100.0), weight);
        })
        .shapes
        .into_iter()
        .find_map(|c| match c.shape {
            egui::Shape::Text(t) => Some(t.galley),
            _ => None,
        })
        .expect("the marker painted no glyph")
    }

    /// The two weights are different glyphs (a tint would not survive small sizes)
    /// and both fit the grab radius.
    #[test]
    fn the_two_marker_weights_differ_and_both_fit_the_grab_radius() {
        let editable = marker(Marker::Editable);
        let read_only = marker(Marker::ReadOnly);
        assert_ne!(
            editable.text(),
            read_only.text(),
            "both weights drew the same glyph, so the state is invisible"
        );
        for (what, g) in [("editable", &editable), ("read-only", &read_only)] {
            let reach = g.size().max_elem() * 0.5;
            assert!(
                reach >= PIVOT_PICK_PX - 2.0,
                "{what}: the marker reaches {reach}, well inside its {PIVOT_PICK_PX}px \
                 grab radius — it will look like a dot in a hole"
            );
            assert!(
                reach <= PIVOT_PICK_PX + 3.0,
                "{what}: the marker reaches {reach} but only {PIVOT_PICK_PX}px of it \
                 can be grabbed"
            );
        }
    }

    /// Nothing is painted behind the glyph.
    #[test]
    fn the_marker_paints_no_ground_behind_itself() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        for weight in [Marker::Editable, Marker::ReadOnly] {
            let shapes = ctx
                .run_ui(Default::default(), |ui| {
                    paint_pivot(ui.painter(), egui::pos2(100.0, 100.0), weight);
                })
                .shapes;
            assert_eq!(
                shapes.len(),
                1,
                "{weight:?}: the marker is one glyph and nothing else, got {:?}",
                shapes.iter().map(|s| &s.shape).collect::<Vec<_>>()
            );
        }
    }
}

#[cfg(test)]
mod paste_text_tests {
    use super::*;
    use ondin_core::TextSizing;

    /// A paste that fits keeps auto width; one that does not wraps at the page, as
    /// `create_image_at` does.
    /// flip: `>=` for `>` in `wrapped_paste_sizing` → fails at "exactly fits" only
    #[test]
    fn a_pasted_line_wraps_only_once_it_outruns_the_page() {
        assert_eq!(wrapped_paste_sizing(10.0, 100.0), None, "well inside");
        assert_eq!(
            wrapped_paste_sizing(100.0, 100.0),
            None,
            "exactly fits, so auto width is still the answer"
        );
        assert_eq!(
            wrapped_paste_sizing(100.1, 100.0),
            Some(TextSizing::AutoHeight(100.0)),
            "a hair over wraps, at the page's width and not the line's"
        );
    }

    /// A root, a frame, a group in the frame, a rect in the group, and a rect
    /// straight on the canvas, returned in that order.
    fn nested() -> (Document, [NodeId; 5]) {
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, group, deep, loose) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let rect = || NodeKind::Rect {
            size: ondin_core::kurbo::Size::new(10.0, 10.0),
            corner_radii: Default::default(),
        };
        let create = |id, parent, index, kind| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            create(
                frame,
                root,
                0,
                NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(100.0, 100.0),
                },
            ),
            create(group, frame, 0, NodeKind::Group),
            create(deep, group, 0, rect()),
            create(loose, root, 1, rect()),
        ]))
        .expect("the fixture builds");
        (doc, [root, frame, group, deep, loose])
    }

    /// A selected frame answers with itself, and a layer inside one with its frame.
    /// flip: seed the walk from `parent()` → fails at the first assertion only
    #[test]
    fn the_frame_a_paste_is_aimed_at_is_the_one_the_selection_is_or_is_inside() {
        let (doc, [root, frame, group, deep, loose]) = nested();

        assert_eq!(
            enclosing_artboard(&doc, frame),
            Some(frame),
            "a selected frame is the frame — the case the old rule was the only one to get right"
        );
        assert_eq!(
            enclosing_artboard(&doc, group),
            Some(frame),
            "a group inside it hands over the frame"
        );
        assert_eq!(
            enclosing_artboard(&doc, deep),
            Some(frame),
            "and so does a shape two levels down"
        );

        // The bare canvas.
        assert_eq!(
            enclosing_artboard(&doc, loose),
            None,
            "a shape on the canvas is in no frame, and must not borrow one"
        );
        assert_eq!(
            enclosing_artboard(&doc, root),
            None,
            "the root is not a frame"
        );
    }

    /// A panel Paste lands above the clicked row, which is `+ 1`; the two `None`
    /// cases are what the fallback in `paste_clipboard_beside` rests on.
    /// flip: `position(id)` without `+ 1` → fails at all four slot assertions
    #[test]
    fn a_panel_paste_lands_in_the_slot_above_the_clicked_row() {
        let (doc, [root, frame, group, deep, loose]) = nested();

        assert_eq!(
            doc.get(root).map(|n| n.children().to_vec()),
            Some(vec![frame, loose])
        );
        assert_eq!(
            doc.get(frame).map(|n| n.children().to_vec()),
            Some(vec![group])
        );

        // `loose` sits at 1, so "above" is 2 and "at" is 1.
        assert_eq!(sibling_slot_above(&doc, loose), Some((root, 2)));
        assert_eq!(sibling_slot_above(&doc, frame), Some((root, 1)));
        // The parent is the clicked row's, so a row inside a group pastes into it.
        assert_eq!(sibling_slot_above(&doc, group), Some((frame, 1)));
        assert_eq!(sibling_slot_above(&doc, deep), Some((group, 1)));

        // The two with no answer.
        assert_eq!(
            sibling_slot_above(&doc, root),
            None,
            "the root has no siblings to sit beside"
        );
        assert_eq!(
            sibling_slot_above(&doc, NodeId { actor: 99, seq: 99 }),
            None,
            "and neither has a row that is not in the document"
        );
    }

    /// The box is centred, not the node's origin (default text's trimmed origin
    /// sits 4.54 units above its box).
    /// flip: drop the `box_offset` term → the first assertion passes, the second is 4.54 out
    #[test]
    fn a_pasted_layer_centres_its_box_and_not_its_origin() {
        let centre = Point::new(100.0, 100.0);
        let size = ondin_core::kurbo::Size::new(55.11, 17.46);

        // Untrimmed: the box *is* the origin, so this is plain centring.
        assert_eq!(
            centred_paste_origin(centre, size, Vec2::ZERO),
            Vec2::new(100.0 - 55.11 / 2.0, 100.0 - 17.46 / 2.0),
            "the fixture: with no trim the two points coincide"
        );

        // Trimmed, which is what `default_text_parts` actually produces.
        let origin = centred_paste_origin(centre, size, Vec2::new(0.0, 4.54));
        assert_eq!(
            origin.y + 4.54 + 17.46 / 2.0,
            100.0,
            "the box's own centre has to land on the point, not the node's origin"
        );
    }

    /// A wrapped paste is `AutoHeight`, never `Fixed`, which would clip at a height nobody chose.
    #[test]
    fn a_wrapped_paste_keeps_its_height_the_contents() {
        let sizing = wrapped_paste_sizing(500.0, 200.0).expect("this one wraps");
        assert!(
            !sizing.height_is_authored(),
            "{sizing:?} would fix a height the paste never asked for"
        );
        assert_eq!(sizing.wrap_width(), Some(200.0));
    }
}

#[cfg(test)]
mod image_name_tests {
    use super::*;

    /// A document whose root already holds children with these names.
    fn doc_with(names: &[&str]) -> (Document, NodeId) {
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        for (i, name) in names.iter().enumerate() {
            let id = ids.mint();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: Some((*name).to_string()),
            }]))
            .expect("a named sibling");
        }
        (doc, root)
    }

    /// A picture with no name of its own is numbered `Image N`, not `Rectangle N`
    /// (an image is a fill on a Rect), taking the lowest free number.
    #[test]
    fn a_picture_that_arrived_without_a_name_is_numbered_image() {
        let (doc, root) = doc_with(&[]);
        assert_eq!(image_layer_name(&doc, root, None), "Image 1");

        let (doc, root) = doc_with(&["Image 1", "Image 2"]);
        assert_eq!(image_layer_name(&doc, root, None), "Image 3");

        // The lowest free number fills the hole.
        let (doc, root) = doc_with(&["Image 1", "Image 3"]);
        assert_eq!(image_layer_name(&doc, root, None), "Image 2");
    }

    /// A filename is identity and survives, even when a sibling has the same name.
    #[test]
    fn a_picture_that_arrived_with_a_filename_keeps_it() {
        let (doc, root) = doc_with(&[]);
        assert_eq!(
            image_layer_name(&doc, root, Some("hero.png".into())),
            "hero.png"
        );
        let (doc, root) = doc_with(&["hero.png"]);
        assert_eq!(
            image_layer_name(&doc, root, Some("hero.png".into())),
            "hero.png"
        );
    }
}

#[cfg(test)]
mod drop_label_tests {
    use super::*;

    /// A hovering SVG is announced as an import and never as a picture replace; a mix
    /// is counted as files.
    #[test]
    fn a_hovering_svg_is_an_import() {
        assert_eq!(drop_label(1, 1, None), "Import SVG");
        assert_eq!(drop_label(2, 2, None), "Import 2 SVGs");
        assert_eq!(drop_label(3, 1, None), "Place 3 files");
        assert_eq!(drop_label(2, 0, None), "Place 2 images");
        let svg = egui::HoveredFile {
            path: Some("C:/art/Logo.SVG".into()),
            ..Default::default()
        };
        let png = egui::HoveredFile {
            path: Some("C:/art/logo.png".into()),
            ..Default::default()
        };
        assert!(hovers_svg(&svg) && !hovers_svg(&png));
    }

    /// The pill `paint_drop_label` paints, as a screen rect.
    fn pill(canvas: egui::Rect, at: egui::Pos2) -> egui::Rect {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx.run_ui(Default::default(), |ui| {
            paint_drop_label(ui.painter(), canvas, at, "Replace image in Photograph");
        })
        .shapes
        .into_iter()
        .find_map(|c| match c.shape {
            egui::Shape::Rect(r) => Some(r.rect),
            _ => None,
        })
        .expect("the label painted no pill")
    }

    /// The pill stays inside the canvas however far off its anchor is; the anchor is only a preference.
    #[test]
    fn the_label_stays_inside_the_canvas_however_far_off_its_anchor_is() {
        let canvas = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 600.0));
        for at in [
            egui::pos2(4000.0, -3000.0),
            egui::pos2(-4000.0, 3000.0),
            canvas.center(),
        ] {
            let pill = pill(canvas, at);
            assert!(
                canvas.contains_rect(pill),
                "anchored at {at:?} the pill landed at {pill:?}, outside the {canvas:?} canvas"
            );
        }
    }

    /// A canvas smaller than the pill centres it; `egui::Rect::clamp` would panic here.
    #[test]
    fn a_canvas_too_small_for_the_pill_centres_it_rather_than_panicking() {
        let canvas = egui::Rect::from_min_size(egui::pos2(20.0, 10.0), egui::vec2(60.0, 30.0));
        let pill = pill(canvas, egui::pos2(4000.0, -3000.0));
        let off = (pill.center() - canvas.center()).abs();
        assert!(
            off.max_elem() < 0.5,
            "the pill went to {:?} rather than the canvas centre {:?}",
            pill.center(),
            canvas.center()
        );
    }

    /// The desktop cursor maps into egui space at a scale factor of 1.5, where the
    /// wrong order of scaling and subtracting differs.
    #[test]
    fn the_desktop_cursor_lands_in_egui_space_at_a_scale_factor_that_is_not_one() {
        let inner = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0));
        let at = screen_pos_of_desktop_px((450.0, 375.0), inner, 1.5);
        assert!(
            (at - egui::pos2(200.0, 200.0)).abs().max_elem() < 0.01,
            "the cursor mapped to {at:?} rather than 200,200 inside the client area"
        );
    }

    /// The client area's top-left is egui's origin, whatever the desktop thinks.
    #[test]
    fn the_client_areas_top_left_is_the_origin_of_egui_space() {
        let inner = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0));
        let at = screen_pos_of_desktop_px((150.0, 75.0), inner, 1.5);
        assert!(
            at.to_vec2().abs().max_elem() < 0.01,
            "the window's own corner mapped to {at:?} rather than the origin"
        );
    }
}

#[cfg(test)]
mod autopan_tests {
    use super::*;
    use crate::preview::Drag;

    /// An 800x600 canvas at the window origin.
    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2 { x: 0.0, y: 0.0 },
        max: egui::Pos2 { x: 800.0, y: 600.0 },
    };

    /// The view travels toward the pointer: `pan_pixels` takes the delta the
    /// content moves by, so the sign is asserted through the camera.
    #[test]
    fn scrolling_into_an_edge_moves_the_view_that_way() {
        for (label, p, dx, dy) in [
            ("right", egui::pos2(799.0, 300.0), true, false),
            ("left", egui::pos2(1.0, 300.0), true, false),
            ("bottom", egui::pos2(400.0, 599.0), false, true),
            ("top", egui::pos2(400.0, 1.0), false, true),
        ] {
            let pan = autopan_delta(p, RECT, 1.0 / 60.0, 1.0).expect("inside the band");
            let mut cam = crate::view::Camera::default();
            let before = cam.center;
            cam.pan_pixels(pan);
            let moved = cam.center - before;
            if dx {
                let toward = (p.x - RECT.center().x).signum();
                assert!(
                    moved.x.signum() == f64::from(toward) && moved.y == 0.0,
                    "{label}: view moved {moved:?}, expected x toward {toward}"
                );
            }
            if dy {
                let toward = (p.y - RECT.center().y).signum();
                assert!(
                    moved.y.signum() == f64::from(toward) && moved.x == 0.0,
                    "{label}: view moved {moved:?}, expected y toward {toward}"
                );
            }
        }
    }

    /// Clear of both bands is no scroll, or every drag would drift.
    #[test]
    fn the_middle_of_the_canvas_does_not_scroll() {
        assert!(autopan_delta(egui::pos2(400.0, 300.0), RECT, 1.0 / 60.0, 1.0).is_none());
        // The band's inner lip, on both axes.
        let lip = egui::pos2(RECT.right() - AUTOPAN_BAND, RECT.bottom() - AUTOPAN_BAND);
        assert!(autopan_delta(lip, RECT, 1.0 / 60.0, 1.0).is_none());
    }

    /// The ramp is quadratic: half way into the band is a quarter speed.
    #[test]
    fn the_ramp_is_quadratic_and_clamps_past_the_edge() {
        let at = |x: f32| autopan_axis(x, 0.0, 800.0);
        let full = at(800.0);
        assert!((full - AUTOPAN_MAX).abs() < 1e-3, "at the edge: {full}");
        let half = at(800.0 - AUTOPAN_BAND / 2.0);
        assert!(
            (half - AUTOPAN_MAX / 4.0).abs() < 1e-3,
            "half into the band should be a quarter speed, got {half}"
        );
        assert!(
            (at(1200.0) - full).abs() < 1e-3,
            "400px past the window is not forty times faster"
        );
        assert_eq!(at(-400.0), -full, "and the same on the way out the left");
    }

    /// A canvas narrower than two bands scrolls in neither direction.
    #[test]
    fn a_canvas_too_narrow_for_two_bands_does_not_scroll() {
        let thin = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(40.0, 600.0));
        assert!(autopan_delta(egui::pos2(20.0, 300.0), thin, 1.0 / 60.0, 1.0).is_none());
    }

    /// A hitch must not throw the view across the document (the clamp is the caller's).
    #[test]
    fn a_long_frame_is_clamped_before_it_becomes_a_leap() {
        let p = egui::pos2(799.0, 300.0);
        let frame = |dt: f32| {
            autopan_delta(p, RECT, dt.min(AUTOPAN_MAX_DT), 1.0)
                .unwrap()
                .x
                .abs()
        };
        let normal = frame(1.0 / 60.0);
        assert!(frame(0.5) < normal * 5.0, "clamped to {AUTOPAN_MAX_DT}s");
        assert!(
            frame(0.5) > normal,
            "but a slow frame does still move further"
        );
    }

    /// Which gestures auto-pan and which are exempt, `Drag::Guide` and `PointRotate` included.
    /// flip: drop `PointRotate` or `Guide` from the `matches!` → fails naming that case
    #[test]
    fn panning_and_rotating_do_not_autopan() {
        assert!(!Drag::None.autopans());
        assert!(
            !Drag::Pan.autopans(),
            "the hand is already moving the view; the band's speed would add to it"
        );
        assert!(
            !Drag::Rotate {
                target: crate::preview::Target::Selection,
                pivot: ondin_core::kurbo::Point::ZERO,
                start: 0.0,
                turn: Affine::IDENTITY,
            }
            .autopans(),
            "a rotation turns around a pivot and never needs more room"
        );
        assert!(
            !Drag::PointRotate {
                id: ondin_core::NodeId::from_wire("1:1").unwrap(),
                pivot: ondin_core::kurbo::Point::ZERO,
                start: 0.0,
            }
            .autopans(),
            "a point rotation turns about a fixed pivot on exactly the argument \
             that exempts `Drag::Rotate`, and auto-panned 42.23 px for months \
             (§15 D760)"
        );
        assert!(
            !Drag::Guide(crate::preview::GuideDrag {
                id: None,
                axis: ondin_core::GuideAxis::Vertical,
                position: 0.0,
                owner: None,
                grab: ondin_core::kurbo::Vec2::ZERO,
                discarding: false,
                left_ruler: false,
                rulers_shown: true,
                copying: false,
            })
            .autopans(),
            "a guide's band and its home are the same place (§15 D303) — exempt \
             since then, and asserted by nothing until D760"
        );
        assert!(
            Drag::Move {
                anchor: ondin_core::kurbo::Point::ZERO
            }
            .autopans()
        );
        assert!(
            Drag::Marquee {
                origin: ondin_core::kurbo::Point::ZERO,
                additive: false
            }
            .autopans()
        );
        assert!(Drag::TextSelect.autopans());
    }

    /// A hovered marker is heavier, not bigger, measured through the tessellator
    /// because a stroked circle reaches `r + w`, not `r + w/2`.
    #[test]
    fn the_hover_weight_does_not_change_a_markers_silhouette() {
        fn bounds(shape: egui::Shape) -> egui::Rect {
            let mut mesh = egui::epaint::Mesh::default();
            egui::epaint::Tessellator::new(
                1.0,
                egui::epaint::TessellationOptions::default(),
                [1, 1],
                Vec::new(),
            )
            .tessellate_shape(shape, &mut mesh);
            let mut r = egui::Rect::NOTHING;
            for v in &mesh.vertices {
                r.extend_with(v.pos);
            }
            r
        }

        let at = egui::pos2(50.0, 50.0);
        let marker = |weight: f32| {
            let stroke = egui::Stroke::new(weight, egui::Color32::WHITE);
            (
                bounds(egui::Shape::rect_stroke(
                    egui::Rect::from_center_size(at, egui::Vec2::splat(POINT_PX)),
                    egui::CornerRadius::ZERO,
                    stroke,
                    egui::StrokeKind::Inside,
                )),
                bounds(egui::Shape::circle_stroke(
                    at,
                    POINT_PX * 0.5 - (weight - POINT_STROKE_PX),
                    stroke,
                )),
            )
        };

        let (square, circle) = marker(POINT_STROKE_PX);
        let (square_hovered, circle_hovered) = marker(POINT_STROKE_HOVER_PX);
        assert_eq!(square, square_hovered, "the corner marker changed size");
        assert_eq!(circle, circle_hovered, "the smooth marker changed size");

        // The resting circle is the plain radius.
        assert_eq!(
            circle,
            bounds(egui::Shape::circle_stroke(
                at,
                POINT_PX * 0.5,
                egui::Stroke::new(POINT_STROKE_PX, egui::Color32::WHITE),
            ))
        );
    }
}

#[cfg(test)]
mod text_gesture {
    //! The egui behaviour the Text tool's gestures rest on: `normal_mode_input`
    //! plants a node on `resp.clicked()` and begins a create on
    //! `resp.drag_started()`, exclusive only because egui reports a press as either.

    use super::MIN_CREATE_PX;
    use eframe::egui;

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// Press at `from`, move through `path`, release — reporting what the sensed
    /// region said over the whole gesture as `(clicked, drag_started, drag_stopped)`.
    fn gesture(from: egui::Pos2, path: &[egui::Pos2]) -> (bool, bool, bool) {
        let ctx = egui::Context::default();
        let id = egui::Id::new("canvas-under-test");
        let mut seen = (false, false, false);
        let mut t = 0.0_f64;
        let mut frame = |events: Vec<egui::Event>, seen: &mut (bool, bool, bool)| {
            t += 0.1;
            let _ = ctx.run_ui(
                egui::RawInput {
                    time: Some(t),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let resp = ui.interact(
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)),
                        id,
                        egui::Sense::click_and_drag(),
                    );
                    seen.0 |= resp.clicked();
                    seen.1 |= resp.drag_started();
                    seen.2 |= resp.drag_stopped();
                },
            );
        };
        frame(vec![egui::Event::PointerMoved(from)], &mut seen);
        frame(vec![button(from, true)], &mut seen);
        for p in path {
            frame(vec![egui::Event::PointerMoved(*p)], &mut seen);
        }
        frame(
            vec![button(*path.last().unwrap_or(&from), false)],
            &mut seen,
        );
        frame(Vec::new(), &mut seen);
        seen
    }

    /// egui's drag threshold (6px) sits at or above `MIN_CREATE_PX`, so no gesture
    /// falls between the two; raising `MIN_CREATE_PX` fails here.
    #[test]
    fn egui_starts_a_drag_only_past_the_distance_a_create_needs() {
        let at = egui::pos2(100.0, 100.0);
        // The largest move egui calls a click, and the smallest it calls a drag.
        let click_only = 6.0_f32;
        let drags = 6.5_f32;
        let (c, s, _) = gesture(at, &[egui::pos2(100.0 + click_only, 100.0)]);
        assert!(c && !s, "{click_only}px is still a click");
        let (c, s, _) = gesture(at, &[egui::pos2(100.0 + drags, 100.0)]);
        assert!(s && !c, "{drags}px is a drag");
        assert!(
            f64::from(click_only) >= MIN_CREATE_PX,
            "egui's threshold ({click_only}px) must stay at or above MIN_CREATE_PX \
             ({MIN_CREATE_PX}), or a drag could be too short to create and the Text \
             tool would have a gesture that does nothing"
        );
    }

    /// **A press is a click or a drag, never both.**
    #[test]
    fn a_press_is_reported_as_a_click_or_a_drag_and_never_both() {
        let at = egui::pos2(100.0, 100.0);

        let (clicked, started, _) = gesture(at, &[]);
        assert!(clicked, "a still press is a click");
        assert!(!started, "and starts no drag");

        let (clicked, started, stopped) =
            gesture(at, &[egui::pos2(160.0, 140.0), egui::pos2(220.0, 180.0)]);
        assert!(started && stopped, "a long press-move-release is a drag");
        assert!(
            !clicked,
            "and must not also be a click, or the Text tool would plant twice"
        );

        // Out and back is a drag too, which the text release arm acts on.
        let (clicked, started, stopped) =
            gesture(at, &[egui::pos2(160.0, 100.0), egui::pos2(100.5, 100.0)]);
        assert!(started && stopped, "out and back is still a drag");
        assert!(!clicked, "and still not a click, so nothing else plants");
    }
}

#[cfg(test)]
mod crop_chrome_tests {
    //! Image editing's chrome: these pin the shapes `crop_chrome` returns, not where
    //! it is called from.

    use super::*;

    /// The window, 100×60 at (20, 30), corners clockwise from the top-left.
    fn window() -> Vec<egui::Pos2> {
        vec![
            egui::pos2(20.0, 30.0),
            egui::pos2(120.0, 30.0),
            egui::pos2(120.0, 90.0),
            egui::pos2(20.0, 90.0),
        ]
    }

    /// Every dash segment the chrome emitted, as endpoint pairs.
    fn segments(shapes: &[egui::Shape]) -> Vec<[egui::Pos2; 2]> {
        shapes
            .iter()
            .filter_map(|s| match s {
                egui::Shape::LineSegment { points, .. } => Some(*points),
                _ => None,
            })
            .collect()
    }

    /// All four sides carry dashes, asserted per side, at a weight no lighter than
    /// the handles.
    #[test]
    fn the_crop_window_is_dashed_on_all_four_sides() {
        let w = window();
        let segs = segments(&crop_chrome(&w, None));
        // Top y = 30, right x = 120, bottom y = 90, left x = 20.
        let on = |pick: fn(egui::Pos2) -> f32, at: f32| {
            segs.iter()
                .filter(|[a, b]| (pick(*a) - at).abs() < 0.51 && (pick(*b) - at).abs() < 0.51)
                .count()
        };
        for (name, count) in [
            ("top", on(|p| p.y, 30.0)),
            ("right", on(|p| p.x, 120.0)),
            ("bottom", on(|p| p.y, 90.0)),
            ("left", on(|p| p.x, 20.0)),
        ] {
            assert!(
                count > 0,
                "the {name} side carries no dashes — the quad was not closed, so \
                 the box is drawn with an edge missing. All sides: {segs:?}"
            );
        }
        // Genuinely dashed, measured against the constants so retuning follows.
        let perimeter = 2.0 * (100.0 + 60.0);
        let expect = (perimeter / (CROP_DASH_PX + CROP_GAP_PX)) as usize;
        assert!(
            segs.len() + 4 >= expect && segs.len() > 8,
            "{} segments for a {perimeter}pt perimeter at a {CROP_DASH_PX}/\
             {CROP_GAP_PX} dash, where about {expect} are due — that is not the \
             dash the constants describe",
            segs.len()
        );
        // The weight keeps it distinct from the 1.2px selection hairline.
        for s in &crop_chrome(&w, None) {
            if let egui::Shape::LineSegment { stroke, .. } = s {
                assert!(
                    stroke.width >= HANDLE_STROKE_PX,
                    "the dash paints {} against a {HANDLE_STROKE_PX}pt handle — \
                     a hairline with gaps in it reads as the selection box",
                    stroke.width
                );
            }
        }
    }

    /// An unclosed quad (four points, no repeat) leaves the left side bare, which is why `crop_chrome` closes it.
    #[test]
    fn an_unclosed_quad_leaves_one_side_bare() {
        let w = window();
        let segs = segments(&egui::Shape::dashed_line(
            &w,
            egui::Stroke::new(CROP_STROKE_PX, color::SELECT),
            CROP_DASH_PX,
            CROP_GAP_PX,
        ));
        let left = segs
            .iter()
            .filter(|[a, b]| (a.x - 20.0).abs() < 0.51 && (b.x - 20.0).abs() < 0.51)
            .count();
        assert_eq!(
            left, 0,
            "the unclosed version drew the left side after all, so the closing \
             step this test justifies is not doing anything"
        );
    }

    /// Four hollow handles, and the picture's outline (one closed `Path`) only when there is a crop.
    #[test]
    fn the_handles_are_hollow_and_the_pictures_outline_is_conditional() {
        let w = window();
        let count = |shapes: &[egui::Shape]| {
            let rects = shapes
                .iter()
                .filter(|s| matches!(s, egui::Shape::Rect(_)))
                .count();
            let paths = shapes
                .iter()
                .filter(|s| matches!(s, egui::Shape::Path(_)))
                .count();
            (rects, paths)
        };

        let (rects, paths) = count(&crop_chrome(&w, None));
        assert_eq!(rects, 4, "four corners, four handles");
        assert_eq!(paths, 0, "no crop, so no outline round the whole picture");

        let picture = vec![
            egui::pos2(0.0, 10.0),
            egui::pos2(200.0, 10.0),
            egui::pos2(200.0, 120.0),
            egui::pos2(0.0, 120.0),
        ];
        let shapes = crop_chrome(&w, Some(&picture));
        let (rects, paths) = count(&shapes);
        assert_eq!(rects, 4, "the handles did not change");
        assert_eq!(paths, 1, "the picture's outline is one closed path");

        // Hollow: a filled handle covers the corner it places.
        for s in &shapes {
            if let egui::Shape::Rect(r) = s {
                assert_eq!(
                    r.fill,
                    egui::Color32::TRANSPARENT,
                    "a crop handle is filled, so it covers the corner it marks"
                );
                assert!(r.stroke.width > 0.0, "a hollow handle needs its ring");
            }
        }
    }
}

#[cfg(test)]
mod origin_field_tests {
    //! The canvas origin fields' readout (`origin_percent`) and write
    //! (`origin_pivot`), which must be inverses.
    use super::*;
    use ondin_core::kurbo::Size;

    fn rect() -> NodeKind {
        NodeKind::Rect {
            size: Size::new(80.0, 40.0),
            corner_radii: Default::default(),
        }
    }

    /// What a field shows is what typing it back asks for, so a scrub's identity
    /// edit is a no-op; the box is offset from the local origin.
    /// flip: measure from `0` rather than `box_.x0` → the top-left shows −25%/25%
    #[test]
    fn the_shown_percentage_and_the_typed_one_are_inverses() {
        let box_ = KRect::new(-20.0, 10.0, 60.0, 50.0);
        for (name, at) in [
            ("top left", Point::new(box_.x0, box_.y0)),
            ("bottom right", Point::new(box_.x1, box_.y1)),
            (
                "a third along",
                Point::new(box_.x0 + box_.width() / 3.0, box_.y1),
            ),
            // Outside the box: a field must not clamp what the marker can express.
            (
                "off to one side",
                Point::new(box_.x1 + 40.0, box_.y0 - 10.0),
            ),
        ] {
            let (px, py) = origin_percent(at, box_);
            let (px, py) = (px.expect(name), py.expect(name));
            let pivot = origin_pivot(&rect(), box_, px, py);
            let back = geometry::pivot_point(pivot, box_);
            assert!(
                (back.x - at.x).abs() < 1e-9 && (back.y - at.y).abs() < 1e-9,
                "{name}: {at:?} showed ({px}, {py}) and came back as {back:?}"
            );
        }
        // Corners read as round numbers: 0 left edge, 50 centre, 100 right.
        assert_eq!(
            origin_percent(Point::new(box_.x0, box_.y1), box_),
            (Some(0.0), Some(100.0))
        );
    }

    /// 50/50 is the absence of a pivot, not a stored half.
    /// flip: drop `pivot_placed_at`'s centre test → the centre comes back `Some(Normalized(0.5, 0.5))`
    #[test]
    fn both_axes_at_fifty_clear_the_origin() {
        let box_ = KRect::new(-20.0, 10.0, 60.0, 50.0);
        assert_eq!(origin_pivot(&rect(), box_, 50.0, 50.0), None);
        // One axis alone does not: the pivot is still somewhere off the middle.
        assert!(origin_pivot(&rect(), box_, 50.0, 30.0).is_some());
        assert!(origin_pivot(&rect(), box_, 30.0, 50.0).is_some());
        // An untouched layer reads back as 50/50.
        assert_eq!(
            origin_percent(geometry::pivot_point(None, box_), box_),
            (Some(50.0), Some(50.0))
        );
    }

    /// A dragged marker lands on a whole percent, the value the fields would
    /// produce; the nine snap positions survive the rounding.
    /// flip: round to one decimal → passes the nine, fails the shown string ("30")
    #[test]
    fn a_dragged_origin_lands_on_a_whole_percent() {
        let box_ = KRect::new(-20.0, 10.0, 60.0, 50.0);
        let round_trip = |at: Point| {
            let (px, py) = origin_percent(at, box_);
            let pivot = origin_pivot(
                &rect(),
                box_,
                px.unwrap_or(50.0).round(),
                py.unwrap_or(50.0).round(),
            );
            origin_percent(geometry::pivot_point(pivot, box_), box_)
        };

        // The nine are already whole percents.
        for (fx, fy) in [(0.0, 0.0), (0.5, 0.0), (1.0, 0.5), (0.5, 0.5), (1.0, 1.0)] {
            let at = Point::new(box_.x0 + box_.width() * fx, box_.y0 + box_.height() * fy);
            let (px, py) = round_trip(at);
            assert_eq!(
                (px, py),
                (Some(fx * 100.0), Some(fy * 100.0)),
                "the nine must be exact after rounding: {at:?}"
            );
        }

        // Asserted on the string the field paints: the stored fraction round-trips
        // only to within float error.
        let shown = crate::ui::number(2);
        let (px, py) = round_trip(Point::new(box_.x0 + 23.8, box_.y0 + 16.932));
        assert_eq!(shown(px.expect("x"), 0..=2), "30");
        assert_eq!(shown(py.expect("y"), 0..=2), "42");
    }

    /// A flat line (drawn with Shift) has one position on its short axis, so that
    /// field is dimmed and the other keeps working; the test is per axis.
    /// flip: test the box for degeneracy rather than each axis → the live axis is `None`, not 25%
    #[test]
    fn a_flat_line_dims_one_field_and_keeps_the_other() {
        let line = NodeKind::Line {
            end: Point::new(100.0, 0.0),
        };
        let flat = KRect::new(0.0, 0.0, 100.0, 0.0);
        let (px, py) = origin_percent(Point::new(25.0, 0.0), flat);
        assert_eq!(px, Some(25.0), "the live axis reads out");
        assert_eq!(py, None, "and the dead one has no percentage to show");

        // The dead axis contributes its single position; the live one decides.
        assert!(matches!(
            origin_pivot(&line, flat, 25.0, 50.0),
            Some(Pivot::Local(p)) if p == Point::new(25.0, 0.0)
        ));
        assert_eq!(origin_pivot(&line, flat, 50.0, 50.0), None);
        // Whatever the dimmed field showed.
        assert_eq!(origin_pivot(&line, flat, 50.0, 0.0), None);
    }
}

#[cfg(test)]
mod origin_pill_tests {
    //! The origin pill's width and its row height.
    use super::*;
    use eframe::egui;

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass so the fonts exist to lay text out with.
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    /// The pill holds the widest value the fields can show, `-31.25%` (the fields
    /// are unclamped).
    /// flip: `ORIGIN_FIELD_W` back to 62 → the two-decimal cases fail, `100%` passes
    #[test]
    fn the_number_has_room_for_the_widest_value_the_fields_can_show() {
        let ctx = ctx();
        let width = |s: &str, pt: f32| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    s.to_string(),
                    egui::FontId::proportional(pt),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            })
        };
        // What `badge_field` leaves for the digits.
        let room = ORIGIN_FIELD_W - crate::ui::badge_field_chrome_w(&ctx, "X");
        for s in ["0%", "100%", "-100%", "31.25%", "-31.25%"] {
            let w = width(s, crate::ui::BADGE_PT);
            assert!(
                w <= room,
                "`{s}` lays out at {w:.2}pt and the number has {room:.2}"
            );
        }
    }

    /// The pill row is exactly as tall as a pill, so the pills hang where the size
    /// badge does.
    /// flip: drop the `interact_size` line → the row measures 24
    #[test]
    fn the_pill_row_is_exactly_as_tall_as_a_pill() {
        let ctx = ctx();
        let at = egui::pos2(100.0, 100.0);
        let mut row = egui::Rect::NOTHING;
        // Three passes: an `Area` reports the origin until laid out once.
        for _ in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::Area::new(egui::Id::new("pill-row"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(at)
                        .show(ui.ctx(), |ui| {
                            ui.spacing_mut().interact_size.y = crate::ui::BADGE_FIELD_H;
                            row = ui
                                .horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = ORIGIN_FIELD_GAP;
                                    for (label, mut v) in [("X", 80.0_f64), ("Y", 31.25_f64)] {
                                        crate::ui::badge_field(
                                            ui,
                                            label,
                                            ORIGIN_FIELD_W,
                                            &mut v,
                                            crate::ui::Scrub::whole(0.5),
                                            |d| {
                                                d.suffix("%").custom_formatter(crate::ui::number(2))
                                            },
                                        );
                                    }
                                })
                                .response
                                .rect;
                        });
                },
            );
        }
        assert_eq!(row.top(), at.y, "the row must start where it was placed");
        assert_eq!(
            row.height(),
            crate::ui::BADGE_FIELD_H,
            "a taller row centres the pills in it and drops them below the badge's line"
        );
    }
}

#[cfg(test)]
mod overlay_claim {
    //! The egui behaviour the canvas's origin fields rest on: a press must resolve
    //! to the topmost layer by itself, pinned against egui directly.

    use eframe::egui;

    /// Press at `pos` with a `Foreground` area covering `over`, and report what the
    /// full-window canvas region beneath said: `(clicked, drag_started)`.
    fn canvas_saw(pos: egui::Pos2, over: egui::Rect) -> (bool, bool) {
        let ctx = egui::Context::default();
        let id = egui::Id::new("canvas-under-test");
        let mut seen = (false, false);
        let mut t = 0.0_f64;
        let mut frame = |events: Vec<egui::Event>, seen: &mut (bool, bool)| {
            t += 0.1;
            let _ = ctx.run_ui(
                egui::RawInput {
                    time: Some(t),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    // The overlay first, the hostile order, so this tests the area
                    // rather than registration order.
                    egui::Area::new(egui::Id::new("overlay"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(over.min)
                        .show(ui.ctx(), |ui| {
                            ui.allocate_exact_size(over.size(), egui::Sense::click_and_drag());
                        });
                    // The canvas: the full window, sensed as `canvas_ui` senses it.
                    let resp = ui.interact(
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)),
                        id,
                        egui::Sense::click_and_drag(),
                    );
                    seen.0 |= resp.clicked();
                    seen.1 |= resp.drag_started();
                },
            );
        };
        // Two settling frames: interaction resolves against the previous frame's rects.
        frame(vec![egui::Event::PointerMoved(pos)], &mut seen);
        frame(vec![egui::Event::PointerMoved(pos)], &mut seen);
        frame(
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            &mut seen,
        );
        frame(
            vec![egui::Event::PointerMoved(pos + egui::vec2(40.0, 30.0))],
            &mut seen,
        );
        frame(
            vec![egui::Event::PointerButton {
                pos: pos + egui::vec2(40.0, 30.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            &mut seen,
        );
        frame(Vec::new(), &mut seen);
        seen
    }

    /// `layer_id_at` names the area under the pointer, so `wheel_input` can take the
    /// wheel back from an area that swallows it.
    /// flip: ask for a layer id no area was created with → the exemption never matches
    #[test]
    fn the_layer_under_the_pointer_is_nameable() {
        let ctx = egui::Context::default();
        let over = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(69.0, 20.0));
        let ours = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("ours"));
        let mut at_pill = None;
        let mut at_canvas = None;
        for _ in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    events: vec![egui::Event::PointerMoved(over.center())],
                    ..Default::default()
                },
                |ui| {
                    ui.interact(
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)),
                        egui::Id::new("canvas"),
                        egui::Sense::click_and_drag(),
                    );
                    egui::Area::new(ours.id)
                        .order(ours.order)
                        .fixed_pos(over.min)
                        .show(ui.ctx(), |ui| {
                            ui.allocate_exact_size(over.size(), egui::Sense::click_and_drag());
                        });
                    at_pill = ui.ctx().layer_id_at(over.center());
                    at_canvas = ui.ctx().layer_id_at(egui::pos2(20.0, 20.0));
                },
            );
        }
        assert_eq!(
            at_pill,
            Some(ours),
            "the area under the pointer has to be nameable, or the wheel cannot be \
             handed back to the canvas"
        );
        assert_ne!(
            at_canvas, at_pill,
            "and a point off the pills must not answer with the pills' layer"
        );
    }

    /// A press on an `Area` does not reach the canvas beneath it, so pressing an
    /// origin field cannot start a marquee. A press just outside must reach the
    /// canvas, or the harness delivered nothing.
    #[test]
    fn an_overlay_takes_the_press_from_the_canvas_beneath() {
        let over = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(120.0, 20.0));

        let (clicked, started) = canvas_saw(over.center(), over);
        assert!(
            !clicked && !started,
            "the canvas must not see a press that landed on the overlay"
        );

        let outside = egui::pos2(over.left() - 10.0, over.center().y);
        let (clicked, started) = canvas_saw(outside, over);
        assert!(
            clicked || started,
            "the fixture is delivering presses — a press beside the overlay reaches \
             the canvas"
        );
    }
}

/// The one key in a live text session with two claimants (`docs/shortcuts.md` §10).
#[cfg(test)]
mod arrow_claim_tests {
    use super::arrow_moves_caret;
    use crate::input::{Action, Mode, TextChord, resolve};
    use eframe::egui;

    /// Drive `input::resolve` for one arrow key under one set of modifiers.
    fn keymap_claims(key: egui::Key, modifiers: egui::Modifiers) -> bool {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let mut got = Vec::new();
        let _ = ctx.run_ui(raw, |ui| {
            // Insert mode resolves no nudge, so the step is immaterial.
            got = resolve(
                ui.ctx(),
                Mode::TextInsert,
                crate::input::NudgeStep::default(),
            );
        });
        got.iter().any(|a| {
            matches!(
                a,
                Action::TextStyle(TextChord::Tracking(_) | TextChord::Leading(_))
            )
        })
    }

    /// Exactly one of the editor's loop and the keymap takes an arrow, for every
    /// modifier combination; `Ctrl+Alt` catches a keymap written `m.alt` instead of
    /// `!cmd && m.alt`.
    #[test]
    fn the_two_claimants_of_an_arrow_key_never_both_fire() {
        let arrows = [
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
        ];
        for bits in 0..8u8 {
            let modifiers = egui::Modifiers {
                alt: bits & 1 != 0,
                shift: bits & 2 != 0,
                ctrl: bits & 4 != 0,
                command: bits & 4 != 0,
                mac_cmd: false,
            };
            for key in arrows {
                let caret = arrow_moves_caret(modifiers);
                let chord = keymap_claims(key, modifiers);
                assert!(
                    caret != chord,
                    "{key:?} with {modifiers:?}: caret={caret}, chord={chord} — \
                     exactly one claimant must take it"
                );
            }
        }
    }
}

#[cfg(test)]
mod headless_app_tests {
    //! Assertions against a real headless `OndinApp`, which swaps a wgpu device,
    //! font threads and the preferences file for inert ones.
    use super::*;
    use ondin_core::kurbo::Size;

    fn rect(w: f64, h: f64) -> NodeKind {
        NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: Default::default(),
        }
    }

    /// Two 100×100 rects side by side, their union spanning (0,0)–(300,100);
    /// `pub(super)` so `autopan_ownership_tests` shares the fixture.
    pub(super) fn app_with_two_rects(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: a,
                parent: root,
                index: 0,
                kind: rect(100.0, 100.0),
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: b,
                parent: root,
                index: 1,
                kind: rect(100.0, 100.0),
                transform: Some(Affine::translate((200.0, 0.0))),
                name: None,
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        (app, a, b)
    }

    /// An `OndinApp` can be built without a GPU.
    #[test]
    fn the_app_builds_headless_and_holds_a_document() {
        let ctx = egui::Context::default();
        let (app, a, _) = app_with_two_rects(&ctx);
        assert_eq!(
            app.session.doc.get(a).map(|n| n.kind()),
            Some(&rect(100.0, 100.0))
        );
        // The decode cache is real even with no device.
        assert!(
            app.canvas.images().is_empty(),
            "nothing has been decoded yet"
        );
    }

    /// `Shift` inverts the proportion lock rather than setting it (an exclusive-or).
    #[test]
    fn shift_inverts_the_proportion_lock_rather_than_setting_it() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app_with_two_rects(&ctx);
        let target = crate::preview::Target::Layer(a);

        for (locked, shift, want) in [
            (false, false, false),
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            app.session
                .try_commit(Transaction(vec![Operation::SetProportionsLocked {
                    id: a,
                    locked,
                }]))
                .expect("set the lock");
            assert_eq!(
                app.resize_opts(target, shift, false).keep_ratio,
                want,
                "locked={locked}, shift={shift}"
            );
        }
    }

    /// A multi-selection resize scales every member about the shared box, so the
    /// far rect must move (200 to 400) as well as grow.
    #[test]
    fn dragging_a_corner_of_a_multi_selection_scales_the_whole_box() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_two_rects(&ctx);
        app.session.selection.set(vec![a, b]);
        let opts = app.resize_opts(crate::preview::Target::Selection, false, false);
        let tx = app.resize_tx(
            crate::preview::Target::Selection,
            crate::preview::Handle::BottomRight,
            Point::new(600.0, 100.0),
            opts,
        );
        assert!(!tx.0.is_empty(), "a selection resize has to produce edits");

        let mut doc = app.session.doc.clone();
        doc.apply(&tx).expect("apply the resize");
        let res = Resolved::rebuild(&doc);
        let far = res.world_bounds(b).expect("the far rect has bounds");
        assert!(
            (far.x0 - 400.0).abs() < 0.5,
            "the far rect has to travel with the box, not just grow: {far:?}"
        );
        assert!(
            (far.width() - 200.0).abs() < 0.5,
            "and double, since the union went 300 wide to 600: {far:?}"
        );
    }

    /// A skewing set's frame leans instead of inflating: the bottom pair (the held
    /// side) stays put and the top pair slides by the same amount.
    /// flip: drop the `Drag::Skew` arm from `selection_handles` → fails at the held side (`BottomLeft` moves)
    #[test]
    fn a_skewing_selections_frame_leans_rather_than_inflating() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        app.session.selection.set(vec![a, b]);

        let corners = |app: &OndinApp| {
            let h = app.selection_handles(area, 1.0).expect("a set has handles");
            let mut out: Vec<(Handle, egui::Pos2)> = h.corners.to_vec();
            out.sort_by_key(|(handle, _)| format!("{handle:?}"));
            out
        };

        let before = corners(&app);
        let union = app
            .session
            .resolved
            .world_bounds(a)
            .unwrap()
            .union(app.session.resolved.world_bounds(b).unwrap());
        assert!(
            (union.width() - 300.0).abs() < 1e-9,
            "the fixture's union should be 300 wide, not {union:?}"
        );

        // Arm a skew on the top side through `skew_tx`, as `update_drag` does.
        let press = Point::new(union.center().x, union.min_y());
        app.drag = Drag::Skew {
            target: Target::Selection,
            handle: Handle::Top,
            press,
            lean: Affine::IDENTITY,
        };
        let (tx, lean) = app.skew_tx(
            Target::Selection,
            Handle::Top,
            press,
            Point::new(press.x + 60.0, press.y),
            false,
        );
        app.session.set_preview(&tx);
        app.drag = Drag::Skew {
            target: Target::Selection,
            handle: Handle::Top,
            press,
            lean,
        };

        let after = corners(&app);

        // The held side is the bottom: those two corners must not have moved.
        for (h, p0) in &before {
            let (_, p1) = after.iter().find(|(k, _)| k == h).expect("same handles");
            if matches!(h, Handle::BottomLeft | Handle::BottomRight) {
                assert!(
                    (p1.x - p0.x).abs() < 0.01 && (p1.y - p0.y).abs() < 0.01,
                    "{h:?} is on the held side and moved: {p0:?} -> {p1:?}"
                );
            }
        }

        // The top pair slides by the same amount: a lean, not a widening.
        let slide = |h: Handle| {
            let p0 = before.iter().find(|(k, _)| *k == h).unwrap().1;
            let p1 = after.iter().find(|(k, _)| *k == h).unwrap().1;
            (p1.x - p0.x, (p1.y - p0.y).abs())
        };
        let (dl, dly) = slide(Handle::TopLeft);
        let (dr, dry) = slide(Handle::TopRight);
        assert!(
            dl > 1.0 && (dl - dr).abs() < 0.01,
            "both top corners should slide the same way — a lean, not a stretch: \
             left {dl}, right {dr}"
        );
        assert!(
            dly < 0.01 && dry < 0.01,
            "and the top edge stays at its own height: {dly}, {dry}"
        );
    }

    /// A turning set's frame turns with it, keeping its 300×100 size, through
    /// `Drag::selection_frame`.
    /// flip: `Drag::selection_frame` answers `None` for a rotation → the sides come back 236.6 and 309.8
    #[test]
    fn a_turning_selections_frame_turns_and_keeps_its_size() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        app.session.selection.set(vec![a, b]);

        let corner = |app: &OndinApp, want: Handle| {
            let h = app.selection_handles(area, 1.0).expect("a set has handles");
            h.corners
                .iter()
                .find(|(k, _)| *k == want)
                .expect("the handle")
                .1
        };
        let side = |app: &OndinApp, p: Handle, q: Handle| corner(app, p).distance(corner(app, q));

        // An upright 300×100 union, in screen units at zoom 1.
        assert!(
            (side(&app, Handle::TopLeft, Handle::TopRight) - 300.0).abs() < 0.01
                && (side(&app, Handle::TopLeft, Handle::BottomLeft) - 100.0).abs() < 0.01,
            "the fixture should start 300×100"
        );

        let union = app
            .session
            .resolved
            .world_bounds(a)
            .unwrap()
            .union(app.session.resolved.world_bounds(b).unwrap());
        let pivot = union.center();
        // 60° of turn.
        let at = pivot + Vec2::new(100.0 * 0.5, 100.0 * 3f64.sqrt() / 2.0);
        let (tx, turn) = app.rotate_tx(Target::Selection, pivot, 0.0, at, false);
        app.session.set_preview(&tx);
        app.drag = Drag::Rotate {
            target: Target::Selection,
            pivot,
            start: 0.0,
            turn,
        };

        let top = side(&app, Handle::TopLeft, Handle::TopRight);
        let left = side(&app, Handle::TopLeft, Handle::BottomLeft);
        assert!(
            (top - 300.0).abs() < 0.01 && (left - 100.0).abs() < 0.01,
            "a turned frame keeps its own 300×100; a re-unioned one measures about \
             236.6 × 309.8 — got {top} × {left}"
        );
        // Still a rectangle: equal diagonals, so it turned rather than deformed.
        let d1 = corner(&app, Handle::TopLeft).distance(corner(&app, Handle::BottomRight));
        let d2 = corner(&app, Handle::TopRight).distance(corner(&app, Handle::BottomLeft));
        assert!(
            (d1 - d2).abs() < 0.01,
            "the frame should still be a rectangle: diagonals {d1} and {d2}"
        );
        // It actually turned: the top edge is no longer level.
        let (tl, tr) = (
            corner(&app, Handle::TopLeft),
            corner(&app, Handle::TopRight),
        );
        assert!(
            (tr.y - tl.y).abs() > 10.0,
            "the frame did not turn at all: top edge from {tl:?} to {tr:?}"
        );
    }

    /// The frame keeps its orientation after release and gives it up when the
    /// selection changes, driven through the real `finish_drag`.
    /// flip: drop `set_frame` from the skew release arm → the frame squares up; `Selection::frame` ignores its stored ids → the last assertion; compose with `IDENTITY` → the first of two gestures is lost
    #[test]
    fn a_sets_frame_outlives_the_gesture_and_dies_with_the_selection() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        app.session.selection.set(vec![a, b]);

        // A lean holds the top edge level and tilts the sides.
        let side_slant = |app: &OndinApp| {
            let h = app.selection_handles(area, 1.0).expect("handles");
            let c = |want: Handle| {
                h.corners
                    .iter()
                    .find(|(k, _)| *k == want)
                    .expect("the handle")
                    .1
            };
            (c(Handle::TopLeft).x - c(Handle::BottomLeft).x).abs()
        };
        assert!(
            side_slant(&app) < 0.01,
            "the fixture starts upright, or nothing below means anything"
        );

        let union = app
            .session
            .resolved
            .world_bounds(a)
            .unwrap()
            .union(app.session.resolved.world_bounds(b).unwrap());
        let press = Point::new(union.center().x, union.min_y());
        app.drag = Drag::Skew {
            target: Target::Selection,
            handle: Handle::Top,
            press,
            lean: Affine::IDENTITY,
        };
        app.finish_drag(Some(Point::new(press.x + 60.0, press.y)), false, false);

        let leaning = side_slant(&app);
        assert!(
            leaning > 10.0,
            "the frame should still lean after the mouse came up, not square up: \
             the left side slants by {leaning}"
        );
        assert!(
            app.session.selection.frame().as_coeffs() != Affine::IDENTITY.as_coeffs(),
            "and the stored orientation is what keeps it there"
        );

        // Selecting again, even the same layers, is a new selection.
        app.session.selection.set(vec![a, b]);
        assert!(
            side_slant(&app) < 0.01,
            "reselecting squares the frame up: the orientation belongs to the \
             selection, not to the layers"
        );
    }

    /// Deselecting and reselecting the same pair must not resurrect the old
    /// orientation: `clear()` only makes the stored ids stop matching, so
    /// `crate::session::Selection::forget_frame` runs on every mutator.
    /// flip: drop `forget_frame()` from `set` → fails here and in the sibling test; from `clear` alone changes nothing (defence in depth)
    #[test]
    fn deselecting_does_not_leave_an_orientation_to_come_back_to() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        app.session.selection.set(vec![a, b]);

        let slant = |app: &OndinApp| {
            let h = app.selection_handles(area, 1.0).expect("handles");
            let c = |want: Handle| {
                h.corners
                    .iter()
                    .find(|(k, _)| *k == want)
                    .expect("the handle")
                    .1
            };
            (c(Handle::TopLeft).x - c(Handle::BottomLeft).x).abs()
        };

        // Leave a lean on the set, as a released gesture does.
        let union = app
            .session
            .resolved
            .world_bounds(a)
            .unwrap()
            .union(app.session.resolved.world_bounds(b).unwrap());
        let press = Point::new(union.center().x, union.min_y());
        app.drag = Drag::Skew {
            target: Target::Selection,
            handle: Handle::Top,
            press,
            lean: Affine::IDENTITY,
        };
        app.finish_drag(Some(Point::new(press.x + 60.0, press.y)), false, false);
        assert!(
            slant(&app) > 10.0,
            "the fixture needs a leaning frame first"
        );

        // Click away, then take the same two layers again.
        app.session.selection.clear();
        app.session.selection.set(vec![a, b]);
        assert!(
            slant(&app) < 0.01,
            "the old orientation came back after a deselect — the stored entry \
             outlived the selection it belonged to"
        );
    }

    /// The size pill sits on the same oriented box as the handles, not an upright
    /// re-derivation.
    #[test]
    fn the_size_pill_measures_the_frame_the_handles_are_on() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        app.session.selection.set(vec![a, b]);

        // Leave the set turned.
        let union = app
            .session
            .resolved
            .world_bounds(a)
            .unwrap()
            .union(app.session.resolved.world_bounds(b).unwrap());
        let pivot = union.center();
        let at = pivot + Vec2::new(100.0 * 0.5, 100.0 * 3f64.sqrt() / 2.0);
        let (tx, turn) = app.rotate_tx(Target::Selection, pivot, 0.0, at, false);
        app.session.try_commit(tx).expect("the turn commits");
        app.session.selection.set_frame(turn);

        let (_, want) = app.selection_frame_box().expect("an oriented box");
        let (_, got) = app
            .badge_placement(area, 1.0, egui::Vec2::ZERO)
            .expect("a pill for a set");
        assert!(
            (got.x0 - want.x0).abs() < 1e-9
                && (got.y0 - want.y0).abs() < 1e-9
                && (got.width() - want.width()).abs() < 1e-9
                && (got.height() - want.height()).abs() < 1e-9,
            "the pill measures {got:?} where the handles are on {want:?}"
        );
        // Its numbers are the set's own 300 × 100, not an upright box's.
        assert!(
            (got.width() - 300.0).abs() < 0.01 && (got.height() - 100.0).abs() < 0.01,
            "the pill should read the set's own extent: {got:?}"
        );
    }

    /// An Alt-drag copy lands above its own original whichever order the stack was
    /// picked in; the document-order case is the control.
    #[test]
    fn an_alt_drag_copy_lands_above_its_own_original_in_either_pick_order() {
        for (pick, label) in [(false, "document order"), (true, "reverse pick order")] {
            let ctx = egui::Context::default();
            let (mut app, a, b) = app_with_two_rects(&ctx);
            app.session.selection.set(match pick {
                false => vec![a, b],
                true => vec![b, a],
            });
            app.sync_alt_clone(true);
            let tx = app.clone_tx(Vec2::new(10.0, 0.0));
            app.session.try_commit(tx).expect("the clone commits");

            let kids = app
                .session
                .doc
                .get(app.session.doc.root())
                .unwrap()
                .children();
            assert_eq!(kids.len(), 4, "{label}: two originals and two copies");
            let at = |id: NodeId| kids.iter().position(|c| *c == id).expect("in the parent");
            assert_eq!(
                at(b) - at(a),
                2,
                "{label}: `a`'s copy sits between the two originals, so they are two \
                 apart — got {kids:?} with a at {} and b at {}",
                at(a),
                at(b)
            );
        }
    }

    /// One box-less member (an empty group) is skipped, so the rest of the
    /// selection keeps its transform box, handles and size badge.
    #[test]
    fn an_empty_group_in_the_selection_is_skipped_rather_than_fatal() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        app.session.selection.set(vec![a, b]);
        let (_, control) = app
            .selection_frame_box()
            .expect("control: two rects have a box");
        assert!(
            app.selection_handles(area, 1.0).is_some(),
            "control: and handles to grab it by"
        );

        let group = app.session.ids.mint();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: group,
                parent: app.session.doc.root(),
                index: 2,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            }]))
            .expect("an empty group is a legal document");
        assert!(
            app.session.preview_local_box(group).is_none(),
            "the fixture must actually reach the box-less state it names"
        );

        app.session.selection.set(vec![a, b, group]);
        let (_, with) = app
            .selection_frame_box()
            .expect("the empty group is skipped, not fatal");
        assert_eq!(
            with, control,
            "and the box is the one the members that have a box measure"
        );
        assert!(
            app.selection_handles(area, 1.0).is_some(),
            "so there are still handles on it"
        );
        assert!(
            app.badge_placement(area, 1.0, egui::Vec2::ZERO).is_some(),
            "and a size badge, which reads the same measurement"
        );

        // The door the user actually comes through.
        app.select_all();
        assert!(
            app.session.selection.ids().contains(&group),
            "fixture: Ctrl+A picks the empty group up too"
        );
        assert!(
            app.selection_handles(area, 1.0).is_some(),
            "Ctrl+A is the reported door and it must not disarm the chrome"
        );
    }

    /// A lone line wears no box handles, but a line in a set does (the rule is
    /// "lone", not "line"). The outline guard in `draw_canvas` is unobservable,
    /// because `line_ends` is matched first.
    #[test]
    fn a_lone_line_gets_no_selection_chrome_but_a_line_in_a_set_does() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let rect_area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        let line = ondin_core::IdSource::new(900).mint();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: line,
                parent: app.session.doc.root(),
                index: 2,
                kind: NodeKind::Line {
                    end: Point::new(120.0, 60.0),
                },
                transform: Some(Affine::translate((400.0, 0.0))),
                name: None,
            }]))
            .expect("add a line");

        app.session.selection.set_one(a);
        assert!(
            app.selection_handles(rect_area, 1.0).is_some(),
            "the control: an ordinary layer has handles, so `None` below means the \
             line rule and not a broken function"
        );

        app.session.selection.set_one(line);
        assert!(
            app.selection_handles(rect_area, 1.0).is_none(),
            "a lone line has no box, so it wears none of the box's chrome"
        );

        app.session.selection.set(vec![a, line]);
        assert!(
            app.selection_handles(rect_area, 1.0).is_some(),
            "a *set* containing a line has a union box and keeps its handles — the \
             rule is `lone_line`, not `is_line`"
        );
    }

    /// The size badge refuses a lone line on its own `lone_line` guard, separate
    /// from `selection_handles`.
    #[test]
    fn a_lone_line_gets_no_size_badge_and_a_rect_does() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, a, _) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let rect_area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        let line = ondin_core::IdSource::new(900).mint();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: line,
                parent: app.session.doc.root(),
                index: 2,
                kind: NodeKind::Line {
                    end: Point::new(120.0, 60.0),
                },
                transform: Some(Affine::translate((400.0, 0.0))),
                name: None,
            }]))
            .expect("add a line");

        // Did the badge paint? A real painter, because the pill lays out a galley.
        let badge = |app: &OndinApp| -> Option<()> {
            let mut out = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect_area),
                    ..Default::default()
                },
                |ui| out = app.draw_size_badge(ui.painter(), rect_area, 1.0, egui::Vec2::ZERO),
            );
            out
        };

        app.session.selection.set_one(a);
        assert!(
            badge(&app).is_some(),
            "the control: an ordinary layer is measured, so `None` below is the line \
             rule and not a badge that never draws"
        );

        app.session.selection.set_one(line);
        assert!(
            badge(&app).is_none(),
            "a lone line has one measurement worth quoting and it is not `W × H` — \
             the box round a diagonal is a rectangle nobody drew"
        );

        app.session.selection.set(vec![a, line]);
        assert!(
            badge(&app).is_some(),
            "and a *set* containing a line has a union box that does mean something"
        );
    }

    /// An open path — (0,0) → (100,0) → (100,100) — selected, with the Node tool
    /// armed and the pen bias on, which is what `pen_scope` asks for.
    fn app_editing_an_open_path(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let (mut app, a, _) = app_with_two_rects(ctx);
        // Clear of the rects, so "nothing under the pointer" is a reachable state.
        let mut p = ondin_core::kurbo::BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        p.line_to((100.0, 100.0));
        let id = ondin_core::IdSource::new(700).mint();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id,
                parent: app.session.doc.root(),
                index: 2,
                kind: NodeKind::Path {
                    path: p,
                    corner_radii: Vec::new(),
                },
                transform: Some(Affine::translate((1000.0, 1000.0))),
                name: None,
            }]))
            .expect("add the path");
        app.tool = Tool::Node;
        app.pen_bias = true;
        app.session.selection.set_one(id);
        // The rects stay selectable, which the fourth case below needs.
        let _ = a;
        (app, id)
    }

    /// `pen_verb`'s four answers are distinct; over another layer it is `None`
    /// because a press there still switches subject.
    #[test]
    fn the_pen_bias_routes_a_press_by_what_is_under_it() {
        let ctx = egui::Context::default();
        let (app, _) = app_editing_an_open_path(&ctx);
        let at = |x: f64, y: f64| Point::new(1000.0 + x, 1000.0 + y);

        assert!(
            matches!(app.pen_verb(at(100.0, 100.0)), Some(PenVerb::Continue(_))),
            "an open end of this path is the smallest, most deliberate target and \
             takes the press first"
        );
        assert!(
            matches!(app.pen_verb(at(50.0, 0.0)), Some(PenVerb::Insert(..))),
            "a point along one of its segments inserts an anchor there"
        );
        assert!(
            matches!(app.pen_verb(at(-400.0, 400.0)), Some(PenVerb::NewSubpath)),
            "empty canvas starts a new run — the case the whole verb exists for"
        );
        assert!(
            app.pen_verb(Point::new(50.0, 50.0)).is_none(),
            "another layer under the pointer still means *switch subject*; a pen \
             verb here would make that unreachable while the bias is on"
        );
    }

    /// Auto-scroll moves the view toward the pointer, and `autopan_total` records
    /// exactly what the camera did. A press comes first: `interact_pointer_pos` is
    /// `None` otherwise.
    #[test]
    fn reaching_the_canvas_edge_scrolls_the_view_toward_the_pointer() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        app.drag = Drag::TextSelect;
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        let press = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frame = |ctx: &egui::Context, events: Vec<egui::Event>, app: &mut OndinApp| {
            let input = egui::RawInput {
                screen_rect: Some(area),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let resp = ui.interact(
                    area,
                    egui::Id::new("autopan-probe"),
                    egui::Sense::click_and_drag(),
                );
                app.autopan(ui, &resp, area, 1.0);
            });
        };

        // A warm-up frame: egui resolves a press against last frame's widget rects.
        let middle = egui::pos2(400.0, 300.0);
        frame(&ctx, vec![egui::Event::PointerMoved(middle)], &mut app);
        // Middle of the canvas: in neither band.
        frame(
            &ctx,
            vec![egui::Event::PointerMoved(middle), press(middle, true)],
            &mut app,
        );
        let settled = app.session.camera.center;
        assert_eq!(
            app.autopan_total,
            Vec2::ZERO,
            "the middle of the canvas is clear of both bands — nothing to scroll"
        );

        // Now reach past the right edge with the button still down.
        for _ in 0..3 {
            frame(
                &ctx,
                vec![egui::Event::PointerMoved(egui::pos2(799.0, 300.0))],
                &mut app,
            );
        }
        let moved = app.session.camera.center - settled;
        assert!(
            moved.x > 0.0,
            "reaching right has to take the view right; a flipped sign makes the \
             canvas flee the thing being dragged toward it ({moved:?})"
        );
        assert!(
            moved.y.abs() < 1e-9,
            "the pointer was clear of the top and bottom bands ({moved:?})"
        );
        assert_eq!(
            app.autopan_total, moved,
            "the accumulator has to record exactly what the camera did, or a \
             cancel puts the view back somewhere else"
        );
    }

    /// The cursor ladder's ordering: panning over arming, arming under a live draw.
    /// Read from the `FullOutput` of real frames.
    #[test]
    fn the_cursor_ladder_puts_panning_over_arming_and_arming_under_a_live_draw() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let at = egui::pos2(400.0, 300.0);

        // Two frames per case (egui resolves hover against last frame's rects), read
        // from the `FullOutput`.
        let icon = |app: &mut OndinApp, keys: Vec<egui::Key>| {
            let mut last = egui::CursorIcon::Default;
            for _ in 0..2 {
                let input = egui::RawInput {
                    screen_rect: Some(area),
                    events: keys
                        .iter()
                        .map(|key| egui::Event::Key {
                            key: *key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: Default::default(),
                        })
                        .chain(std::iter::once(egui::Event::PointerMoved(at)))
                        .collect(),
                    ..Default::default()
                };
                let out = ctx.run_ui(input, |ui| {
                    let resp = ui.interact(
                        area,
                        egui::Id::new("cursor-probe"),
                        egui::Sense::click_and_drag(),
                    );
                    app.canvas_cursor(ui, &resp, area, 1.0);
                });
                last = out.platform_output.cursor_icon;
            }
            last
        };

        app.tool = Tool::Hand;
        assert_eq!(
            icon(&mut app, vec![]),
            egui::CursorIcon::Grab,
            "the hand tool arms a pan: an open hand"
        );

        app.tool = Tool::Rect;
        assert_eq!(
            icon(&mut app, vec![egui::Key::Space]),
            egui::CursorIcon::Grab,
            "and so does Space from any tool in normal mode"
        );

        app.drag = Drag::Pan;
        assert_eq!(
            icon(&mut app, vec![]),
            egui::CursorIcon::Grabbing,
            "a pan in hand outranks arming: a closed hand"
        );

        // `reshape_create` claims Space mid-draw, so nothing pans.
        app.drag = Drag::Create {
            id: a,
            parent: app.session.doc.root(),
            parent_world: Affine::IDENTITY,
            start: Point::ZERO,
            shape: Tool::Rect,
            sides: 4,
            last: Point::ZERO,
        };
        assert_eq!(
            icon(&mut app, vec![egui::Key::Space]),
            egui::CursorIcon::Crosshair,
            "Space during a create drag repositions the shape rather than panning, \
             so the cursor stays the tool's own"
        );
    }

    /// `cancel_gesture` is armed by each of our own witnesses without egui's
    /// `dragged_id` (which blinks on a short right-click) and is a no-op when idle.
    #[test]
    fn cancelling_needs_a_witness_and_our_two_do_not_blink() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app_with_two_rects(&ctx);
        assert!(
            !app.cancel_gesture(&ctx),
            "an idle app has nothing to cancel — a gate any wider than this takes \
             the right-click that removes a gradient stop"
        );

        app.drag = Drag::TextSelect;
        assert!(app.cancel_gesture(&ctx), "a canvas drag is a witness");
        assert!(matches!(app.drag, Drag::None), "and it is dropped");

        // The one that blinks is egui's; ours are tested with `dragged_id` empty.
        assert!(ctx.dragged_id().is_none(), "the fixture has no egui drag");
        app.in_flight.session_scrub = Some((a, Default::default(), Default::default()));
        assert!(
            app.cancel_gesture(&ctx),
            "a scrub previewing through the session is a witness of its own, and \
             it is the one that survives a click too short to span a frame"
        );
        assert!(app.in_flight.session_scrub.is_none(), "and it is taken");

        // Neither shows through `has_gesture_preview`: `SetExports` never reaches
        // `set_preview` and `SetGuidePosition` leaves `overrides` empty.
        app.in_flight.export_quality_scrub = Some((0, 0.8));
        assert!(
            app.cancel_gesture(&ctx),
            "the export quality scrub is a witness"
        );
        app.in_flight.export_quality_scrub = None;

        app.in_flight.guide_position_drag = Some((ondin_core::GuideAxis::Vertical, 100.0));
        assert!(
            app.cancel_gesture(&ctx),
            "and so is the inspector's guide-position field"
        );
        app.in_flight.guide_position_drag = None;

        app.layer_drag = None;
        app.session.selection.set_one(a);
        assert!(
            !app.cancel_gesture(&ctx),
            "and back to a strict no-op once every witness is gone"
        );
    }

    /// A valved field's teardown drops the gesture's preview and leaves a live
    /// text session's alone, on both the commit and the `Escape` disengage arms.
    #[test]
    fn a_valved_edit_does_not_take_a_live_text_sessions_preview_with_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, a, _) = app_with_two_rects(&ctx);

        let session_tx = Transaction(vec![Operation::SetOpacity {
            id: a,
            opacity: 0.5,
        }]);

        // Focused, then unfocused: `engaged` then `was_engaged`, the disengage arm.
        let drive = |app: &mut OndinApp, escape: bool, session: bool| {
            let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 60.0));
            let mut id = None;
            // Frame one: focused, so the valve arms.
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(area),
                    ..Default::default()
                },
                |ui| {
                    let r = ui.allocate_response(egui::vec2(80.0, 20.0), egui::Sense::click());
                    r.request_focus();
                    id = Some(r.id);
                    app.edit_valve(&r, Transaction(vec![]));
                },
            );
            // Re-seeded between the frames so the control case has no session preview.
            if session {
                app.session.set_session_preview(&session_tx, None);
            }
            // Frame two: focus surrendered, so `was_engaged && !engaged`.
            let events = if escape {
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }]
            } else {
                Vec::new()
            };
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(area),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let r = ui.allocate_response(egui::vec2(80.0, 20.0), egui::Sense::click());
                    r.surrender_focus();
                    app.edit_valve(&r, Transaction(vec![]));
                },
            );
        };

        // Asserted on the ink: `display_node` is what every panel and the scene read.
        let shown = |app: &OndinApp| app.session.display_node(a).map(|n| n.opacity());

        drive(&mut app, false, true);
        assert_eq!(
            shown(&app),
            Some(0.5),
            "the commit path must leave the session's uncommitted content on the canvas"
        );

        app.session.clear_preview();
        drive(&mut app, true, true);
        assert_eq!(
            shown(&app),
            Some(0.5),
            "and so must the Escape path, which is the one where nothing else clears"
        );

        // The control: with no session preview, the teardown really does clear.
        app.session.clear_preview();
        app.session.set_preview(&session_tx);
        drive(&mut app, true, false);
        assert_eq!(
            shown(&app),
            Some(1.0),
            "a plain gesture preview is still dropped"
        );
    }

    /// A valved field engaged over a live text session previews over the session's
    /// content, and its release winds back to the session alone (a `SetOpacity`
    /// stands in for `SetText`, which needs a font).
    #[test]
    fn a_valved_edit_engaged_over_a_live_text_session_previews_over_it_not_instead_of_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, a, _) = app_with_two_rects(&ctx);

        let session_tx = Transaction(vec![Operation::SetOpacity {
            id: a,
            opacity: 0.5,
        }]);
        let field_tx = Transaction(vec![Operation::SetTransform {
            id: a,
            transform: Affine::translate((120.0, 0.0)),
        }]);

        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 60.0));
        // One frame of a valved field: `focused` decides `engaged`, `escape` which
        // disengage arm is taken.
        let frame = |app: &mut OndinApp, focused: bool, escape: bool, tx: Transaction| {
            let events = match escape {
                true => vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                false => Vec::new(),
            };
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(area),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let r = ui.allocate_response(egui::vec2(80.0, 20.0), egui::Sense::click());
                    if focused {
                        r.request_focus();
                    } else {
                        r.surrender_focus();
                    }
                    app.edit_valve(&r, tx.clone());
                },
            );
        };

        let opacity = |app: &OndinApp| app.session.display_node(a).map(|n| n.opacity());
        let x = |app: &OndinApp| {
            app.session
                .preview_local_transform(a)
                .map(|t| t.translation().x)
        };

        // The session's preview is standing, the way `preview_session` leaves it.
        app.session.set_session_preview(&session_tx, None);
        frame(&mut app, true, false, field_tx.clone());

        assert_eq!(
            opacity(&app),
            Some(0.5),
            "the live session's uncommitted content must survive a field engaging over it"
        );
        assert_eq!(
            x(&app),
            Some(120.0),
            "and the field's own preview must still show, or the fix is 'stop previewing'"
        );

        // `cancel_gesture`'s witness: the composed preview is both a session and a gesture.
        assert!(
            app.session.has_gesture_preview(),
            "a scrub over a live session is still a gesture in flight"
        );

        // Released with `Escape`: the other arm commits, which would mask a leftover override.
        frame(&mut app, false, true, field_tx.clone());
        assert_eq!(
            x(&app),
            Some(0.0),
            "an abandoned field's preview must not stand for the rest of the session"
        );
        assert_eq!(
            opacity(&app),
            Some(0.5),
            "and the session's content is still on the canvas after it"
        );
        assert!(
            !app.session.has_gesture_preview(),
            "with the gesture gone, only the session's preview is left"
        );

        // The commit arm: the session's preview survives it.
        frame(&mut app, true, false, field_tx.clone());
        frame(&mut app, false, false, field_tx);
        assert_eq!(
            x(&app),
            Some(120.0),
            "the committed field value is in the document now"
        );
        assert_eq!(
            opacity(&app),
            Some(0.5),
            "and an unrelated commit still does not take the live text off the canvas"
        );
    }

    /// A pointer far from a path does not walk its segments (counted), while one
    /// within slop of the ink still finds its segment.
    /// flip: drop the hull reject in `node_grab` → fails at the far-pointer count (1 against 0).
    #[test]
    fn a_pointer_away_from_a_path_does_not_walk_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0xA11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();

        // Anchors 100 apart, so a pointer on the ink reaches the segment walk.
        let mut p = ondin_core::kurbo::BezPath::new();
        p.move_to(Point::new(0.0, 0.0));
        for i in 1..4096 {
            p.line_to(Point::new(i as f64 * 100.0, 0.0));
        }
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: p,
                corner_radii: Vec::new(),
            },
            transform: None,
            name: None,
        }]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.tool = Tool::Node;
        assert_eq!(
            app.edited_path(),
            Some(id),
            "the fixture arms the Node tool"
        );

        // On the ink, mid-path — the answer must be a segment and must not move.
        assert!(
            matches!(
                app.node_grab(Point::new(2050.0, 0.0)),
                Some(NodeGrab::Segment(..))
            ),
            "a pointer on the ink still finds its segment"
        );
        // And just off it, inside the grab slop — the case the inflation is for.
        assert!(
            matches!(
                app.node_grab(Point::new(2050.0, 1.0)),
                Some(NodeGrab::Segment(..))
            ),
            "and so does one within grabbing distance of it"
        );

        // Counted: the walk runs for a pointer near the path, not far from it.
        let walks = |at: Point| {
            let before = app.segment_walks.get();
            let _ = app.node_grab(at);
            app.segment_walks.get() - before
        };
        assert_eq!(walks(Point::new(2050.0, 0.0)), 1, "on the ink, it walks");
        assert_eq!(
            walks(Point::new(2050.0, 50_000.0)),
            0,
            "and 50,000 units away it must not"
        );
        // The far corner: outside the hull on both axes.
        assert_eq!(walks(Point::new(-50_000.0, -50_000.0)), 0);
    }

    /// A move drag walks the document once (counted by `frame_walks`), not once per
    /// selected layer, and a commit invalidates the artboard memo.
    /// flip: drop the `index.at != Some(revision)` guard → fails at the first count (20 against 0).
    #[test]
    fn a_move_drag_walks_the_document_once_however_many_layers_are_selected() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut ops = Vec::new();
        let mut roots = Vec::new();
        for i in 0..20 {
            let id = ids.mint();
            roots.push(id);
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: rect(10.0, 10.0),
                transform: Some(Affine::translate((i as f64 * 20.0, 0.0))),
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set(roots.clone());

        // One read to fill the memo.
        let _ = app.artboards();
        let before = app.frame_walks();
        let _ = app.leaving_their_frames(Vec2::new(5.0, 5.0));
        assert_eq!(
            app.frame_walks() - before,
            0,
            "twenty roots asked the same question twenty times and it was answered once"
        );

        // An edit invalidates the memo: a commit is when an artboard can appear, vanish or move.
        let moved = roots[0];
        app.session
            .commit(Transaction(vec![Operation::SetTransform {
                id: moved,
                transform: Affine::translate((3.0, 3.0)),
            }]));
        let _ = app.artboards();
        assert_eq!(
            app.frame_walks() - before,
            1,
            "a commit must make the next read re-walk"
        );
    }

    /// Cancelling a drag that auto-scrolled puts the view back; only the
    /// auto-scroll is undone (a zoom or wheel-pan stays).
    #[test]
    fn cancelling_a_drag_that_auto_scrolled_puts_the_view_back() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        app.drag = Drag::TextSelect;
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let home = app.session.camera.center;

        let frame = |events: Vec<egui::Event>, app: &mut OndinApp| {
            let input = egui::RawInput {
                screen_rect: Some(area),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let resp = ui.interact(
                    area,
                    egui::Id::new("cancel-autopan-probe"),
                    egui::Sense::click_and_drag(),
                );
                app.autopan(ui, &resp, area, 1.0);
            });
        };
        let middle = egui::pos2(400.0, 300.0);
        frame(vec![egui::Event::PointerMoved(middle)], &mut app);
        frame(
            vec![
                egui::Event::PointerMoved(middle),
                egui::Event::PointerButton {
                    pos: middle,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            &mut app,
        );
        for _ in 0..3 {
            frame(
                vec![egui::Event::PointerMoved(egui::pos2(799.0, 300.0))],
                &mut app,
            );
        }
        assert!(
            app.autopan_total.x > 0.0,
            "the fixture has to have actually scrolled: {:?}",
            app.autopan_total
        );

        assert!(app.cancel_gesture(&ctx), "the drag is a witness");
        assert_eq!(
            app.session.camera.center, home,
            "the view goes home with the layer"
        );
        assert_eq!(
            app.autopan_total,
            Vec2::ZERO,
            "and the accumulator is spent, or the next cancel moves the view again"
        );
    }

    /// An arrow key reaches the caret or the keymap and never both: `text_mode_input`
    /// moves the caret on a plain arrow and leaves an Alt-arrow for the nudge.
    #[test]
    fn an_arrow_key_reaches_the_caret_or_the_keymap_but_never_both() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        let text = ondin_core::IdSource::new(800).mint();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: text,
                parent: app.session.doc.root(),
                index: 2,
                kind: {
                    // The app's own authoring default.
                    let parts = crate::fonts::FontService::default_text_parts();
                    ondin_core::NodeKind::Text {
                        content: "hello world".into(),
                        style: Box::new(parts.style),
                        spans: parts.spans,
                        para_spans: parts.para_spans,
                        paragraph: parts.paragraph,
                        block: parts.block,
                        sizing: parts.sizing,
                        on_path: None,
                        on_path_flip: false,
                        on_path_offset: 0.0,
                    }
                },
                transform: Some(Affine::translate((400.0, 400.0))),
                name: None,
            }]))
            .expect("add the text node");
        // No world point: it selects all, so the caret has room to move.
        app.begin_edit_text(Some(text), None);

        let caret = |app: &OndinApp| {
            app.text
                .as_ref()
                .expect("the session is live")
                .editor
                .selected_range()
                .end
        };
        let arrow = |app: &mut OndinApp, alt: bool| {
            let modifiers = egui::Modifiers {
                alt,
                ..Default::default()
            };
            let input = egui::RawInput {
                screen_rect: Some(area),
                modifiers,
                events: vec![egui::Event::Key {
                    key: egui::Key::ArrowLeft,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let resp = ui.interact(
                    area,
                    egui::Id::new("arrow-probe"),
                    egui::Sense::click_and_drag(),
                );
                app.text_mode_input(ui, &resp, area, 1.0);
            });
        };

        let start = caret(&app);
        assert!(
            start > 0,
            "select-all leaves the caret with room to move left"
        );

        arrow(&mut app, true);
        assert_eq!(
            caret(&app),
            start,
            "Alt+← belongs to the keymap — it nudges the layer, and the caret must \
             not move with it"
        );

        arrow(&mut app, false);
        assert!(
            caret(&app) < start,
            "and a plain ← is the caret's, or the text field reads as dead"
        );
    }

    /// `Ctrl+Shift+C` centres the paragraph without writing the clipboard; the bare
    /// `Ctrl+C` control still copies.
    /// flip: drop `&& !shift` from the two arms → fails at the first assertion.
    #[test]
    fn ctrl_shift_c_centres_the_paragraph_without_copying() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_two_rects(&ctx);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        let text = ondin_core::IdSource::new(801).mint();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: text,
                parent: app.session.doc.root(),
                index: 2,
                kind: {
                    let parts = crate::fonts::FontService::default_text_parts();
                    ondin_core::NodeKind::Text {
                        content: "hello world".into(),
                        style: Box::new(parts.style),
                        spans: parts.spans,
                        para_spans: parts.para_spans,
                        paragraph: parts.paragraph,
                        block: parts.block,
                        sizing: parts.sizing,
                        on_path: None,
                        on_path_flip: false,
                        on_path_offset: 0.0,
                    }
                },
                transform: Some(Affine::translate((400.0, 400.0))),
                name: None,
            }]))
            .expect("add the text node");
        // Selects all, so there is something for a copy to take.
        app.begin_edit_text(Some(text), None);
        assert!(
            !app.text
                .as_ref()
                .expect("the session is live")
                .editor
                .selected_text()
                .is_empty(),
            "fixture: the selection must be non-empty, or a copy would write \
             nothing whatever the guard says"
        );

        // `egui_winit` turns the chord into `Event::Copy`, so only the frame's modifiers show the shift.
        let copy_under = |app: &mut OndinApp, shift: bool| {
            let modifiers = egui::Modifiers {
                command: true,
                shift,
                ..Default::default()
            };
            let input = egui::RawInput {
                screen_rect: Some(area),
                modifiers,
                events: vec![egui::Event::Copy],
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| {
                let resp = ui.interact(
                    area,
                    egui::Id::new("copy-probe"),
                    egui::Sense::click_and_drag(),
                );
                app.text_mode_input(ui, &resp, area, 1.0);
            });
            out.platform_output.commands.len()
        };

        assert_eq!(
            copy_under(&mut app, true),
            0,
            "Ctrl+Shift+C is centre-align — it must not also write the clipboard"
        );
        assert_eq!(
            copy_under(&mut app, false),
            1,
            "control: a bare Ctrl+C still copies out of a text session, which is \
             the thing D217 added and the plausible wrong fix would remove"
        );
    }

    /// Drawing the inspector commits and previews nothing, over a layer with paint.
    #[test]
    fn laying_out_the_inspector_commits_nothing_and_previews_nothing() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app_with_two_rects(&ctx);
        app.session
            .try_commit(Transaction(vec![
                Operation::SetFills {
                    id: a,
                    fills: ondin_core::keyed_by_position([ondin_core::Fill {
                        brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::WHITE).into(),
                        visible: true,
                    }]),
                },
                Operation::SetStrokes {
                    id: a,
                    strokes: ondin_core::keyed_by_position([
                        ondin_core::Stroke {
                            width: 2.0,
                            ..Default::default()
                        },
                        ondin_core::Stroke {
                            width: 4.0,
                            ..Default::default()
                        },
                    ]),
                },
            ]))
            .expect("give it paint to draw");
        app.session.selection.set_one(a);

        let before = app.session.revision();
        for _ in 0..2 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(280.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| app.inspector_ui(ui),
            );
        }
        assert_eq!(
            app.session.revision(),
            before,
            "no pointer touched anything, so nothing may have been committed"
        );
        assert!(
            !app.session.has_gesture_preview(),
            "and nothing may be left previewing — a preview outliving the frame that \
             made it is the snapshot every later write is then built from"
        );
    }

    /// A slot preview shows exactly the paint the later commit lands on.
    #[test]
    fn previewing_a_slot_shows_exactly_what_committing_it_will_do() {
        use crate::panels::paint::PaintSlot;
        let ctx = egui::Context::default();
        let (mut app, a, _) = app_with_two_rects(&ctx);
        app.session
            .try_commit(Transaction(vec![Operation::SetFills {
                id: a,
                fills: ondin_core::keyed_by_position([ondin_core::Fill {
                    brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::WHITE).into(),
                    visible: true,
                }]),
            }]))
            .expect("a fill to edit");
        let red = ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(255, 0, 0));
        let shown = |app: &OndinApp| {
            app.session
                .display_node(a)
                .map(|n| n.paint().fills[0].brush.clone())
        };

        let before = app.session.revision();
        assert!(
            app.preview_slot(a, PaintSlot::Fill(0), red.clone()),
            "a fill slot is previewable"
        );
        assert_eq!(
            app.session.revision(),
            before,
            "a preview is not a commit — this is the whole of the distinction"
        );
        let previewed = shown(&app);
        assert_eq!(
            previewed,
            Some(red.clone().into()),
            "and the panel reads it back through the override, or the drag shows \
             the old colour"
        );

        app.write_slot(a, PaintSlot::Fill(0), red.clone());
        assert_ne!(app.session.revision(), before, "a write *is* a commit");
        assert_eq!(
            shown(&app),
            previewed,
            "and it lands on the colour the preview promised"
        );
    }

    /// `write_slot`, `preview_slot` and `valve_slot` each divert a character-scoped slot
    /// onto the type's own `TextStyle::color`; the last needs a real drag.
    #[test]
    fn a_character_scoped_slot_is_diverted_onto_the_types_own_colour() {
        use crate::panels::paint::PaintSlot;
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_two_rects(&ctx);
        let parts = crate::fonts::FontService::default_text_parts();
        let text = ondin_core::IdSource::new(810).mint();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: text,
                parent: app.session.doc.root(),
                index: 2,
                kind: ondin_core::NodeKind::Text {
                    content: "hello".into(),
                    style: Box::new(parts.style),
                    spans: parts.spans,
                    para_spans: parts.para_spans,
                    paragraph: parts.paragraph,
                    block: parts.block,
                    sizing: parts.sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: None,
                name: None,
            }]))
            .expect("a text node");
        assert!(
            app.session
                .doc
                .get(text)
                .is_some_and(|n| n.paint().fills.is_empty()),
            "the fixture starts with no fills, or the assertions below prove nothing"
        );

        let blue = ondin_core::peniko::Color::from_rgb8(0, 0, 255);
        let brush = ondin_core::Brush::Solid(blue);
        let colour = |app: &OndinApp| match app.session.display_node(text).map(|n| n.kind()) {
            Some(ondin_core::NodeKind::Text { style, .. }) => style.color,
            _ => None,
        };
        let fills = |app: &OndinApp| app.session.doc.get(text).map(|n| n.paint().fills.len());

        app.write_slot(text, PaintSlot::TextColor, brush.clone());
        assert_eq!(
            colour(&app),
            Some(blue),
            "`write_slot` has to divert a text colour onto the type's own style"
        );
        assert_eq!(fills(&app), Some(0), "and never onto the fill list");

        // A fresh node, so the commit above cannot stand in for the preview.
        let (mut app, _, _) = app_with_two_rects(&ctx);
        let parts = crate::fonts::FontService::default_text_parts();
        let second = ondin_core::IdSource::new(811).mint();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: second,
                parent: app.session.doc.root(),
                index: 2,
                kind: ondin_core::NodeKind::Text {
                    content: "hello".into(),
                    style: Box::new(parts.style),
                    spans: parts.spans,
                    para_spans: parts.para_spans,
                    paragraph: parts.paragraph,
                    block: parts.block,
                    sizing: parts.sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: None,
                name: None,
            }]))
            .expect("a second text node");
        let before = app.session.revision();
        let _ = app.preview_slot(second, PaintSlot::TextColor, brush);
        let previewed = match app.session.display_node(second).map(|n| n.kind()) {
            Some(ondin_core::NodeKind::Text { style, .. }) => style.color,
            _ => None,
        };
        assert_eq!(
            previewed,
            Some(blue),
            "`preview_slot` diverts it the same way and shows the same colour"
        );
        assert_eq!(
            app.session.revision(),
            before,
            "and it is still a preview — the commit is owed at the end of the \
             session, not the end of the drag (§9.3)"
        );

        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 400.0));
        let third = ondin_core::IdSource::new(812).mint();
        let parts = crate::fonts::FontService::default_text_parts();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: third,
                parent: app.session.doc.root(),
                index: 3,
                kind: ondin_core::NodeKind::Text {
                    content: "hello".into(),
                    style: Box::new(parts.style),
                    spans: parts.spans,
                    para_spans: parts.para_spans,
                    paragraph: parts.paragraph,
                    block: parts.block,
                    sizing: parts.sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: None,
                name: None,
            }]))
            .expect("a third text node");
        let green = ondin_core::peniko::Color::from_rgb8(0, 255, 0);
        let mut dragged = false;
        for (i, pos) in [
            egui::pos2(100.0, 100.0),
            egui::pos2(100.0, 100.0),
            egui::pos2(180.0, 100.0),
        ]
        .into_iter()
        .enumerate()
        {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if i == 1 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(area),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let resp = ui.interact(
                        area,
                        egui::Id::new("valve-probe"),
                        egui::Sense::click_and_drag(),
                    );
                    dragged |= resp.dragged();
                    app.valve_slot(
                        &resp,
                        third,
                        PaintSlot::TextColor,
                        ondin_core::Brush::Solid(green),
                    );
                },
            );
        }
        assert!(
            dragged,
            "the fixture has to reach a real drag, or this proves nothing"
        );
        assert_eq!(
            match app.session.display_node(third).map(|n| n.kind()) {
                Some(ondin_core::NodeKind::Text { style, .. }) => style.color,
                _ => None,
            },
            Some(green),
            "`valve_slot` diverts a character-scoped slot too — all three do, which \
             is the whole of what `char_scoped` is for"
        );
    }

    /// Both rects selected and turned 60° about their centre, the orientation kept on the selection.
    fn app_with_a_turned_pair(ctx: &egui::Context) -> OndinApp {
        let (mut app, a, b) = app_with_two_rects(ctx);
        app.session.selection.set(vec![a, b]);
        let union = app
            .session
            .resolved
            .world_bounds(a)
            .unwrap()
            .union(app.session.resolved.world_bounds(b).unwrap());
        let pivot = union.center();
        let at = pivot + Vec2::new(100.0 * 0.5, 100.0 * 3f64.sqrt() / 2.0);
        let (tx, turn) = app.rotate_tx(Target::Selection, pivot, 0.0, at, false);
        app.session.try_commit(tx).expect("the turn commits");
        app.session.selection.set_frame(turn);
        app
    }

    /// The world position of a corner of the box the handles are drawn on
    /// (`frame * box corner`, as `selection_handles` places it).
    fn drawn_corner(app: &OndinApp, handle: Handle) -> Point {
        let (frame, b) = app.selection_frame_box().expect("an oriented box");
        let (ux, uy) = handle.unit();
        frame * Point::new(b.x0 + b.width() * ux, b.y0 + b.height() * uy)
    }

    /// Dragging a turned set's corner puts that corner on the pointer and holds the
    /// opposite corner still; the unturned pass is the control.
    /// flip: `resize_tx`'s Selection arm measures the upright world-bounds union → fails at the first assertion (turned pass).
    #[test]
    fn a_turned_set_resizes_about_the_box_its_handles_are_drawn_on() {
        let ctx = egui::Context::default();
        for turned in [false, true] {
            let mut app = match turned {
                true => app_with_a_turned_pair(&ctx),
                false => {
                    let (mut app, a, b) = app_with_two_rects(&ctx);
                    app.session.selection.set(vec![a, b]);
                    app
                }
            };
            let (frame, _) = app.selection_frame_box().expect("an oriented box");
            let grabbed = drawn_corner(&app, Handle::TopLeft);
            let held = drawn_corner(&app, Handle::BottomRight);
            let pointer = grabbed + (frame * Point::new(-40.0, 0.0) - frame * Point::ZERO);

            let tx = app.resize_tx(
                Target::Selection,
                Handle::TopLeft,
                pointer,
                tools::Resize::geometry(false, false),
            );
            app.session.try_commit(tx).expect("the resize commits");

            let now = drawn_corner(&app, Handle::TopLeft);
            let still = drawn_corner(&app, Handle::BottomRight);
            let which = if turned { "turned" } else { "upright" };
            assert!(
                (now - pointer).hypot() < 1e-9,
                "the {which} set's dragged corner landed {:.4} units from the \
                 pointer: drawn at {grabbed:?}, dragged to {pointer:?}, ended at \
                 {now:?}",
                (now - pointer).hypot()
            );
            assert!(
                (still - held).hypot() < 1e-9,
                "the {which} set's anchored corner moved {:.4} units — a resize \
                 holds the corner opposite the one being dragged: {held:?} → \
                 {still:?}",
                (still - held).hypot()
            );
        }
    }

    /// A guide does not take a press over a lone line's endpoint, and `chrome_claims` agrees
    /// with `begin_select_drag` along the line.
    /// flip: `chrome_claims` reduced to the `handles.armed` term → fails at the endpoint assertion.
    #[test]
    fn a_guide_does_not_steal_a_press_the_chrome_claims() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let line = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: line,
            parent: root,
            index: 0,
            kind: NodeKind::Line {
                end: Point::new(200.0, 0.0),
            },
            transform: Some(Affine::translate((100.0, 100.0))),
            name: None,
        }]))
        .expect("a lone line");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![line]);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        assert!(
            app.selection_handles(area, 1.0).is_none(),
            "the fixture has to be a lone line, or this is about something else"
        );
        let (_, start, _) = app.line_ends(area, 1.0).expect("its endpoints are drawn");
        assert!(
            app.chrome_claims(start, area, 1.0, &None),
            "the endpoint the cursor offers a Grab for has to be claimed"
        );
        assert!(
            !app.chrome_claims(egui::pos2(700.0, 500.0), area, 1.0, &None),
            "and empty canvas is not — a predicate that always says yes would \
             pass everything above"
        );

        let _ = ctx.run_ui(Default::default(), |ui| {
            for k in 0..=40 {
                let at = egui::pos2(start.x + k as f32 * 5.0, start.y);
                let predicted = app.chrome_claims(at, area, 1.0, &None);
                app.drag = Drag::None;
                app.begin_select_drag(ui, at, area, 1.0, &None, false);
                let claimed = matches!(
                    app.drag,
                    Drag::Pivot { .. }
                        | Drag::LineEnd { .. }
                        | Drag::RailOffset { .. }
                        | Drag::Resize { .. }
                        | Drag::Rotate { .. }
                        | Drag::Skew { .. }
                        | Drag::Spacing { .. }
                );
                assert_eq!(
                    predicted, claimed,
                    "at {at:?} the guide gate and the press ladder disagree: \
                     chrome_claims said {predicted}, the ladder claimed {claimed}"
                );
            }
        });
    }

    /// A selected row's spacing bands are claimed by `chrome_claims` exactly where
    /// `begin_select_drag` starts a band drag, swept across a gap, an item and the padding.
    /// flip: `chrome_claims` without its `spacing_at` term → the sweep fails at the first band point.
    #[test]
    fn a_guide_does_not_steal_a_press_a_spacing_band_claims() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let row = ids.mint();
        let mut ops = vec![Operation::CreateNode {
            id: row,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: Some(Affine::translate((100.0, 100.0))),
            name: None,
        }];
        for i in 0..3 {
            ops.push(Operation::CreateNode {
                id: ids.mint(),
                parent: row,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 30.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
        }
        ops.push(Operation::SetDisplay {
            id: row,
            display: Some(ondin_core::container::Display::Flex(
                ondin_core::container::Flex {
                    column_gap: 10.0,
                    padding: [20.0; 4],
                    ..Default::default()
                },
            )),
        });
        let mut doc = Document::new(root);
        doc.apply(&Transaction(ops)).expect("a row");
        app.session.adopt_document(doc, None);
        app.session.selection.set(vec![row]);
        app.canvas_px = (800, 600);
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let handles = app.selection_handles(area, 1.0);
        let at_world = |app: &OndinApp, x: f64, y: f64| app.to_screen(Point::new(x, y), area, 1.0);
        let gap = at_world(&app, 165.0, 135.0);
        assert!(
            app.spacing_at(gap, area, 1.0).is_some(),
            "the fixture: a band at the gap"
        );
        let _ = ctx.run_ui(Default::default(), |ui| {
            for k in 0..=60 {
                let at = at_world(&app, 100.0 + f64::from(k) * 5.0, 135.0);
                // `spacing_at` answers only with no drag in flight.
                app.drag = Drag::None;
                let predicted = app.chrome_claims(at, area, 1.0, &handles);
                app.begin_select_drag(ui, at, area, 1.0, &handles, false);
                let claimed = matches!(
                    app.drag,
                    Drag::Resize { .. }
                        | Drag::Rotate { .. }
                        | Drag::Skew { .. }
                        | Drag::Spacing { .. }
                );
                assert_eq!(
                    predicted, claimed,
                    "at {at:?} the guide gate and the press disagree"
                );
            }
        });
    }

    /// A turned set's handles snap along the frame's axes, not the world's: at 60°
    /// the Left side travels on both world axes, upright on x alone.
    /// flip: `resize_axes`' Selection arm returns `Affine::IDENTITY` → the turned assertion fails.
    #[test]
    fn a_turned_sets_snap_axes_are_its_own() {
        let ctx = egui::Context::default();
        let (mut upright, a, b) = app_with_two_rects(&ctx);
        upright.session.selection.set(vec![a, b]);
        assert_eq!(
            upright.resize_axes(Target::Selection, Handle::Left),
            (true, false),
            "an upright set's Left side travels on x alone"
        );

        let turned = app_with_a_turned_pair(&ctx);
        assert_eq!(
            turned.resize_axes(Target::Selection, Handle::Left),
            (true, true),
            "a 60°-turned set's Left side travels on both world axes"
        );
    }
}

#[cfg(test)]
mod size_badge_lock_tests {
    //! Which lock the size pill reports: `query::is_effectively_locked`, not `Node::locked`,
    //! measured as the pill's width (the padlock reserves `LOCK_PT + LOCK_GAP`).

    use super::*;
    use ondin_core::kurbo::Size;

    /// A 120 × 80 rect inside a group, nothing locked yet.
    fn app_with_a_grouped_rect(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (group, rect) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(120.0, 80.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::translate((200.0, 150.0))),
                name: None,
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(rect);
        (app, group, rect)
    }

    /// The width of the pill's ground, the only filled polygon the badge draws in
    /// `color::BADGE`; asserts the badge drew at all first.
    fn pill_width(app: &OndinApp, ctx: &egui::Context) -> f32 {
        let view = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut drawn = false;
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(view),
                ..Default::default()
            },
            |ui| {
                drawn = app
                    .draw_size_badge(ui.painter(), view, 1.0, egui::Vec2::ZERO)
                    .is_some();
            },
        );
        assert!(
            drawn,
            "no badge was drawn — the fixture is not in the state this test assumes"
        );
        out.shapes
            .into_iter()
            .find_map(|c| match c.shape {
                egui::Shape::Path(p) if p.fill == color::BADGE => {
                    Some(egui::Rect::from_points(&p.points).width())
                }
                _ => None,
            })
            .expect("the pill's ground is a filled polygon in `color::BADGE`")
    }

    /// A layer inside a locked group wears the padlock, as one locked itself does.
    /// flip: the badge reads `n.locked()` → fails at the third (grouped) assertion; the first two only set the scale.
    #[test]
    fn the_padlock_follows_the_effective_lock_not_the_nodes_own_flag() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let (mut app, group, rect) = app_with_a_grouped_rect(&ctx);
        let free = pill_width(&app, &ctx);

        let lock = |app: &mut OndinApp, id, locked| {
            app.session
                .try_commit(Transaction(vec![Operation::SetLocked { id, locked }]))
                .expect("set the lock");
        };

        lock(&mut app, rect, true);
        let own = pill_width(&app, &ctx);
        // `10.0 + 5.0` is `LOCK_PT + LOCK_GAP` copied: both are local to `paint_size_pill`.
        assert_eq!(
            own - free,
            10.0 + 5.0,
            "a directly locked layer's pill is {own} against {free} free, and the \
             padlock is worth `LOCK_PT + LOCK_GAP` — so the badge is not drawing \
             the lock the way `paint_size_pill` says it does"
        );

        lock(&mut app, rect, false);
        lock(&mut app, group, true);
        assert_eq!(
            pill_width(&app, &ctx),
            own,
            "a layer inside a locked group got the unlocked pill: its box will not \
             drag and the one thing on the canvas that says why is silent"
        );
    }
}

#[cfg(test)]
mod baseline_overlay_tests {
    //! A selected text node's baselines are drawn, one per line, and the switch turns them off.

    use super::*;

    /// A text node at (400, 400) holding `lines` lines, selected.
    fn app_with_text(ctx: &egui::Context, body: &str) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let text = ids.mint();
        let parts = crate::fonts::FontService::default_text_parts();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: text,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: body.into(),
                style: Box::new(parts.style),
                spans: parts.spans,
                para_spans: parts.para_spans,
                paragraph: parts.paragraph,
                block: parts.block,
                sizing: parts.sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate((400.0, 400.0))),
            name: None,
        }]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(text);
        app.canvas_px = (800, 600);
        (app, text)
    }

    /// The horizontal hairlines the overlay drew, by their y.
    fn drawn(app: &OndinApp, ctx: &egui::Context) -> Vec<f32> {
        let view = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(view),
                ..Default::default()
            },
            |ui| {
                app.draw_text_baselines(ui.painter(), view, 1.0, egui::Vec2::ZERO);
            },
        );
        out.shapes
            .into_iter()
            .filter_map(|c| match c.shape {
                egui::Shape::LineSegment { points, .. } => Some(points[0].y),
                _ => None,
            })
            .collect()
    }

    /// One hairline is drawn per baseline of the layout (counted against
    /// `layout.baselines`, not a literal), and the switch turns them off.
    /// flip: draw `box_.min_y()` instead of the baseline → the distinct-count assertion fails.
    #[test]
    fn a_selected_text_node_draws_one_hairline_per_baseline() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, text) = app_with_text(&ctx, "a\nb\nc");

        let want = app
            .session
            .preview_text_layout(text)
            .expect("the fixture is a text node")
            .baselines
            .len();
        assert!(
            want > 1,
            "the fixture has {want} baseline(s); a one-per-line claim needs more \
             than one line to be about anything"
        );

        let ys = drawn(&app, &ctx);
        assert_eq!(ys.len(), want, "one hairline per baseline, got {ys:?}");
        let mut distinct = ys.clone();
        distinct.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            want,
            "the hairlines are stacked on one y, so they are being drawn at the \
             box rather than at each baseline: {ys:?}"
        );

        app.snap_baselines = false;
        assert!(
            drawn(&app, &ctx).is_empty(),
            "the switch is off and the overlay drew anyway"
        );
    }

    /// `baseline_lines` skips a rotated node and a hidden one, and keeps a half-turn (still horizontal).
    #[test]
    fn a_rotated_or_hidden_text_node_offers_no_baselines_but_a_half_turn_does() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, text) = app_with_text(&ctx, "a\nb");
        let view = app.session.camera.viewport(app.canvas_px).view;

        let upright = app.baseline_lines(&[], view).len();
        assert!(
            upright >= 2,
            "the fixture offers {upright} baselines; the exclusions below are only \
             about something if the unrotated case offers some"
        );

        let turn = |app: &mut OndinApp, radians: f64| {
            app.session
                .try_commit(Transaction(vec![Operation::SetTransform {
                    id: text,
                    transform: Affine::translate((400.0, 400.0)) * Affine::rotate(radians),
                }]))
                .expect("rotate it");
        };

        turn(&mut app, std::f64::consts::FRAC_PI_4);
        assert!(
            app.baseline_lines(&[], view).is_empty(),
            "a 45° text node offered snap targets; its baselines are slanted and \
             every y here is an axis-aligned coordinate"
        );

        turn(&mut app, std::f64::consts::PI);
        assert_eq!(
            app.baseline_lines(&[], view).len(),
            upright,
            "a half-turn's baselines are still horizontal and were refused anyway \
             — the predicate has become an is-it-unrotated test"
        );

        turn(&mut app, 0.0);
        app.session
            .try_commit(Transaction(vec![Operation::SetVisible {
                id: text,
                visible: false,
            }]))
            .expect("hide it");
        assert!(
            app.baseline_lines(&[], view).is_empty(),
            "a hidden text node offered snap targets"
        );
    }

    /// `baseline_lines` excludes everything being transformed, so a moving group
    /// does not snap to the baseline of the text inside it.
    /// flip: delete the `is_inside_any` clause → fails at the first assertion.
    #[test]
    fn a_moving_group_does_not_snap_to_the_baseline_of_the_text_inside_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, text) = app_with_text(&ctx, "a\nb");
        let view = app.session.camera.viewport(app.canvas_px).view;

        let group = app.session.ids.mint();
        let root = app.session.doc.root();
        app.session
            .try_commit(Transaction(vec![
                Operation::CreateNode {
                    id: group,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Group,
                    transform: None,
                    name: None,
                },
                Operation::Reparent {
                    id: text,
                    new_parent: group,
                    index: 0,
                },
            ]))
            .expect("wrap the text in a group");

        assert!(
            !app.baseline_lines(&[], view).is_empty(),
            "fixture: with nothing moving the text still offers baselines, or the \
             assertion below is about a function that has stopped working"
        );
        assert!(
            app.baseline_lines(&[text], view).is_empty(),
            "control: the direct exclusion was always right"
        );
        assert!(
            app.baseline_lines(&[group], view).is_empty(),
            "and a group carrying the text must not be offered its baselines — \
             the drag would be pulled back to where its own text used to be"
        );
        assert!(
            app.snapping(&[group]).baselines.is_empty(),
            "`snapping` hands the gesture the same answer"
        );
    }

    /// The switch empties the snap source, not only the overlay (`snapping`'s guard).
    #[test]
    fn the_switch_empties_the_snap_source_and_not_only_the_overlay() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, _) = app_with_text(&ctx, "a\nb");

        assert!(
            !app.snapping(&[]).baselines.is_empty(),
            "the fixture: with the switch on, baselines reach `Snapping`"
        );
        app.snap_baselines = false;
        assert!(
            app.snapping(&[]).baselines.is_empty(),
            "the switch is off and baselines still reached the snap source, so a \
             drag goes on being pulled by lines that are no longer drawn"
        );
        assert!(
            !app.snapping(&[]).targets.is_empty(),
            "turning baselines off took the shape targets with it"
        );
    }

    /// The node being dragged does not offer its own baselines, or it would snap to
    /// itself (`snap::targets` excludes `moving` for the same reason).
    #[test]
    fn the_moving_node_is_excluded_from_its_own_baselines() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (app, text) = app_with_text(&ctx, "a\nb");
        let view = app.session.camera.viewport(app.canvas_px).view;

        assert!(
            !app.baseline_lines(&[], view).is_empty(),
            "the fixture: it offers baselines when it is not the one moving"
        );
        assert!(
            app.baseline_lines(&[text], view).is_empty(),
            "the node being dragged offered its own baselines as targets, so it \
             snaps to itself"
        );
    }

    /// Nothing is drawn for a selection that is not text (`preview_text_layout`
    /// returns `None`).
    #[test]
    fn a_shape_selection_draws_no_baselines() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, _) = app_with_text(&ctx, "a\nb");

        let rect = ondin_core::IdSource::new(900).mint();
        app.session
            .try_commit(Transaction(vec![Operation::CreateNode {
                id: rect,
                parent: app.session.doc.root(),
                index: 1,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(50.0, 50.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            }]))
            .expect("add a rect");
        app.session.selection.set_one(rect);
        assert!(
            drawn(&app, &ctx).is_empty(),
            "a rectangle has no baselines and the overlay found some"
        );
    }
}

/// Sliding type along its rail: the handle, a grab on it, and the offset a drag
/// writes. The bend itself is tested in `ondin_core::text`.
#[cfg(test)]
mod rail_drag_tests {
    use super::*;

    /// One text node on a closed 200-unit ring at the origin, so world and local coincide.
    fn app_on_a_ring(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(3);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let text = ids.mint();
        let ring = ondin_core::geometry::local_path(&NodeKind::Ellipse {
            size: ondin_core::kurbo::Size::new(200.0, 200.0),
        })
        .expect("an ellipse has an outline");
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: text,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "AROUND".into(),
                style: Box::new(ondin_core::TextStyle {
                    font_family: "Inter".into(),
                    font_size: 16.0,
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: ondin_core::TextSizing::Auto,
                on_path: Some(Box::new(ring)),
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(text);
        (app, text)
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    fn area() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
    }

    /// The offset a drag to `world` would write.
    fn dragged_to(app: &OndinApp, id: NodeId, world: Point) -> f64 {
        match app.rail_offset_tx(id, world).0.as_slice() {
            [
                Operation::SetGeometry {
                    geometry: ondin_core::GeometryPatch::TextPathOffset(v),
                    ..
                },
            ] => *v,
            other => panic!("expected one offset patch, got {other:?}"),
        }
    }

    /// The handle is drawn where the type starts and moves with it (it reads the
    /// preview layout).
    #[test]
    fn the_handle_sits_where_the_type_starts() {
        let ctx = context();
        let (mut app, id) = app_on_a_ring(&ctx);

        let at_zero = app
            .rail_handle(area(), 1.0)
            .expect("a railed node has one")
            .1;
        let layout = app.session.resolved.text_layout(id).expect("a layout");
        let first = layout.runs[0].glyphs[0];
        let glyph = app.to_screen(
            Point::new(f64::from(first.x), f64::from(first.y)),
            area(),
            1.0,
        );
        assert!(
            at_zero.distance(glyph) < 1.0,
            "the handle is on the type it moves: {at_zero:?} against {glyph:?}"
        );

        app.session.commit(Transaction(vec![Operation::SetGeometry {
            id,
            geometry: ondin_core::GeometryPatch::TextPathOffset(0.25),
        }]));
        let moved = app.rail_handle(area(), 1.0).expect("still railed").1;
        assert!(
            moved.distance(at_zero) > 50.0,
            "a quarter of the way round a 200-unit ring is a long way: {moved:?} \
             against {at_zero:?}"
        );
    }

    /// A drag writes the offset that puts the start under the pointer; a press without
    /// moving, and a drag back to the origin from a committed 0.25, write back what they began with.
    #[test]
    fn a_drag_puts_the_start_under_the_pointer_and_returns_it() {
        let ctx = context();
        let (mut app, id) = app_on_a_ring(&ctx);

        let handle = app.rail_handle(area(), 1.0).expect("a handle").1;
        let still = app.to_world(handle, area(), 1.0);
        assert!(
            dragged_to(&app, id, still).abs() < 0.02,
            "a press that does not move must not move the text: {}",
            dragged_to(&app, id, still)
        );

        // kurbo starts an ellipse at its rightmost point and runs clockwise: the bottom is a quarter round.
        let quarter = dragged_to(&app, id, Point::new(100.0, 200.0));
        assert!(
            (quarter - 0.25).abs() < 0.02,
            "dragging to the ring's bottom puts the start a quarter round: {quarter}"
        );

        app.session.commit(Transaction(vec![Operation::SetGeometry {
            id,
            geometry: ondin_core::GeometryPatch::TextPathOffset(0.25),
        }]));
        let back = dragged_to(&app, id, Point::new(200.0, 100.0));
        assert!(
            back.abs() < 0.02,
            "dragging back to the rail's start returns to where it began: {back}"
        );
    }

    /// A drag on a centred railed node keeps its offset inside one lap (the SVG writer
    /// emits `startOffset`); centred is the fixture, since with `Start` the normalisation is unreachable.
    #[test]
    fn a_drag_on_a_centred_node_stays_inside_one_lap() {
        let ctx = context();
        let (mut app, id) = app_on_a_ring(&ctx);
        app.session
            .commit(Transaction(vec![Operation::SetParagraphStyle {
                id,
                paragraph: ondin_core::ParagraphStyle {
                    align: ondin_core::TextAlign::Center,
                    ..Default::default()
                },
                spans: None,
            }]));

        let at_seam = dragged_to(&app, id, Point::new(200.0, 100.0));
        assert!(
            (0.0..1.0).contains(&at_seam),
            "an offset on a loop stays inside one lap: {at_seam}"
        );
        assert!(
            at_seam > 0.5,
            "and it is the near side of the seam rather than a small positive: \
             {at_seam}"
        );
    }

    /// A grab has to land on the handle, which outranks the box handles at the
    /// press, so a generous radius would take over drags meant to move the layer.
    #[test]
    fn only_a_grab_on_the_handle_is_a_grab() {
        let ctx = context();
        let (app, id) = app_on_a_ring(&ctx);
        let handle = app.rail_handle(area(), 1.0).expect("a handle").1;

        assert_eq!(app.grab_rail_handle(handle, area(), 1.0), Some(id));
        assert_eq!(
            app.grab_rail_handle(handle + egui::vec2(HANDLE_PICK_PX * 3.0, 0.0), area(), 1.0),
            None,
            "three grab radii away is not a grab"
        );
    }

    /// An ordinary text node offers no handle at all (the control).
    #[test]
    fn a_node_with_no_rail_has_no_handle() {
        let ctx = context();
        let (mut app, id) = app_on_a_ring(&ctx);
        app.session.commit(Transaction(vec![Operation::SetGeometry {
            id,
            geometry: ondin_core::GeometryPatch::TextPath(None),
        }]));
        assert!(app.rail_handle(area(), 1.0).is_none());
    }

    /// The stroked path shapes `draw_text_rails` put on the painter.
    fn rails_drawn(app: &OndinApp, ctx: &egui::Context) -> usize {
        let view = area();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(view),
                ..Default::default()
            },
            |ui| {
                app.draw_text_rails(ui.painter(), view, 1.0, egui::Vec2::ZERO);
            },
        );
        out.shapes
            .into_iter()
            .filter(|c| matches!(c.shape, egui::Shape::Path(_)))
            .count()
    }

    /// The node being edited gets one rail, not two: `draw_text_rails` chains the
    /// live session's id onto the selection, which `begin_edit_text` has just set.
    /// flip: drop the `.filter(…)` from `live` → fails at the second assertion (2 against 1).
    #[test]
    fn the_edited_nodes_rail_is_drawn_once() {
        let ctx = context();
        let (mut app, id) = app_on_a_ring(&ctx);

        assert_eq!(rails_drawn(&app, &ctx), 1, "selected only: one rail");

        app.begin_edit_text(Some(id), None);
        assert!(
            app.session.selection.contains(id),
            "the fixture must reach the state: begin_edit_text selects the node it opens on"
        );
        assert_eq!(
            rails_drawn(&app, &ctx),
            1,
            "selected and being edited is still one rail, not two coats of the same stroke"
        );
    }
}

#[cfg(test)]
mod paste_placement_tests {
    //! Where a paste lands and whose child it is, via `paste_placement`, a `&self` function
    //! because the paste commands read the clipboard and cannot be driven headlessly.

    use super::*;
    use ondin_core::kurbo::Size;

    /// A 200×150 frame at world (100, 100) with one 40×40 rect inside it.
    fn app_with_a_frame(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, inside) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(200.0, 150.0),
                },
                transform: Some(Affine::translate((100.0, 100.0))),
                name: Some("Board".into()),
            },
            Operation::CreateNode {
                id: inside,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        (app, frame, inside)
    }

    /// A pointer beats the selection; without one the selected frame decides, and a shape
    /// inside a frame hands back that frame.
    #[test]
    fn a_paste_takes_the_pointer_then_the_selected_frames_centre_then_the_view() {
        let ctx = egui::Context::default();
        let (mut app, frame, inside) = app_with_a_frame(&ctx);
        let frame_centre = Point::new(200.0, 175.0);

        app.session.selection.set(vec![frame]);
        let (centre, _, _) = app.paste_placement(Some(Point::new(7.0, 9.0)));
        assert_eq!(
            centre,
            Point::new(7.0, 9.0),
            "a paste aimed at a place goes to that place"
        );

        let (centre, parent, _) = app.paste_placement(None);
        assert_eq!(centre, frame_centre, "the selected frame's centre");
        assert_eq!(parent, frame, "and the paste is its child");

        app.session.selection.set(vec![inside]);
        let (centre, parent, _) = app.paste_placement(None);
        assert_eq!(
            centre, frame_centre,
            "a shape selected inside a frame takes THAT FRAME's centre — \
             paste_frame walks up to the enclosing artboard (§15 D221)"
        );
        assert_eq!(parent, frame, "and the frame is still the parent");

        app.session.selection.clear();
        let (centre, _, _) = app.paste_placement(None);
        assert_eq!(
            centre,
            app.viewport_centre(),
            "a paste with no pointer and no frame lands in the middle of the view"
        );
    }
}

#[cfg(test)]
mod frame_menu_door_tests {
    //! An occupied frame's name tag is a pick, and the context-menu door asks the same
    //! `pick_at_pointer` as the left-click; asserted on that chain, not `canvas_context_menu`.

    use super::*;
    use ondin_core::kurbo::Size;

    /// A 200×150 frame at world (100, 100) with one rect inside it, so
    /// `is_occupied_frame` is true and `pick_leaf` skips it.
    fn app_with_an_occupied_frame(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, inside) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(200.0, 150.0),
                },
                transform: Some(Affine::translate((100.0, 100.0))),
                name: Some("Board".into()),
            },
            Operation::CreateNode {
                id: inside,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        (app, frame)
    }

    /// A component set's tag is a tab on the frame's top edge; it adds the variant count
    /// while selected and names a clash, each clashing variant leading with the warning glyph.
    #[test]
    fn a_sets_tag_is_a_tab_with_its_count_and_its_clash() {
        use ondin_core::variant::{VariantProp, VariantSet};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, set) = app_with_an_occupied_frame(&ctx);
        let [a, b] = [(); 2].map(|_| app.session.ids.mint());
        let frame = |id, index| Operation::CreateNode {
            id,
            parent: set,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(20.0, 20.0),
            },
            transform: None,
            name: None,
        };
        let props = VariantSet {
            props: vec![VariantProp {
                name: "Size".into(),
                values: vec!["Small".into(), "Large".into()],
            }],
        };
        let mut ops = vec![frame(a, 1), frame(b, 2)];
        for id in [a, b] {
            ops.push(Operation::SetComponent {
                id,
                component: true,
            });
        }
        ops.push(Operation::SetVariantSet {
            id: set,
            set: Some(props),
        });
        ops.push(Operation::SetVariant {
            id: a,
            values: vec!["Small".into()],
        });
        ops.push(Operation::SetVariant {
            id: b,
            values: vec!["Large".into()],
        });
        assert!(app.session.commit(Transaction(ops)));

        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let tag = |app: &OndinApp, id| {
            let mut out = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    let (r, g) = app.frame_label(ui, id, rect, 1.0).expect("a tag");
                    let top = app.selection_quad(id, rect, 1.0).expect("a quad")[0].y;
                    out = Some((r, g.text().to_string(), top));
                },
            );
            out.expect("the closure ran")
        };
        let (r, text, top) = tag(&app, set);
        assert!(
            (r.bottom() - top).abs() < 0.01,
            "fused to the edge: {r:?} over {top}"
        );
        assert!(!text.contains("variants"), "no count at rest: {text:?}");
        app.session.selection.set_one(set);
        assert!(tag(&app, set).1.ends_with("2 variants"));

        assert!(app.session.commit(Transaction(vec![Operation::SetVariant {
            id: b,
            values: vec!["Small".into()],
        }])));
        assert!(tag(&app, set).1.ends_with("2 variants are Small"));
        assert!(tag(&app, b).1.starts_with(icon::WARNING));
        assert!(tag(&app, a).1.starts_with(icon::WARNING));
    }

    /// A selected set's `+` has its own hit route: the middle of the bottom edge,
    /// claimed ahead of the artwork and guides, only while the set is selected.
    /// flip: drop the `+` from `chrome_claims` → fails at "a guide does not take it".
    #[test]
    fn a_selected_sets_plus_is_its_own_target() {
        let ctx = egui::Context::default();
        let (mut app, set) = app_with_a_set(&ctx);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        app.session.selection.set_one(set);
        let (_, at) = app.set_plus(rect, 1.0).expect("a selected set has its +");
        let q = app.selection_quad(set, rect, 1.0).unwrap();
        assert!(
            (at.y - (q[2].y + q[3].y) / 2.0).abs() < 0.01,
            "on the bottom edge: {at:?} {q:?}"
        );
        assert_eq!(app.set_plus_at(at, rect, 1.0), Some(set));
        assert!(
            app.chrome_claims(at, rect, 1.0, &None),
            "a guide does not take it"
        );
        assert_eq!(
            app.set_plus_at(at + egui::vec2(0.0, -40.0), rect, 1.0),
            None,
            "inside the set is the artwork's"
        );
        app.session.selection.clear();
        assert_eq!(app.set_plus_at(at, rect, 1.0), None, "only while selected");
    }

    /// `app_with_an_occupied_frame`'s frame made a component set of one variant,
    /// *Small* — the frame's id, which is the set's.
    fn app_with_a_set(ctx: &egui::Context) -> (OndinApp, NodeId) {
        use ondin_core::variant::{VariantProp, VariantSet};
        crate::theme::install(ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, set) = app_with_an_occupied_frame(ctx);
        let small = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: small,
                parent: set,
                index: 1,
                kind: NodeKind::Artboard {
                    size: Size::new(20.0, 20.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetComponent {
                id: small,
                component: true,
            },
            Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![VariantProp {
                        name: "Size".into(),
                        values: vec!["Small".into()],
                    }],
                }),
            },
            Operation::SetVariant {
                id: small,
                values: vec!["Small".into()],
            },
        ])));
        (app, set)
    }

    /// The variants under `set`, counted.
    fn variants_in(app: &OndinApp, set: NodeId) -> usize {
        let doc = &app.session.doc;
        doc.get(set)
            .map(|n| {
                n.children()
                    .iter()
                    .filter(|c| doc.get(**c).is_some_and(|c| c.component()))
                    .count()
            })
            .unwrap_or(0)
    }

    /// A click on a selected set's `+` adds a variant through `normal_mode_input`;
    /// a click inside the set, off the disc, adds nothing.
    /// flip: delete the `+` arm from `normal_mode_input` → fails at "the click on the + adds one".
    #[test]
    fn a_click_on_a_sets_plus_adds_a_variant() {
        const AREA: egui::Rect = egui::Rect {
            min: egui::Pos2::ZERO,
            max: egui::Pos2::new(800.0, 600.0),
        };
        let ctx = egui::Context::default();
        let (mut app, set) = app_with_a_set(&ctx);
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(AREA),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let resp = ui.interact(
                    AREA,
                    egui::Id::new("set-plus-probe"),
                    egui::Sense::click_and_drag(),
                );
                app.normal_mode_input(ui, &resp, AREA, 1.0);
            });
        };
        let click = |app: &mut OndinApp, at: egui::Pos2| {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(app, vec![egui::Event::PointerMoved(at)]);
            frame(app, vec![button(true)]);
            frame(app, vec![button(false)]);
        };
        app.session.selection.set_one(set);
        let (_, at) = app.set_plus(AREA, 1.0).expect("the +");
        assert_eq!(variants_in(&app, set), 1, "the fixture: one variant");

        click(&mut app, at + egui::vec2(0.0, -40.0));
        assert_eq!(
            variants_in(&app, set),
            1,
            "control: a click inside the set, off the disc, adds nothing"
        );
        app.session.selection.set_one(set);
        click(&mut app, at);
        assert_eq!(variants_in(&app, set), 2, "the click on the + adds one");
    }

    /// A press-drag on a selected set's `+` starts nothing and the cursor over it is the
    /// arrow; the disc sits on the bottom-middle resize handle, which only
    /// `begin_select_drag`'s early return keeps from arming.
    /// flip: `begin_select_drag`'s guard made `if false && …` → fails at "the set keeps its box";
    /// `select_cursor`'s arm made `&& false &&` → fails at "the arrow over the disc".
    #[test]
    fn a_press_drag_on_a_sets_plus_starts_nothing_and_shows_the_arrow() {
        const AREA: egui::Rect = egui::Rect {
            min: egui::Pos2::ZERO,
            max: egui::Pos2::new(800.0, 600.0),
        };
        let ctx = egui::Context::default();
        let (mut app, set) = app_with_a_set(&ctx);
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(AREA),
                events,
                ..Default::default()
            };
            let mut cursor = egui::CursorIcon::Default;
            let _ = ctx.run_ui(input, |ui| {
                let resp = ui.interact(
                    AREA,
                    egui::Id::new("set-plus-drag-probe"),
                    egui::Sense::click_and_drag(),
                );
                app.normal_mode_input(ui, &resp, AREA, 1.0);
                cursor = app.select_cursor(&resp, AREA, 1.0).0;
            });
            cursor
        };
        app.session.selection.set_one(set);
        let (_, at) = app.set_plus(AREA, 1.0).expect("the +");
        let before = app
            .session
            .resolved
            .world_bounds(set)
            .expect("the set's box");
        let variants = variants_in(&app, set);

        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        let cursor = frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        assert_eq!(cursor, egui::CursorIcon::Default, "the arrow over the disc");

        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, vec![button(at, true)]);
        for step in 1..=10 {
            let p = at + egui::vec2(0.0, 4.0 * step as f32);
            frame(&mut app, vec![egui::Event::PointerMoved(p)]);
        }
        let end = at + egui::vec2(0.0, 40.0);
        frame(&mut app, vec![button(end, false)]);
        frame(&mut app, Vec::new());

        assert_eq!(
            app.session.resolved.world_bounds(set),
            Some(before),
            "the set keeps its box"
        );
        assert_eq!(variants_in(&app, set), variants, "and no variant was added");
        assert!(matches!(app.drag, Drag::None), "nothing is in flight");
    }

    /// The components chrome, read off the frame's shapes: a main's hexagon chip, an
    /// instance's hairlines when its main is selected, an entered instance's dashed edge
    /// and *Esc to exit* tag, a selected set's `+` disc.
    /// flip: drop `paint_hexagon_filled` → fails at "one filled hexagon"; draw the entered
    /// edge as four solid segments → fails at "dashed" (run alone, since the first flip hides it).
    #[test]
    fn the_components_chrome_paints_its_chip_hexagon_hairlines_dashes_and_disc() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, main) = app_with_an_occupied_frame(&ctx);
        let inst = main_with_an_instance(&mut app, main);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let shapes = |app: &OndinApp, labels: bool| {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    if labels {
                        app.draw_frame_labels(ui, ui.painter(), rect, 1.0);
                    } else {
                        app.draw_component_chrome(ui.painter(), rect, 1.0);
                        app.draw_set_plus(ui.painter(), rect, 1.0);
                    }
                },
            );
            out.shapes.into_iter().map(|c| c.shape).collect::<Vec<_>>()
        };
        let segments = |shapes: &[egui::Shape], ink: egui::Color32| {
            shapes
                .iter()
                .filter(
                    |s| matches!(s, egui::Shape::LineSegment { stroke, .. } if stroke.color == ink),
                )
                .count()
        };

        // The tags, the instance selected: the only state its tag shows in besides entered.
        app.session.selection.set_one(inst);
        let tags = shapes(&app, true);
        let chips = tags
            .iter()
            .filter(|s| matches!(s, egui::Shape::Rect(r) if r.fill == color::COMPONENT))
            .count();
        assert_eq!(chips, 1, "one chip — the main's, not the instance's");
        let hexagons = tags
            .iter()
            .filter(|s| {
                matches!(s, egui::Shape::Path(p)
                    if p.closed && p.points.len() == 6 && p.fill == color::COMPONENT_INK)
            })
            .count();
        assert_eq!(hexagons, 1, "one filled hexagon, on the main's chip");
        let mut inks: Vec<_> = tags
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) => Some(t.fallback_color),
                _ => None,
            })
            .collect();
        inks.sort_by_key(|c| c.to_array());
        let mut want = vec![color::COMPONENT_INK, color::COMPONENT];
        want.sort_by_key(|c| c.to_array());
        assert_eq!(
            inks, want,
            "the chip's ink and the instance's violet tag (§15 D1001)"
        );

        app.session.selection.set_one(main);
        let chrome = shapes(&app, false);
        assert_eq!(
            segments(&chrome, theme::text::FAINT),
            4,
            "four hairlines round the one instance"
        );

        app.session.selection.clear();
        app.entered_group = Some(inst);
        let chrome = shapes(&app, false);
        assert!(
            segments(&chrome, theme::text::DIM) > 8,
            "dashed: many short segments, not four sides — {}",
            segments(&chrome, theme::text::DIM)
        );
        assert!(
            chrome.iter().any(|s| matches!(s, egui::Shape::Text(t)
                if t.galley.text().contains("Esc to exit"))),
            "and the tag under it"
        );
        app.entered_group = None;

        let (mut app, set) = app_with_a_set(&ctx);
        app.session.selection.set_one(set);
        let (_, at) = app.set_plus(rect, 1.0).expect("the +");
        let chrome = shapes(&app, false);
        assert!(
            chrome.iter().any(|s| matches!(s, egui::Shape::Circle(c)
                if c.fill == theme::color::CARD && c.center == at && c.radius == SET_PLUS_R)),
            "the disc on the set's bottom edge"
        );
        assert_eq!(
            segments(&chrome, theme::text::STRONG),
            2,
            "and its two arms"
        );
    }

    /// `main` made a main by *Create component*, plus an instance of it 300 to its right; returns the instance.
    fn main_with_an_instance(app: &mut OndinApp, main: NodeId) -> NodeId {
        app.session.selection.set_one(main);
        app.create_component();
        assert!(
            app.session.doc.get(main).is_some_and(|n| n.component()),
            "the fixture's main"
        );
        let doc = &app.session.doc;
        let (tx, made) = ondin_core::insert_subtrees(
            doc,
            &mut app.session.ids,
            &[ondin_core::Placement {
                nodes: doc.capture_subtree(main).expect("the main"),
                parent: doc.root(),
                index: None,
            }],
            ondin_core::kurbo::Vec2::new(300.0, 0.0),
        );
        assert!(app.session.commit(tx));
        made[0]
    }

    /// A frame's tag names the mode it sets after its name (*Board · Dark*), not
    /// one it inherits.
    #[test]
    fn a_frames_tag_names_the_mode_it_sets_and_not_one_it_inherits() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, frame) = app_with_an_occupied_frame(&ctx);
        let (light, dark) = (app.session.ids.mint_mode(), app.session.ids.mint_mode());
        let mode = |id, name: &str| ondin_core::Mode {
            id,
            name: name.into(),
        };
        assert!(app.session.commit(Transaction(vec![
            Operation::AddMode {
                mode: mode(light, "Light"),
                index: 0,
                cells: vec![],
            },
            Operation::AddMode {
                mode: mode(dark, "Dark"),
                index: 1,
                cells: vec![],
            },
        ])));
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let tag = |app: &OndinApp| {
            let mut out = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    let (_, g) = app.frame_label(ui, frame, rect, 1.0).expect("a tag");
                    out = Some(g.text().to_string());
                },
            );
            out.expect("the closure ran")
        };
        assert!(
            !tag(&app).contains('·'),
            "an inherited mode stays off the canvas"
        );
        assert!(app.session.commit(Transaction(vec![Operation::SetMode {
            id: frame,
            mode: Some(dark),
        }])));
        assert!(tag(&app).ends_with("· Dark"), "{:?}", tag(&app));
        // A main's chip names the mode too; flip: the main's arm without `mode_word` fails here.
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetComponent {
                    id: frame,
                    component: true,
                }]))
        );
        assert!(
            tag(&app).contains("· Dark"),
            "a main's chip: {:?}",
            tag(&app)
        );
    }

    /// A main's tag is a hexagon chip counting its instances while selected; an instance's
    /// trails its main's name, or is a path when entered; neither turns `color::SELECT`.
    /// flip: `draw_frame_labels` without the two component arms → fails at the hue assertion.
    #[test]
    fn a_mains_tag_is_a_chip_and_an_instances_tag_names_its_main() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, main) = app_with_an_occupied_frame(&ctx);
        let inst = main_with_an_instance(&mut app, main);
        assert!(app.session.commit(Transaction(vec![Operation::SetName {
            id: inst,
            name: "Mine".into(),
        }])));
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let tag = |app: &OndinApp, id| {
            let mut out = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    let (_, g) = app.frame_label(ui, id, rect, 1.0).expect("a tag");
                    out = Some(g.text().to_string());
                },
            );
            out.expect("the closure ran")
        };
        app.session.selection.clear();
        let text = tag(&app, main);
        assert!(
            text.starts_with(icon::HEXAGON) && text.ends_with("Board"),
            "the glyph's cell, then the name: {text:?}"
        );
        app.session.selection.set_one(main);
        assert!(tag(&app, main).ends_with("1 instance"));
        app.session.selection.set_one(inst);
        let text = tag(&app, inst);
        assert!(
            text.starts_with(icon::HEXAGON) && text.contains("Mine") && text.ends_with("Board"),
            "renamed, it trails its main's name: {text:?}"
        );
        let child = app.session.doc.get(inst).unwrap().children()[0];
        let child_name = app.session.doc.get(child).unwrap().name().to_string();
        app.entered_group = Some(inst);
        app.session.selection.set_one(child);
        let text = tag(&app, inst);
        assert!(
            text.contains('›') && text.ends_with(&child_name),
            "entered, a path: {text:?}"
        );
        app.entered_group = None;

        app.session.selection.set(vec![main, inst]);
        let mut inks = Vec::new();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ui| app.draw_frame_labels(ui, ui.painter(), rect, 1.0),
        );
        for clipped in &out.shapes {
            if let egui::Shape::Text(t) = &clipped.shape {
                inks.push(t.fallback_color);
            }
        }
        assert_eq!(inks.len(), 2, "two tags painted: {inks:?}");
        assert!(
            !inks.contains(&color::SELECT),
            "a component's tag never takes the accent: {inks:?}"
        );
    }

    /// No tag inside an instance or on an instance inside a main, while a plain
    /// frame inside a main keeps its tag (occupied, it is that frame's handle).
    /// flip: `frame_label` without `tag_withheld` → fails at "an instance inside the main";
    /// `tag_withheld` withholding below any main → fails at "a plain frame inside the main".
    #[test]
    fn no_tag_inside_an_instance_nor_on_an_instance_inside_a_main() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, main) = app_with_an_occupied_frame(&ctx);
        let (inner, icon) = (app.session.ids.mint(), app.session.ids.mint());
        let root = app.session.doc.root();
        let frame = |id, parent, index, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(30.0, 20.0),
            },
            transform: Some(Affine::translate((8.0, 8.0))),
            name: Some(name.into()),
        };
        assert!(app.session.commit(Transaction(vec![
            frame(inner, main, 1, "Inner"),
            frame(icon, root, 1, "Icon"),
        ])));
        app.session.selection.set_one(icon);
        app.create_component();
        let doc = &app.session.doc;
        let (tx, made) = ondin_core::insert_subtrees(
            doc,
            &mut app.session.ids,
            &[ondin_core::Placement {
                nodes: doc.capture_subtree(icon).expect("the icon"),
                parent: main,
                index: None,
            }],
            ondin_core::kurbo::Vec2::ZERO,
        );
        assert!(app.session.commit(tx));
        let nested = made[0];
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let tagged = |app: &mut OndinApp, id| {
            app.session.selection.set_one(id);
            let mut out = false;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| out = app.frame_label(ui, id, rect, 1.0).is_some(),
            );
            out
        };
        assert!(
            tagged(&mut app, nested),
            "the control: inside a plain frame"
        );

        let inst = main_with_an_instance(&mut app, main);
        let copy = |app: &OndinApp, name: &str| {
            *app.session
                .doc
                .get(inst)
                .unwrap()
                .children()
                .iter()
                .find(|c| app.session.doc.get(**c).is_some_and(|n| n.name() == name))
                .expect("the instance's copy")
        };
        let (inner_copy, icon_copy) = (copy(&app, "Inner"), copy(&app, "Icon"));
        assert!(tagged(&mut app, main), "the main's chip");
        assert!(tagged(&mut app, icon), "the other main's chip");
        assert!(tagged(&mut app, inst), "the instance's tag");
        assert!(tagged(&mut app, inner), "a plain frame inside the main");
        assert!(!tagged(&mut app, nested), "an instance inside the main");
        assert!(!tagged(&mut app, inner_copy), "a frame inside the instance");
        assert!(
            !tagged(&mut app, icon_copy),
            "an instance inside the instance"
        );
    }

    /// An instance's tag shows only while selected or entered (a frame's name and a
    /// main's chip always show), and a selected main does not bring it along.
    /// flip: `frame_label` without its selected-or-entered return → fails at "unselected, no tag".
    #[test]
    fn an_instances_tag_shows_only_while_it_is_selected_or_entered() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, main) = app_with_an_occupied_frame(&ctx);
        let plain = app.session.ids.mint();
        let root = app.session.doc.root();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: plain,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(50.0, 50.0),
            },
            transform: Some(Affine::translate((100.0, 400.0))),
            name: Some("Plain".into()),
        }])));
        let inst = main_with_an_instance(&mut app, main);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let tagged = |app: &OndinApp, id| {
            let mut out = false;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| out = app.frame_label(ui, id, rect, 1.0).is_some(),
            );
            out
        };

        app.session.selection.clear();
        assert!(
            tagged(&app, plain),
            "the control: a frame's name, unselected"
        );
        assert!(tagged(&app, main), "the control: a main's chip, unselected");
        assert!(!tagged(&app, inst), "unselected, no tag");
        app.session.selection.set_one(main);
        assert!(!tagged(&app, inst), "nor with only its main selected");
        app.session.selection.set_one(inst);
        assert!(tagged(&app, inst), "selected, its tag");
        let child = app.session.doc.get(inst).unwrap().children()[0];
        app.entered_group = Some(inst);
        app.session.selection.set_one(child);
        assert!(tagged(&app, inst), "entered, its path");
    }

    /// A selected main's outline, handles and size badge take `COMPONENT`; a layer inside
    /// keeps `SELECT`; a mixed selection keeps each outline's hue, handles and badge `SELECT`.
    /// flip: `chrome_hue` always answering `SELECT` → fails at "the main's outline".
    #[test]
    fn a_components_selection_chrome_takes_the_component_hue() {
        use egui::epaint::ColorMode;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, main) = app_with_an_occupied_frame(&ctx);
        let _inst = main_with_an_instance(&mut app, main);
        let child = app.session.doc.get(main).unwrap().children()[0];
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let shapes = |app: &mut OndinApp| {
            let mut shapes = Vec::new();
            for _ in 0..2 {
                shapes = ctx
                    .run_ui(
                        egui::RawInput {
                            screen_rect: Some(rect),
                            ..Default::default()
                        },
                        |ui| app.canvas_ui(ui),
                    )
                    .shapes;
            }
            shapes.into_iter().map(|c| c.shape).collect::<Vec<_>>()
        };
        let outlines = |s: &[egui::Shape], hue| {
            s.iter()
                .filter(|s| {
                    matches!(s, egui::Shape::Path(p)
                        if p.closed && p.fill == egui::Color32::TRANSPARENT
                            && p.stroke.color == ColorMode::Solid(hue))
                })
                .count()
        };
        let handles = |s: &[egui::Shape], hue| {
            s.iter()
                .filter(|s| matches!(s, egui::Shape::Rect(r) if r.stroke.color == hue))
                .count()
        };
        let badge = |s: &[egui::Shape], hue| {
            s.iter()
                .any(|s| matches!(s, egui::Shape::Path(p) if p.closed && p.fill == hue))
        };

        app.session.selection.set_one(main);
        let s = shapes(&mut app);
        assert_eq!(outlines(&s, color::COMPONENT), 1, "the main's outline");
        assert_eq!(handles(&s, color::COMPONENT), 4, "its four handles");
        assert!(badge(&s, color::COMPONENT), "its size badge");

        app.session.selection.set_one(child);
        let s = shapes(&mut app);
        assert_eq!(
            outlines(&s, color::SELECT),
            1,
            "a layer inside keeps SELECT"
        );
        assert_eq!(outlines(&s, color::COMPONENT), 0);
        assert!(badge(&s, color::BADGE));

        app.session.selection.set(vec![main, child]);
        let s = shapes(&mut app);
        assert_eq!(outlines(&s, color::COMPONENT), 1, "mixed: the main's own");
        assert_eq!(handles(&s, color::COMPONENT), 0, "mixed: SELECT handles");
        assert!(badge(&s, color::BADGE), "mixed: the plain badge");
    }

    /// A double-click on an instance's own background steps in, as `Enter` does.
    /// flip: `double_click_pick` without its instance arm → `entered_group` stays empty.
    #[test]
    fn a_double_click_on_an_instances_background_enters_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, main) = app_with_an_occupied_frame(&ctx);
        let inst = main_with_an_instance(&mut app, main);
        let middle = app
            .session
            .resolved
            .world_bounds(inst)
            .expect("bounds")
            .center();
        assert_eq!(
            app.pick_leaf(middle),
            Some(inst),
            "the fixture: its background"
        );
        app.double_click_pick(Some(inst), middle);
        assert_eq!(app.entered_group, Some(inst));
    }

    /// An instance picks like a group, its empty background included, while its
    /// main (an ordinary occupied frame) leaves the background to the marquee.
    /// flip: `pick_leaf` without its instance clause → `None` on the instance's background.
    #[test]
    fn an_instance_picks_like_a_group() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, main) = app_with_an_occupied_frame(&ctx);
        let inst = main_with_an_instance(&mut app, main);
        let middle = |id| {
            app.session
                .resolved
                .world_bounds(id)
                .expect("bounds")
                .center()
        };
        assert_eq!(app.pick_leaf(middle(main)), None, "a main is a frame (D22)");
        assert_eq!(app.pick_leaf(middle(inst)), Some(inst));
        let child = app.session.doc.get(inst).unwrap().children()[0];
        assert_eq!(
            ondin_core::group_chain(&app.session.doc, child, None),
            vec![inst],
            "a click on its child resolves to the instance"
        );
        let main_child = app.session.doc.get(main).unwrap().children()[0];
        assert!(
            ondin_core::group_chain(&app.session.doc, main_child, None).is_empty(),
            "a main's child stays directly clickable"
        );
    }

    /// An occupied frame's name tag resolves to the frame through `pick_at_pointer`
    /// (the tag is its only handle); the emptied frame is the control.
    /// flip: `pick_at_pointer` body replaced by `self.pick_leaf(world)` → fails at the first assertion (`None`).
    #[test]
    fn an_occupied_frames_name_tag_resolves_to_the_frame() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (mut app, frame) = app_with_an_occupied_frame(&ctx);

        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let ppp = 1.0;
        let mut answers = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ui| {
                let tag = app
                    .frame_label(ui, frame, rect, ppp)
                    .expect("the frame draws a name tag")
                    .0
                    .center();
                let world = app.to_world(tag, rect, ppp);
                answers = Some((
                    app.pick_at_pointer(ui, tag, world, rect, ppp),
                    app.pick_leaf(world),
                ));
            },
        );
        let (chain, leaf) = answers.expect("the closure ran");

        assert_eq!(
            chain,
            Some(frame),
            "the tag is the only handle an occupied frame has, and both doors must \
             resolve it to the frame"
        );
        assert_eq!(
            leaf, None,
            "fixture: `pick_leaf` alone answers nothing there, which is what sent \
             the menu to `Target::Canvas`"
        );

        let inside = app.session.doc.get(frame).expect("frame").children()[0];
        app.session
            .commit(Transaction(vec![Operation::DeleteNode { id: inside }]));
        assert_eq!(
            app.pick_leaf(Point::new(200.0, 175.0)),
            Some(frame),
            "control: an empty frame is picked by `pick_leaf` on its own body, so \
             the tag was never its only door — this test is about the occupied case"
        );
    }

    /// Where two name tags coincide, the tag painted last (the inner frame's) is the one a
    /// click picks; paint order and pick are asserted together so neither can be flipped alone.
    /// flip: remove `.rev()` from `frame_label_at` → fails at the pick assertion (`Page` for `Header`).
    #[test]
    fn the_name_tag_you_can_read_is_the_one_a_click_picks() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (page, header) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: page,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: Some("Page".into()),
            },
            Operation::CreateNode {
                id: header,
                parent: page,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 80.0),
                },
                transform: None,
                name: Some("Header".into()),
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);

        assert_eq!(
            app.artboards(),
            vec![page, header],
            "the fixture rests on `artboards()` being depth-first paint order, so \
             the inner frame is last — if that changes, this test is about \
             something else"
        );

        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let ppp = 1.0;
        let mut answers = None;
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ui| {
                let outer = app
                    .frame_label(ui, page, rect, ppp)
                    .expect("the page draws a name tag")
                    .0;
                let inner = app
                    .frame_label(ui, header, rect, ppp)
                    .expect("the header draws a name tag")
                    .0;
                let over = outer.intersect(inner);
                let at = over.center();
                answers = Some((
                    over,
                    at,
                    app.frame_label_at(ui, at, rect, ppp),
                    // Inside both bodies: a tag is drawn outside its frame, so `target_artboard` answers `None` at it.
                    app.target_artboard(Point::new(200.0, 40.0))
                        .map(|(id, _)| id),
                ));
                app.draw_frame_labels(ui, ui.painter(), rect, ppp);
            },
        );
        let (over, at, picked, target) = answers.expect("the closure ran");

        assert!(
            over.width() > 1.0 && over.height() > 1.0,
            "the fixture has to put the two tags on top of each other, or there is \
             no ambiguity to resolve; the overlap was {over:?}"
        );

        let on_screen = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t)
                    if t.galley.rect.translate(t.pos.to_vec2()).contains(at) =>
                {
                    Some(t.galley.text().to_owned())
                }
                _ => None,
            })
            .next_back();
        assert_eq!(
            on_screen.as_deref(),
            Some("Header"),
            "the inner frame's tag is painted last and is therefore the only word \
             legible at that point"
        );

        assert_eq!(
            picked,
            Some(header),
            "and a click there has to take the frame whose name is under the \
             pointer. `Page` means the word on screen and the frame selected are \
             two different frames — and `hover_target` outlined the wrong one \
             before the click"
        );

        // The control, green either way: every other reader of `artboards()` answers the inner frame.
        assert_eq!(
            target,
            Some(header),
            "control: `target_artboard` takes the last match at the same point"
        );
    }
}

#[cfg(test)]
mod locked_picture_tests {
    //! A locked picture refuses crop (`edited_image`) and replacement (`image_drop_target`),
    //! guarded separately: the layers panel can bypass `begin_image_edit`.

    use super::*;
    use ondin_core::kurbo::Size;
    use ondin_core::{Fill, image_brush};

    /// A 100×100 rect carrying one picture fill, selected, not yet locked.
    fn app_with_a_picture(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: image_brush(ondin_core::ImageId("a-photo".into())).into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(rect);
        (app, rect)
    }

    /// A locked picture is neither croppable nor replaceable; unlocked, both doors
    /// answer `Some` (the control).
    /// flip: remove either guard → fails at its own assertion (crop or drop), the other stays green.
    #[test]
    fn a_locked_picture_is_neither_croppable_nor_replaceable() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let (mut app, rect) = app_with_a_picture(&ctx);
        app.tool = Tool::ImageEdit;

        assert_eq!(
            app.edited_image(),
            Some((rect, 0)),
            "control: an unlocked picture is croppable"
        );
        assert_eq!(
            app.image_drop_target(Some(Point::new(50.0, 50.0))),
            Some((rect, 0)),
            "control: and its fill is a drop target"
        );
        assert_eq!(
            app.image_drop_target(None),
            Some((rect, 0)),
            "control: the paste arm too, which is addressed by the selection alone"
        );

        app.session.commit(Transaction(vec![Operation::SetLocked {
            id: rect,
            locked: true,
        }]));
        assert!(
            ondin_core::is_effectively_locked(&app.session.doc, rect),
            "fixture: the lock is on"
        );
        assert_eq!(
            app.session.selection.ids(),
            [rect],
            "fixture: and the selection survived it, which is what makes this reachable"
        );

        assert_eq!(
            app.edited_image(),
            None,
            "a locked picture is not croppable — §5.5a: a crop is a resize"
        );
        assert_eq!(
            app.image_drop_target(Some(Point::new(50.0, 50.0))),
            None,
            "and a drop may not replace it"
        );
        assert_eq!(
            app.image_drop_target(None),
            None,
            "nor the paste chord, which is the same funnel by a different arm"
        );
    }
}

#[cfg(test)]
mod locked_path_verb_tests {
    //! A locked path refuses `reverse_subpaths` on both its doors: `edited_path`
    //! (the node tool's path) and the `or_else` fallback for a single selected `Path`.

    use super::*;

    /// A three-anchor open path, selected, not yet locked.
    fn app_with_a_path(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let mut p = ondin_core::kurbo::BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((40.0, 0.0));
        p.line_to((40.0, 30.0));
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: p,
                corner_radii: Vec::new(),
            },
            transform: None,
            name: None,
        }]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        (app, id)
    }

    fn stored(app: &OndinApp, id: NodeId) -> String {
        let NodeKind::Path { path, .. } = app.session.doc.get(id).expect("a live node").kind()
        else {
            panic!("a Path");
        };
        path.to_svg()
    }

    /// `Shift+D` on a locked, merely selected path changes nothing and spends no
    /// undo step; the `Select` tool is the fixture because `Node` reaches the first door.
    /// flip: remove the lock check from the `or_else` arm → fails at "a locked path is not reversed".
    #[test]
    fn a_locked_path_is_not_reversed_by_the_selection_door() {
        let ctx = egui::Context::default();

        let (mut app, id) = app_with_a_path(&ctx);
        app.tool = Tool::Select;
        let before = stored(&app, id);
        app.reverse_subpaths();
        assert_ne!(
            stored(&app, id),
            before,
            "control: the verb reverses an unlocked selected path"
        );

        let (mut app, id) = app_with_a_path(&ctx);
        app.tool = Tool::Select;
        app.session
            .try_commit(Transaction(vec![Operation::SetLocked { id, locked: true }]))
            .expect("locking commits");
        assert_eq!(
            app.edited_path(),
            None,
            "fixture: the first door already refuses, so this is only about the second"
        );
        let before = stored(&app, id);
        let depth = app.session.history.undo_depth();
        app.reverse_subpaths();
        assert_eq!(stored(&app, id), before, "a locked path is not reversed");
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "and no undo step is spent on it"
        );
    }
}

#[cfg(test)]
mod crop_click_tests {
    //! A click in image-edit mode commits nothing: `crop_tx` writes `ImageFit::Crop`
    //! unconditionally, so a zero-delta pan would change the picture's mode.

    use super::*;
    use ondin_core::kurbo::Size;
    use ondin_core::{Fill, ImageEntry, ImageFit, ImageFormat, ImageSource, image_brush};

    /// A 100×100 rect carrying one picture fill, selected, with image editing armed
    /// and the picture's entry in the document (`crop_state` needs its shape).
    fn app_cropping(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        let image = ondin_core::ImageId("a-photo".into());
        doc.apply(&Transaction(vec![
            Operation::AddImage {
                id: image.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(vec![0u8; 8].into()),
                    format: ImageFormat::Png,
                    width: 200,
                    height: 100,
                },
            },
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: image_brush(image).into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(rect);
        app.tool = Tool::ImageEdit;
        app.canvas_px = (400, 300);
        app.session.camera.zoom = 1.0;
        app.session.camera.center = ondin_core::kurbo::Point::new(50.0, 50.0);
        (app, rect)
    }

    fn fit(app: &OndinApp, id: NodeId) -> ImageFit {
        match app.session.doc.get(id).expect("the rect").paint().fills[0]
            .brush
            .literal()
        {
            Some(ondin_core::Brush::Image(img)) => img.image.fit,
            other => panic!("the fixture paints a picture, not {other:?}"),
        }
    }

    /// One frame of `image_edit_input` alone, over a canvas-shaped response.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let size = egui::vec2(400.0, 300.0);
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                let resp = ui.interact(
                    rect,
                    egui::Id::new("crop-click-canvas"),
                    egui::Sense::click_and_drag(),
                );
                app.image_edit_input(ui, &resp, rect, 1.0);
            },
        );
    }

    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// A press and release with no travel commits nothing and changes no mode (asserted
    /// first, since the transaction itself is not a no-op: it writes `Crop`).
    /// flip: commit unconditionally → fails at the mode assertion (`Crop` for `Fill`).
    #[test]
    fn a_click_in_image_edit_mode_commits_nothing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, id) = app_cropping(&ctx);
        let depth0 = app.session.history.undo_depth();
        assert_eq!(fit(&app, id), ImageFit::Fill, "the fixture is a Fill");

        let at = egui::pos2(200.0, 150.0);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut app, vec![click(at, true)]);
        frame(&ctx, &mut app, vec![click(at, false)]);
        frame(&ctx, &mut app, Vec::new());

        assert_eq!(
            fit(&app, id),
            ImageFit::Fill,
            "a click did not silently change the picture's mode"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth0,
            "and left no undo step behind it"
        );
    }

    /// Control for the test above: a drag that travels still commits and flips the mode.
    #[test]
    fn a_travelled_crop_pan_still_commits() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, id) = app_cropping(&ctx);
        let depth0 = app.session.history.undo_depth();

        let at = egui::pos2(200.0, 150.0);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut app, vec![click(at, true)]);
        assert!(
            matches!(app.drag, Drag::CropPan { .. }),
            "the press armed a pan, which is the state both tests are about"
        );
        for step in 1..=3 {
            let to = at + egui::vec2(10.0 * step as f32, 0.0);
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(to)]);
        }
        let end = at + egui::vec2(30.0, 0.0);
        frame(&ctx, &mut app, vec![click(end, false)]);

        assert_eq!(fit(&app, id), ImageFit::Crop, "a real pan crops");
        assert_eq!(
            app.session.history.undo_depth() - depth0,
            1,
            "and is one undo step"
        );
    }
}

#[cfg(test)]
mod empty_text_session_tests {
    //! Which text nodes an empty close removes: only a node the session created,
    //! not any node whose original content was already empty.

    use super::*;

    fn text_kind() -> NodeKind {
        let parts = ondin_core::TextParts::default();
        NodeKind::Text {
            content: "hello".into(),
            style: Box::new(parts.style.clone()),
            spans: parts.spans.clone(),
            para_spans: parts.para_spans.clone(),
            paragraph: parts.paragraph.clone(),
            block: parts.block,
            sizing: parts.sizing,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    fn app_with_a_text(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x7E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: text_kind(),
            transform: None,
            name: None,
        }]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        (app, id)
    }

    fn content_of(app: &OndinApp, id: NodeId) -> Option<String> {
        let NodeKind::Text { content, .. } = app.session.doc.get(id)?.kind() else {
            return None;
        };
        Some(content.clone())
    }

    /// A layer emptied in an earlier session survives an open-and-close with nothing
    /// typed; a node created by the session and left empty is still removed (the control).
    /// flip: `session.created` back to `session.original.0.is_empty()` → fails at "the layer survives a second session that typed nothing".
    #[test]
    fn a_layer_emptied_in_an_earlier_session_is_not_deleted_by_opening_it() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_text(&ctx);

        app.begin_edit_text(Some(id), None);
        {
            let s = app.text.as_mut().expect("a session");
            s.editor.select_all();
            s.editor.backspace();
        }
        app.finish_text_edit();
        assert_eq!(
            content_of(&app, id).as_deref(),
            Some(""),
            "session 1: emptying a layer leaves it in the document"
        );
        let depth = app.session.history.undo_depth();
        assert!(depth >= 1, "session 1: and it is an edit, not a no-op");

        app.begin_edit_text(Some(id), None);
        app.finish_text_edit();
        assert_eq!(
            content_of(&app, id).as_deref(),
            Some(""),
            "the layer survives a second session that typed nothing"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "and no undo step is spent"
        );

        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x7F);
        let root = ids.mint();
        app.session.adopt_document(Document::new(root), None);
        app.session.ids = ids;
        app.tool = Tool::Text;
        app.begin_text(Point::new(10.0, 10.0));
        let made = app.text.as_ref().expect("a session on a new node").id;
        assert!(
            app.text.as_ref().is_some_and(|s| s.created),
            "control fixture: the Text tool's click creates the node it opens"
        );
        app.finish_text_edit();
        assert!(
            app.session.doc.get(made).is_none(),
            "control: a node created and left empty is still removed"
        );
    }
}

#[cfg(test)]
mod boolean_outline_tests {
    //! The hover and key-layer ring over a boolean traces its silhouette on the
    //! page, not every operand's outline.

    use super::*;

    /// A boolean of two 100×100 rects, the second offset by `offset`. `Subtract` at `(50, 50)`
    /// gives an L, whose silhouette bends inside its bounding box.
    fn app_with_boolean(
        ctx: &egui::Context,
        op: ondin_core::BoolOp,
        offset: (f64, f64),
    ) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0xB0);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let b = ids.mint();
        let (a1, a2) = (ids.mint(), ids.mint());
        let rect = || NodeKind::Rect {
            size: ondin_core::kurbo::Size::new(100.0, 100.0),
            corner_radii: Default::default(),
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: b,
                parent: root,
                index: 0,
                kind: NodeKind::Boolean { op },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: a1,
                parent: b,
                index: 0,
                kind: rect(),
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: a2,
                parent: b,
                index: 1,
                kind: rect(),
                transform: Some(Affine::translate(offset)),
                name: None,
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        (app, b, a1)
    }

    /// Every point `stroke_outlines` painted for `id`, in screen space, with its
    /// bounding box and how many shapes it took to draw it.
    fn traced(
        app: &OndinApp,
        ctx: &egui::Context,
        id: NodeId,
    ) -> (egui::Rect, usize, Vec<egui::Pos2>) {
        let view = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(view),
                ..Default::default()
            },
            |ui| {
                let stroke = egui::Stroke::new(3.0, egui::Color32::RED);
                app.stroke_outlines(
                    ui.painter(),
                    id,
                    view,
                    1.0,
                    stroke,
                    app.outline_view(stroke),
                );
            },
        );
        let mut bbox = egui::Rect::NOTHING;
        let mut n = 0;
        let mut pts = Vec::new();
        for c in out.shapes {
            let egui::Shape::Path(p) = c.shape else {
                continue;
            };
            n += 1;
            for pt in p.points {
                bbox.extend_with(pt);
                pts.push(pt);
            }
        }
        (bbox, n, pts)
    }

    /// A boolean is traced by its silhouette, not its operands or its box: the box catches
    /// operand outlines, the shape count operands left underneath, the bend a `match` arm
    /// tracing the selection quad.
    #[test]
    fn a_booleans_outline_is_its_own_silhouette_and_not_its_operands() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (app, b, a1) = app_with_boolean(&ctx, ondin_core::BoolOp::Subtract, (50.0, 50.0));

        let view = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let path = app
            .session
            .resolved
            .boolean_path(b)
            .expect("the fixture's boolean resolves to a shape")
            .clone();
        let want = ondin_core::kurbo::Shape::bounding_box(&path);
        // The fixture: an L whose box is the first rect's, bitten out at (50, 50).
        assert!(
            (want.x0).abs() < 1e-6
                && (want.y0).abs() < 1e-6
                && (want.x1 - 100.0).abs() < 1e-6
                && (want.y1 - 100.0).abs() < 1e-6,
            "the subtract keeps the first rect's box: {want:?}"
        );
        let corner = |x, y| app.to_screen(Point::new(x, y), view, 1.0);
        let (lo, hi) = (corner(want.x0, want.y0), corner(want.x1, want.y1));
        let bend = corner(50.0, 50.0);

        let (got, shapes, pts) = traced(&app, &ctx, b);
        assert!(
            (got.min.x - lo.x).abs() < 0.5
                && (got.min.y - lo.y).abs() < 0.5
                && (got.max.x - hi.x).abs() < 0.5
                && (got.max.y - hi.y).abs() < 0.5,
            "traced {got:?}, the silhouette is {lo:?}–{hi:?}"
        );
        assert_eq!(shapes, 1, "one outline, not one per operand");
        assert!(
            pts.iter()
                .any(|p| (p.x - bend.x).abs() < 0.5 && (p.y - bend.y).abs() < 0.5),
            "the ring bends where the shape bends — a quad has no point at \
             {bend:?}, and traced {pts:?}"
        );

        let rb = ondin_core::local_box(&app.session.doc, &app.session.resolved, a1).expect("a box");
        let (rlo, rhi) = (corner(rb.x0, rb.y0), corner(rb.x1, rb.y1));
        let (rgot, rshapes, _) = traced(&app, &ctx, a1);
        assert!(
            (rgot.min.x - rlo.x).abs() < 0.5 && (rgot.max.x - rhi.x).abs() < 0.5,
            "the control is exact: traced {rgot:?}, wanted {rlo:?}–{rhi:?}"
        );
        assert_eq!(rshapes, 1);
    }

    /// A boolean with no derived outline is traced by its box, as text is, not by its operands.
    #[test]
    fn a_boolean_with_no_result_is_traced_by_its_box_and_not_by_its_operands() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let (app, b, _) = app_with_boolean(&ctx, ondin_core::BoolOp::Intersect, (500.0, 0.0));
        assert!(
            app.session
                .resolved
                .boolean_path(b)
                .is_none_or(|p| p.is_empty()),
            "two disjoint rects intersect to nothing"
        );

        let (_, shapes, pts) = traced(&app, &ctx, b);
        assert_eq!(
            shapes, 1,
            "one mark for the container, not one per operand: {pts:?}"
        );
        assert_eq!(pts.len(), 4, "and it is the box: {pts:?}");
    }
}

#[cfg(test)]
mod outline_reject_tests {
    //! What the hover ring refuses to trace: what is off screen and what is switched off.
    //! Asserts shape counts rather than timings.

    use super::*;

    /// A group of `n` 100×100 rects in a row, translated by `at`; `hidden` indexes children switched off.
    fn app_with_row(
        ctx: &egui::Context,
        n: usize,
        at: (f64, f64),
        hidden: &[usize],
    ) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0xC0);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let mut ops = vec![Operation::CreateNode {
            id: group,
            parent: root,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::translate(at)),
            name: None,
        }];
        for i in 0..n {
            let child = ids.mint();
            ops.push(Operation::CreateNode {
                id: child,
                parent: group,
                index: i,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::translate((i as f64 * 200.0, 0.0))),
                name: None,
            });
            if hidden.contains(&i) {
                ops.push(Operation::SetVisible {
                    id: child,
                    visible: false,
                });
            }
        }
        doc.apply(&Transaction(ops)).expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        (app, group)
    }

    /// Every path shape `stroke_outlines` paints for `id`, and their bounding box.
    fn traced(app: &OndinApp, ctx: &egui::Context, id: NodeId) -> (usize, egui::Rect) {
        let view = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(view),
                ..Default::default()
            },
            |ui| {
                let stroke = egui::Stroke::new(3.0, egui::Color32::RED);
                app.stroke_outlines(
                    ui.painter(),
                    id,
                    view,
                    1.0,
                    stroke,
                    app.outline_view(stroke),
                );
            },
        );
        let mut bbox = egui::Rect::NOTHING;
        let mut n = 0;
        for c in out.shapes {
            if let egui::Shape::Path(p) = c.shape {
                n += 1;
                for pt in p.points {
                    bbox.extend_with(pt);
                }
            }
        }
        (n, bbox)
    }

    /// An off-screen group paints no ring; the on-screen row is the control.
    /// The `outline_view` padding (a box ending at the view edge) is not covered here.
    #[test]
    fn a_group_off_screen_is_not_traced_and_one_on_screen_still_is() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let (app, group) = app_with_row(&ctx, 3, (0.0, 0.0), &[]);
        let (on_screen, bbox) = traced(&app, &ctx, group);
        assert_eq!(
            on_screen, 3,
            "the control: three rects in view, three marks"
        );
        assert!(
            bbox.min.x < 10.0,
            "and they are where they were put: {bbox:?}"
        );

        let (app, group) = app_with_row(&ctx, 3, (1e6, 1e6), &[]);
        let (off_screen, _) = traced(&app, &ctx, group);
        assert_eq!(
            off_screen, 0,
            "a group a million units away paints no ring and pays for no path"
        );
    }

    /// The ring skips a hidden child, following the layers-panel eye preview via `display_node`.
    #[test]
    fn a_hidden_child_is_not_traced() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let (app, group) = app_with_row(&ctx, 2, (0.0, 0.0), &[1]);
        let (n, bbox) = traced(&app, &ctx, group);
        assert_eq!(n, 1, "one visible rect, one mark");
        assert!(
            bbox.max.x < 150.0,
            "and the ring stops at the visible one: {bbox:?}"
        );

        let (mut app, group) = app_with_row(&ctx, 2, (0.0, 0.0), &[]);
        app.session
            .doc
            .apply(&Transaction(vec![Operation::SetVisible {
                id: group,
                visible: false,
            }]))
            .expect("hide the group");
        assert_eq!(traced(&app, &ctx, group).0, 0, "a hidden group is no ring");
    }
}

#[cfg(test)]
mod rotated_click_away_tests {
    //! A click ending a text session on a rotated node: `text_mode_input` tests the node's
    //! own local rectangle, as the I-beam does, not its upright world bounds.

    use super::*;
    use crate::app::OndinApp;

    /// A `"Hello world"` text node under the root, rotated by `deg` about its own
    /// origin, with a live session open on it.
    fn app_with_rotated_text(ctx: &egui::Context, deg: f64) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(23);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let parts = crate::fonts::FontService::default_text_parts();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "Hello world".into(),
                style: Box::new(parts.style),
                spans: parts.spans,
                para_spans: parts.para_spans,
                paragraph: parts.paragraph,
                block: parts.block,
                sizing: parts.sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::rotate(deg.to_radians())),
            name: None,
        }]))
        .expect("the fixture's text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.begin_edit_text(Some(id), None);
        assert!(app.text.is_some(), "the fixture: a session is live");
        (app, id)
    }

    /// The screen rect and scale every sweep below reads through.
    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };
    const PPP: f32 = 1.0;

    /// The `preview_world_bounds` box and `over_edited_text` disagree under rotation and
    /// agree upright (the control against a predicate that is never true).
    #[test]
    fn the_world_box_and_the_local_box_disagree_only_when_the_node_is_rotated() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let count = |deg: f64| {
            let (app, id) = app_with_rotated_text(&ctx, deg);
            let aabb = app
                .session
                .preview_world_bounds(id)
                .expect("the node has a world box");
            let (mut disagree, mut inside_only, mut ibeam_only) = (0, 0, 0);
            for i in 0..120 {
                for j in 0..120 {
                    let w = Point::new(
                        aabb.x0 + aabb.width() * f64::from(i) / 119.0,
                        aabb.y0 + aabb.height() * f64::from(j) / 119.0,
                    );
                    let p = app.to_screen(w, RECT, PPP);
                    let old = aabb.contains(app.to_world(p, RECT, PPP));
                    let now = app.over_edited_text(p, RECT, PPP);
                    if old != now {
                        disagree += 1;
                        if old {
                            inside_only += 1;
                        } else {
                            ibeam_only += 1;
                        }
                    }
                }
            }
            (disagree, inside_only, ibeam_only)
        };

        let (upright, _, _) = count(0.0);
        assert_eq!(
            upright, 0,
            "upright, the two boxes are the same box and nothing may disagree"
        );

        let (rotated, click_only, ibeam_only) = count(45.0);
        assert!(
            rotated > 1000,
            "at 45° the world box strictly contains the local one, so a large \
             minority of the sweep must disagree: {rotated} of 14400"
        );
        assert_eq!(
            ibeam_only, 0,
            "and every disagreement is the same way round — the click said inside \
             where the cursor said outside, never the reverse"
        );
        assert_eq!(click_only, rotated, "which is the same statement, counted");
    }

    /// A click in the corner between the two boxes ends the session, driven through
    /// `canvas_ui`; a click in the middle of the letters is the control.
    #[test]
    fn a_click_between_the_two_boxes_ends_the_session() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let (mut app, id) = app_with_rotated_text(&ctx, 45.0);
        frame(&ctx, &mut app, Vec::new());
        let rect = app.canvas_rect;
        assert!(rect.width() > 1.0, "the fixture: the canvas was laid out");

        let aabb = app
            .session
            .preview_world_bounds(id)
            .expect("the node has a world box");
        app.session.camera.center = aabb.center();

        let corner = Point::new(
            aabb.x0 + aabb.width() * 0.02,
            aabb.y0 + aabb.height() * 0.02,
        );
        assert!(aabb.contains(corner), "the fixture: inside the upright box");
        let at = app.to_screen(corner, rect, PPP);
        assert!(
            rect.contains(at),
            "the fixture: and the point is on the canvas, or no click is delivered \
             at all and the control below cannot say so"
        );
        assert!(
            !app.over_edited_text(at, rect, PPP),
            "the fixture: and outside the letters, so the pointer is not an I-beam"
        );

        let middle = app.to_screen(aabb.center(), rect, PPP);
        click(&ctx, &mut app, middle);
        assert!(
            app.text.is_some(),
            "the control: a click inside the letters keeps the session"
        );

        click(&ctx, &mut app, at);
        assert!(
            app.text.is_none(),
            "a click outside the letters ends the session"
        );
    }

    /// One frame of the whole canvas, with `events` delivered to it.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                events,
                ..Default::default()
            },
            |ui| {
                app.canvas_ui(ui);
            },
        );
    }

    /// Press and release the primary button at `at`, across the separate frames egui needs.
    fn click(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) {
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
        frame(ctx, app, vec![button(true)]);
        frame(ctx, app, vec![button(false)]);
        frame(ctx, app, Vec::new());
    }
}

#[cfg(test)]
mod tool_switch_drops_the_drag_tests {
    //! Changing tool mid-drag abandons the gesture.

    use super::*;
    use crate::app::OndinApp;
    use crate::canvas::headless_app_tests::app_with_two_rects;

    /// A live crop-resize with a gesture preview behind it, built by hand to keep crop mechanics out.
    fn mid_crop_drag(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let (mut app, a, _b) = app_with_two_rects(ctx);
        let frame = KRect::new(0.0, 0.0, 100.0, 100.0);
        app.drag = Drag::CropResize {
            id: a,
            fill: 0,
            handle: Handle::BottomRight,
            frame,
            start: frame,
            limit: frame,
            grab_world: Point::new(100.0, 100.0),
        };
        app.session
            .set_preview(&Transaction(vec![Operation::SetTransform {
                id: a,
                transform: Affine::translate((5.0, 0.0)),
            }]));
        (app, a)
    }

    /// A tool switch mid-drag leaves neither a live `Drag`, its preview, nor an Alt-clone.
    /// The Escape ladder's ordering (`cancel_gesture` before `choose_tool`) is asserted nowhere.
    #[test]
    fn changing_tool_mid_drag_drops_the_drag_and_its_preview() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);

        let (mut app, _a) = mid_crop_drag(&ctx);
        assert!(
            !app.drag.is_none() && app.session.has_gesture_preview(),
            "the fixture is a drag in flight with a preview up, or this test is \
             about nothing"
        );

        app.choose_tool(Tool::Select);
        assert!(
            app.drag.is_none(),
            "a tool switch must not leave a drag armed — the release then arrives \
             for a gesture the user has left"
        );
        assert!(
            !app.session.has_gesture_preview(),
            "and it must not leave the last frame's preview on the canvas, under \
             chrome that has already come back"
        );
        assert!(
            app.alt_clone.is_none(),
            "nor the copy an Alt-drag was carrying: finish_drag clears that on its \
             no-pointer arm and at the end of Drag::Move, and a Drag::None with a \
             pointer position reaches neither, so a stale one is what the next \
             Alt-drag silently inserts"
        );
    }
}

#[cfg(test)]
mod autopan_ownership_tests {
    //! A gesture's auto-scroll (`autopan_total`, which `cancel_gesture` subtracts) is
    //! cleared at both ends of a drag under every tool.

    use super::*;
    use crate::app::OndinApp;
    use crate::canvas::headless_app_tests::app_with_two_rects;

    const AREA: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };
    /// A leftover no rounding can produce.
    const LEFTOVER: Vec2 = Vec2::new(300.0, 0.0);

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// One frame through `normal_mode_input`, on a response over the whole area.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(AREA),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let resp = ui.interact(
                AREA,
                egui::Id::new("autopan-ownership-probe"),
                egui::Sense::click_and_drag(),
            );
            app.normal_mode_input(ui, &resp, AREA, 1.0);
        });
    }

    /// Every tool clears `autopan_total` when a drag starts and again when it ends;
    /// `Select` and `Rect` are the controls. Both edges are asserted so a press-only fix fails.
    #[test]
    fn every_tool_clears_the_autopan_total_at_both_ends_of_a_drag() {
        let ctx = egui::Context::default();
        for tool in [
            Tool::Select,
            Tool::Rect,
            Tool::Node,
            Tool::Pen,
            Tool::ImageEdit,
        ] {
            let (mut app, _, _) = app_with_two_rects(&ctx);
            app.canvas_px = (800, 600);
            app.tool = tool;

            let middle = egui::pos2(400.0, 300.0);
            let over = egui::pos2(460.0, 300.0);
            // egui assigns a press to a widget only after a warm-up frame.
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(middle)]);
            frame(
                &ctx,
                &mut app,
                vec![egui::Event::PointerMoved(middle), button(middle, true)],
            );

            app.autopan_total = LEFTOVER;
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(over)]);
            assert_eq!(
                app.autopan_total,
                Vec2::ZERO,
                "{tool:?} clears on the press"
            );

            app.autopan_total = LEFTOVER;
            frame(&ctx, &mut app, vec![button(over, false)]);
            assert_eq!(
                app.autopan_total,
                Vec2::ZERO,
                "{tool:?} clears on the release — the half the reported failure is \
                 about, since nothing presses on the canvas between the drag that \
                 scrolled and the cancel that spends it"
            );
        }
    }
}

#[cfg(test)]
mod session_rebase_tests {
    //! One editing session is one undo step, even when something outside it writes to the
    //! node: `TextSession::original` is re-based with the editor.

    use super::*;
    use crate::app::OndinApp;

    /// A headless app holding one text node reading `"hello"`, selected.
    fn app_with_text(ctx: &egui::Context) -> (OndinApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x7E5);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let parts = crate::fonts::FontService::default_text_parts();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "hello".into(),
                style: Box::new(parts.style),
                spans: parts.spans,
                para_spans: parts.para_spans,
                paragraph: parts.paragraph,
                block: parts.block,
                sizing: parts.sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("the fixture's text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        (app, id)
    }

    /// The Type panel's character write as `char_attrs_tx` builds it, committed and handed
    /// to `text_session_restyled_after`.
    fn panel_sets_a_character_size(app: &mut OndinApp, id: NodeId, range: std::ops::Range<usize>) {
        let style = match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Text { style, .. }) => (**style).clone(),
            _ => panic!("the fixture is a text node"),
        };
        let mut spans = CharSpans::default();
        // 40, not the default 24: `Spans::set` stores nothing for a default value and the commit is absorbed.
        spans.set(range, ondin_core::CharAttr::Size(40.0), &style);
        let tx = Transaction(vec![
            Operation::SetTextStyle {
                id,
                style,
                spans: None,
            },
            Operation::SetTextSpans { id, spans },
        ]);
        assert!(
            app.session.commit(tx.clone()),
            "the fixture: the panel's own commit must land"
        );
        app.text_session_restyled_after(&tx);
    }

    /// A styling click under a live session is one undo step, not two; the undo-depth
    /// assertion is a control (the `changes_nothing` guard refuses the duplicate commit).
    /// flip: remove `s.original = stored;` → fails at "the session told the user it had updated something it had not".
    #[test]
    fn a_styling_click_under_a_live_session_is_one_undo_step() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_text(&ctx);
        app.begin_edit_text(Some(id), None);
        let opened = app
            .text
            .as_ref()
            .expect("the session is live")
            .original
            .clone();
        assert!(
            !app.text.as_ref().unwrap().changed(),
            "the control: a session that has just opened has changed nothing"
        );

        let depth0 = app.session.history.undo_depth();
        panel_sets_a_character_size(&mut app, id, 0..2);
        let depth1 = app.session.history.undo_depth();
        assert_eq!(
            depth1,
            depth0 + 1,
            "the fixture: the panel's write must be an undo step of its own"
        );
        let stored_spans = match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Text { spans, .. }) => spans.clone(),
            _ => panic!("the fixture is a text node"),
        };
        assert_ne!(
            stored_spans, opened.1,
            "the fixture: the panel must have written spans the session did not \
             open with, or `changed()` answers no for the wrong reason"
        );

        let changed = app.text.as_ref().unwrap().changed();
        app.finish_text_edit();

        assert_ne!(
            app.session.status().text,
            "Text updated",
            "the session told the user it had updated something it had not"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth1,
            "the session committed a second step over the panel's"
        );
        assert!(!changed, "the session reported a change it did not make");
    }

    /// Typed text still commits after an outside restyle: re-basing `original` onto the
    /// editor would make `changed()` answer no and Escape would throw the text away.
    /// flip: re-base `original` onto the editor's fields → fails at "the typed text was thrown away".
    #[test]
    fn typed_text_survives_an_outside_restyle() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_text(&ctx);
        app.begin_edit_text(Some(id), None);
        app.text
            .as_mut()
            .expect("the session is live")
            .editor
            .insert("goodbye");
        assert_eq!(
            app.text.as_ref().unwrap().editor.content(),
            "goodbye",
            "the fixture: the session holds uncommitted content"
        );

        panel_sets_a_character_size(&mut app, id, 0..2);
        let changed = app.text.as_ref().unwrap().changed();
        app.finish_text_edit();

        let content = match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Text { content, .. }) => content.clone(),
            _ => panic!("the text node survived"),
        };
        assert_eq!(content, "goodbye", "the typed text was thrown away");
        assert!(
            changed,
            "the session forgot it was holding uncommitted text"
        );
    }
}

#[cfg(test)]
mod session_preview_shape_tests {
    //! A keystroke shapes the edited node once; the preview reuses the editor's layout.
    //! Counted with `ondin_core::text::shapes()`.

    use super::*;
    use crate::app::OndinApp;

    /// A headless app with one text node open for editing, caret at the end.
    fn app_editing_text(ctx: &egui::Context) -> (OndinApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0xA4E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let parts = crate::fonts::FontService::default_text_parts();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "hello world".into(),
                style: Box::new(parts.style),
                spans: parts.spans,
                para_spans: parts.para_spans,
                paragraph: parts.paragraph,
                block: parts.block,
                sizing: parts.sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("the fixture's text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.begin_edit_text(Some(id), None);
        let editor = &mut app.text.as_mut().expect("a live session").editor;
        editor.move_line_end(false);
        (app, id)
    }

    /// Typing one character shapes once and the preview shapes nothing; the editor's count
    /// and the preview's existence guard against a vacuous zero.
    /// flip: `preview_session` passes `None` for the layout → fails at "the preview re-shaped what the editor had just shaped".
    #[test]
    fn a_keystroke_shapes_the_node_once() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_editing_text(&ctx);

        let at = ondin_core::text::shapes();
        app.text
            .as_mut()
            .expect("a live session")
            .editor
            .insert("x");
        let by_the_editor = ondin_core::text::shapes() - at;
        assert_eq!(
            by_the_editor, 1,
            "the fixture: the keystroke itself must shape exactly once"
        );

        let at = ondin_core::text::shapes();
        app.preview_session();
        assert_eq!(
            ondin_core::text::shapes() - at,
            0,
            "the preview re-shaped what the editor had just shaped"
        );

        assert!(
            app.session.has_gesture_preview() || app.session.preview_text_layout(id).is_some(),
            "the fixture: a preview must actually have been built, or `0` is the \
             count of a build that did not happen"
        );
        let laid = app
            .session
            .preview_text_layout(id)
            .expect("the preview carries the session's layout");
        assert!(
            laid.size.width > 0.0,
            "and it is a real layout rather than an empty stand-in"
        );
    }

    /// Control: a caller with no layout in hand still gets a shaped preview.
    #[test]
    fn a_preview_with_nothing_prepared_still_shapes() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_editing_text(&ctx);
        let tx = Transaction(vec![Operation::SetText {
            id,
            content: "hello world and then some".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]);

        let at = ondin_core::text::shapes();
        app.session.set_session_preview(&tx, None);
        assert_eq!(
            ondin_core::text::shapes() - at,
            1,
            "the control: with nothing prepared the preview must shape it itself"
        );
    }
}

#[cfg(test)]
mod point_box_snap_tests {
    //! A point box's side handles snap the axis they travel along.

    use super::*;
    use crate::app::OndinApp;

    /// A side handle draws no guide for the axis it cannot move; asserts guides, not position.
    /// flip: `snapped_point_box` passes `(true, true)` → fails at the `Right` case (two guides).
    #[test]
    fn a_side_handle_snaps_one_axis_and_a_corner_snaps_both() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);

        let mut ids = ondin_core::IdSource::new(0x5A1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(100.0, 100.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        // Not a ruler guide: `snap_point` emits no `Guide` mark for those.
        let target = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: target,
            parent: root,
            index: 1,
            kind: NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(100.0, 100.0),
                corner_radii: Default::default(),
            },
            transform: Some(ondin_core::kurbo::Affine::translate((300.0, 300.0))),
            name: None,
        }]))
        .expect("something to snap to");
        app.session.adopt_document(doc, None);
        app.canvas_px = (1200, 1200);
        app.session.camera.center = Point::new(300.0, 300.0);

        let grab = Point::new(100.0, 100.0);
        let world = Point::new(300.4, 300.4);
        let axes_of = |handle| {
            let (_, guides) = app.snapped_point_box(id, handle, grab, world);
            let mut on: Vec<crate::measure::Axis> = guides.iter().map(|g| g.axis).collect();
            on.sort_by_key(|a| format!("{a:?}"));
            on.dedup();
            on
        };

        assert_eq!(
            axes_of(Handle::Right),
            vec![crate::measure::Axis::Vertical],
            "a right handle travels along x only, so only the vertical guide is its"
        );
        assert_eq!(
            axes_of(Handle::Bottom),
            vec![crate::measure::Axis::Horizontal],
            "and a bottom handle along y only"
        );
        assert_eq!(
            axes_of(Handle::BottomRight).len(),
            2,
            "a corner still snaps both"
        );
    }
}

#[cfg(test)]
mod pivot_snap_scale_tests {
    //! The nine-point pivot snap is a screen distance: `pivot_at` converts its threshold
    //! with the zoom and the node's own basis (scale or skew).

    use super::*;
    use ondin_core::kurbo::{Affine, Size};

    /// A headless app holding one 100 × 100 rect whose own transform is `basis`, at zoom 1.
    fn app_with_basis(ctx: &egui::Context, basis: Affine) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x9107);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
            },
            transform: Some(basis),
            name: None,
        }]))
        .expect("a rect");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.session.camera.zoom = 1.0;
        app.canvas_px = (1200, 1200);
        (app, id)
    }

    /// The x fraction `pivot_at` would commit for a release at `world`.
    fn x_frac(app: &OndinApp, id: NodeId, world: Point) -> f64 {
        match app.pivot_at(id, world) {
            Some(Pivot::Normalized(v)) => v.x,
            other => panic!("expected a normalized pivot, got {other:?}"),
        }
    }

    /// The y fraction, likewise.
    fn y_frac(app: &OndinApp, id: NodeId, world: Point) -> f64 {
        match app.pivot_at(id, world) {
            Some(Pivot::Normalized(v)) => v.y,
            other => panic!("expected a normalized pivot, got {other:?}"),
        }
    }

    /// A 4× basis does not widen the corner snap beyond `PIVOT_SNAP_PX` screen units;
    /// the first assertion shows the snap still fires.
    /// flip: `tol = PIVOT_SNAP_PX / zoom` → fails at the second assertion.
    #[test]
    fn a_scaled_basis_does_not_widen_the_pivot_snap() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (app, id) = app_with_basis(&ctx, Affine::scale(4.0));

        assert_eq!(
            x_frac(&app, id, Point::new(5.0, 0.0)),
            0.0,
            "five screen units from the corner is still a corner grab"
        );
        assert_ne!(
            x_frac(&app, id, Point::new(10.0, 0.0)),
            0.0,
            "ten screen units is not — under the scale-blind threshold this \
             snapped, and the marker could not be placed there at all"
        );
    }

    /// Under `scale(4, 1)` the axes get different local tolerances (1.5 and 6); a single
    /// scalar such as the geometric mean passes the y half and fails the x half.
    #[test]
    fn a_non_uniform_basis_gives_each_axis_its_own_snap_radius() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (app, id) = app_with_basis(&ctx, Affine::new([4.0, 0.0, 0.0, 1.0, 0.0, 0.0]));

        assert_ne!(
            x_frac(&app, id, Point::new(10.0, 0.0)),
            0.0,
            "x is scaled 4×, so ten world units is well outside its 6px reach"
        );
        assert_eq!(
            y_frac(&app, id, Point::new(0.0, 5.0)),
            0.0,
            "y is scaled 1×, so five world units is inside its 6px reach — the \
             same drag, the same numbers, two different answers"
        );
    }
}

#[cfg(test)]
mod zero_travel_release_tests {
    //! A gesture that ends where it began is no undo step, in every arm of `finish_drag`.
    //! Three mechanisms guard the arms: `Move` has its own epsilon, `Skew` and `Rotate` emit
    //! nothing for a zero drag, `Resize`, `LineEnd` and `RailOffset` fall to `commit_inner`'s
    //! `changes_nothing`. The outcome is asserted, not the route.

    use super::*;
    use ondin_core::kurbo::Size;

    /// Builds one row's app and names the node the gesture acts on.
    type Fixture<'a> = Box<dyn Fn() -> (OndinApp, NodeId) + 'a>;
    /// Builds one row's gesture, given that node.
    type Gesture = Box<dyn Fn(NodeId) -> Drag>;
    /// `(arm name, fixture, gesture, the world point it is released at)`.
    type Case<'a> = (&'static str, Fixture<'a>, Gesture, Point);

    /// A headless app holding one 100 × 60 rect at the origin, selected.
    fn app_with_rect(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x2E80);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 60.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.canvas_px = (1200, 1200);
        app.session.camera.zoom = 1.0;
        (app, id)
    }

    /// A selected 400 × 200 frame at the origin laid out as a row (padding 20, gap 10) of
    /// two 40 × 30 rects: the gap band is at x 60–70.
    fn app_with_a_laid_frame(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x2E83);
        let root = ids.mint();
        let row = ids.mint();
        let mut ops = vec![Operation::CreateNode {
            id: row,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: None,
            name: None,
        }];
        for i in 0..2 {
            ops.push(Operation::CreateNode {
                id: ids.mint(),
                parent: row,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 30.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
        }
        ops.push(Operation::SetDisplay {
            id: row,
            display: Some(ondin_core::container::Display::Flex(
                ondin_core::container::Flex {
                    column_gap: 10.0,
                    padding: [20.0; 4],
                    ..Default::default()
                },
            )),
        });
        let mut doc = Document::new(root);
        doc.apply(&Transaction(ops)).expect("a laid frame");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(row);
        app.canvas_px = (1200, 1200);
        app.session.camera.zoom = 1.0;
        (app, row)
    }

    /// A headless app holding one line from (0, 0) to (100, 0), selected.
    fn app_with_line(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x2E81);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Line {
                end: Point::new(100.0, 0.0),
            },
            transform: None,
            name: None,
        }]))
        .expect("a line");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.canvas_px = (1200, 1200);
        app.session.camera.zoom = 1.0;
        (app, id)
    }

    /// A headless app holding one text node railed to a 200-unit ring, selected.
    fn app_on_a_ring(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(0x2E82);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let ring = ondin_core::geometry::local_path(&NodeKind::Ellipse {
            size: Size::new(200.0, 200.0),
        })
        .expect("an ellipse has an outline");
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "AROUND".into(),
                style: Box::new(ondin_core::TextStyle {
                    font_family: "Inter".into(),
                    font_size: 16.0,
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: ondin_core::TextSizing::Auto,
                on_path: Some(Box::new(ring)),
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a railed text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.canvas_px = (1200, 1200);
        app.session.camera.zoom = 1.0;
        (app, id)
    }

    /// Every committing arm of `finish_drag`, released where pressed, leaves the undo depth alone.
    /// flip: disable the `changes_nothing` guard in `session::commit_inner` → fails with
    /// `["Resize", "LineEnd", "RailOffset"]`.
    #[test]
    fn no_committing_drag_arm_spends_an_undo_step_on_a_zero_travel_release() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let cases: Vec<Case<'_>> = vec![
            (
                "Move",
                Box::new(|| app_with_rect(&ctx)) as Fixture<'_>,
                Box::new(|_id: NodeId| Drag::Move {
                    anchor: Point::new(50.0, 30.0),
                }) as Gesture,
                Point::new(50.0, 30.0),
            ),
            (
                "Resize",
                Box::new(|| app_with_rect(&ctx)),
                Box::new(|id: NodeId| Drag::Resize {
                    target: Target::Layer(id),
                    handle: Handle::BottomRight,
                }),
                Point::new(100.0, 60.0),
            ),
            (
                "Rotate",
                Box::new(|| app_with_rect(&ctx)),
                Box::new(|id: NodeId| Drag::Rotate {
                    target: Target::Layer(id),
                    pivot: Point::new(50.0, 30.0),
                    start: 0.0,
                    turn: Affine::IDENTITY,
                }),
                Point::new(150.0, 30.0),
            ),
            (
                "Skew",
                Box::new(|| app_with_rect(&ctx)),
                Box::new(|id: NodeId| Drag::Skew {
                    target: Target::Layer(id),
                    handle: Handle::Right,
                    press: Point::new(100.0, 30.0),
                    lean: Affine::IDENTITY,
                }),
                Point::new(100.0, 30.0),
            ),
            (
                "LineEnd",
                Box::new(|| app_with_line(&ctx)),
                Box::new(|id: NodeId| Drag::LineEnd {
                    id,
                    which: LineEnd::End,
                }),
                Point::new(100.0, 0.0),
            ),
            (
                "RailOffset",
                Box::new(|| app_on_a_ring(&ctx)),
                Box::new(|id: NodeId| Drag::RailOffset { id }),
                // `geometry::local_path` opens an ellipse at `MoveTo((200, 100))`.
                Point::new(200.0, 100.0),
            ),
            (
                "Spacing",
                Box::new(|| app_with_a_laid_frame(&ctx)),
                Box::new(|id: NodeId| Drag::Spacing {
                    id,
                    band: crate::spacing::BandKind::ColumnGap,
                    anchor: Point::new(65.0, 35.0),
                    base: 10.0,
                }),
                Point::new(65.0, 35.0),
            ),
        ];

        let mut spent: Vec<&str> = Vec::new();
        for (name, build, gesture, release) in cases {
            let (mut app, id) = build();
            let before = app.session.history.undo_depth();
            let kind_before = app.session.doc.get(id).map(|n| n.kind().clone());
            let tf_before = app.session.doc.get(id).map(|n| n.transform());

            app.drag = gesture(id);
            app.finish_drag(Some(release), false, false);

            assert_eq!(
                app.session.doc.get(id).map(|n| n.kind().clone()),
                kind_before,
                "{name}: the fixture must reach the state — a zero-travel release \
                 has to leave the node alone, or this row is measuring a real edit"
            );
            assert_eq!(
                app.session.doc.get(id).map(|n| n.transform()),
                tf_before,
                "{name}: and its transform too"
            );

            if app.session.history.undo_depth() != before {
                spent.push(name);
            }
        }
        assert!(
            spent.is_empty(),
            "these arms spent an undo step on a gesture that ended where it began: \
             {spent:?}"
        );
    }
}

#[cfg(test)]
mod pick_policy_tests {
    //! The click-to-select policy, as a table.
    use super::*;

    fn n(i: u64) -> NodeId {
        NodeId::from_wire(&format!("1:{i}")).expect("a well-formed id")
    }

    /// All five arms of the click policy; `Ctrl` alone is asserted against a deep chain
    /// because an empty one answers the leaf everywhere.
    /// flip: widen `ctrl && !alt` to `ctrl` → fails at the `Ctrl+Alt` two-group assertion.
    #[test]
    fn the_five_arms_of_the_click_policy() {
        let leaf = n(9);
        let (outer, inner) = (n(1), n(2));

        for (ctrl, alt) in [(false, false), (true, false), (true, true), (false, true)] {
            assert_eq!(
                pick_from_chain(ctrl, alt, &[], leaf),
                leaf,
                "ctrl={ctrl} alt={alt}: with no group above it the leaf is the \
                 whole story"
            );
        }

        assert_eq!(
            pick_from_chain(true, false, &[outer, inner], leaf),
            leaf,
            "Ctrl reaches past every group to the layer itself"
        );

        assert_eq!(
            pick_from_chain(true, true, &[outer, inner], leaf),
            inner,
            "Ctrl+Alt steps one level in, not all the way — that is how a deep \
             tree is walked without losing your place"
        );
        assert_eq!(
            pick_from_chain(true, true, &[outer], leaf),
            leaf,
            "and with a single group one level in is the leaf"
        );

        assert_eq!(
            pick_from_chain(false, false, &[outer, inner], leaf),
            outer,
            "a plain click on something in a group selects the group"
        );
        assert_eq!(
            pick_from_chain(false, true, &[outer, inner], leaf),
            outer,
            "and Alt alone is not a pick modifier — it belongs to the drag"
        );
    }

    /// Root, two sibling groups, one rect in each; returns the app, groups and rects.
    fn two_groups(ctx: &egui::Context) -> (OndinApp, [NodeId; 2], [NodeId; 2]) {
        use ondin_core::{Document, IdSource, Operation, Transaction};
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x91C);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let groups = [ids.mint(), ids.mint()];
        let leaves = [ids.mint(), ids.mint()];
        let mut ops = Vec::new();
        for (i, g) in groups.iter().enumerate() {
            ops.push(Operation::CreateNode {
                id: *g,
                parent: root,
                index: i,
                kind: ondin_core::NodeKind::Group,
                transform: None,
                name: None,
            });
            ops.push(Operation::CreateNode {
                id: leaves[i],
                parent: *g,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).expect("two groups");
        app.session.adopt_document(doc, None);
        (app, groups, leaves)
    }

    /// One frame, delivering `modifiers`, returning what the click picked.
    fn pick(app: &mut OndinApp, ctx: &egui::Context, leaf: NodeId, ctrl: bool) -> NodeId {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            modifiers: egui::Modifiers {
                ctrl,
                command: ctrl,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut picked = leaf;
        let _ = ctx.run_ui(input, |ui| picked = app.pick_for_click(leaf, ui));
        picked
    }

    /// Clicking outside the entered group ends the isolation and picks by the normal
    /// rule in the same click; the click that stays inside is the control.
    /// flip: delete the `entered_group = None` guard → fails at the scope assertion.
    #[test]
    fn a_click_outside_the_entered_group_leaves_it() {
        let ctx = egui::Context::default();
        let (mut app, groups, leaves) = two_groups(&ctx);
        app.entered_group = Some(groups[0]);

        let picked = pick(&mut app, &ctx, leaves[0], false);
        assert_eq!(
            app.entered_group,
            Some(groups[0]),
            "a click inside the entered group stays inside it"
        );
        assert_eq!(picked, leaves[0], "and picks the layer, not the group");

        let picked = pick(&mut app, &ctx, leaves[1], false);
        assert_eq!(app.entered_group, None, "the click left the group");
        assert_eq!(
            picked, groups[1],
            "and it selected the other group, not the layer inside it — the \
             guard runs before the chain is built so the same click does both"
        );
    }

    /// `Ctrl` reaching into a group also enters it, as a double-click does; the plain
    /// click is the control.
    /// flip: `pick_for_click`'s enter condition replaced with `false` → fails at "and Ctrl enters the group it reached into".
    #[test]
    fn a_ctrl_click_into_a_group_enters_it_and_a_plain_one_does_not() {
        let ctx = egui::Context::default();
        let (mut app, groups, leaves) = two_groups(&ctx);

        let picked = pick(&mut app, &ctx, leaves[0], false);
        assert_eq!(picked, groups[0], "control: a plain click takes the group");
        assert_eq!(app.entered_group, None, "and enters nothing");

        let picked = pick(&mut app, &ctx, leaves[0], true);
        assert_eq!(picked, leaves[0], "Ctrl reaches the layer inside");
        assert_eq!(
            app.entered_group,
            Some(groups[0]),
            "and Ctrl enters the group it reached into"
        );
    }
}

#[cfg(test)]
mod marquee_selectability_tests {
    //! What a rubber band may pick up. The halves of `selectable` differ on purpose: the
    //! lock is effective (a layer in a locked group is refused, as a click is), visibility
    //! is the node's own flag (`context-menus.md` §5.9).

    use super::*;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, Operation, Transaction};

    /// A 10×10 rect under `parent`, at `index`, offset so the three do not overlap.
    fn rect(doc: &mut Document, ids: &mut IdSource, parent: NodeId, index: usize) -> NodeId {
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((20.0 * index as f64, 0.0))),
            name: None,
        }]))
        .expect("a rect");
        id
    }

    /// A plain, a locked and a hidden rect under one artboard, in a headless app.
    fn fixture(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x5A11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let board = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: board,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(200.0, 200.0),
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        }]))
        .expect("an artboard");

        let plain = rect(&mut doc, &mut ids, board, 0);
        let locked = rect(&mut doc, &mut ids, board, 1);
        let hidden = rect(&mut doc, &mut ids, board, 2);
        doc.apply(&Transaction(vec![
            Operation::SetLocked {
                id: locked,
                locked: true,
            },
            Operation::SetVisible {
                id: hidden,
                visible: false,
            },
        ]))
        .expect("lock one and hide one");
        app.session.adopt_document(doc, None);
        (app, plain, locked, hidden)
    }

    /// A band over all three picks the plain layer only. `may_select_in_bulk` is the lock
    /// rule shared with `select_all`; the visibility term is `apply_marquee`'s alone.
    /// flip: `may_select_in_bulk` returns `true` → fails at the set equality.
    #[test]
    fn a_marquee_takes_neither_a_locked_layer_nor_a_hidden_one() {
        let ctx = egui::Context::default();
        let (mut app, plain, locked, hidden) = fixture(&ctx);

        assert!(
            ondin_core::query::is_effectively_locked(&app.session.doc, locked),
            "the fixture's locked layer really is locked"
        );
        assert!(
            !app.session.doc.get(hidden).expect("still there").visible(),
            "and its hidden layer really is hidden"
        );

        app.apply_marquee(Point::new(-50.0, -50.0), Point::new(200.0, 200.0), false);
        let got = app.session.selection.ids().to_vec();

        assert_eq!(got, vec![plain], "only the plain layer is picked up");
        assert!(
            !got.contains(&locked),
            "a locked layer is refused by the band exactly as it is by a click \
             (§15 D321) — the filter is `marquee_candidates`' since D746"
        );
        assert!(
            !got.contains(&hidden),
            "and a hidden one is refused by `apply_marquee`'s own visibility \
             term, which is the node's flag and stays here"
        );
    }
}

#[cfg(test)]
mod frame_edge_tests {
    //! An occupied frame is picked on its edge from every door (`context-menus.md` §2, C5).

    use super::*;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, Operation, Transaction};

    /// A 200×200 frame at the origin holding one 10×10 rect, so it is occupied.
    fn fixture(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xED9E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let board = ids.mint();
        let inside = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: board,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(200.0, 200.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::CreateNode {
                id: inside,
                parent: board,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::translate((90.0, 90.0))),
                name: None,
            },
        ]))
        .expect("a frame with something in it");
        app.session.adopt_document(doc, None);
        (app, board, inside)
    }

    /// An occupied frame's border answers, its middle does not (`pick_leaf` skips it so a
    /// marquee can cross its contents), and its contents win their own pixels.
    /// flip: drop the `!…inflate(-slop)` term → fails at "the middle of a frame is not the frame".
    /// Dropping the `is_occupied_frame` term does not bite: this fixture's frame is occupied.
    #[test]
    fn a_frames_edge_is_pickable_and_its_middle_is_not() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (app, board, inside) = fixture(&ctx);
        assert!(
            app.is_occupied_frame(board),
            "the fixture must reach the state: an *empty* frame is a different rule"
        );
        let slop = app.pick_slop();
        assert!(
            slop > 0.0 && slop < 50.0,
            "the fixture must reach the state: a usable band at this zoom, got {slop}"
        );
        // Wider than a line's pick: six points either side is still the edge, ten in is not.
        let pt = 1.0 / app.points_per_world();
        for (at, what) in [
            (
                Point::new(100.0, 6.0 * pt),
                "six points inside the top border",
            ),
            (Point::new(100.0, -6.0 * pt), "six points outside it"),
        ] {
            assert_eq!(app.frame_edge_at(at), Some(board), "{what} is the edge");
        }
        assert_eq!(
            app.frame_edge_at(Point::new(100.0, 10.0 * pt)),
            None,
            "ten points inside is the interior"
        );

        for (at, what) in [
            (Point::new(100.0, slop / 2.0), "just inside the top border"),
            (Point::new(100.0, -slop / 2.0), "just outside it"),
            (Point::new(slop / 2.0, 100.0), "the left border"),
            (Point::new(200.0 - slop / 2.0, 100.0), "the right border"),
        ] {
            assert_eq!(
                app.frame_edge_at(at),
                Some(board),
                "{what} is the frame's edge"
            );
        }

        assert_eq!(
            app.frame_edge_at(Point::new(100.0, 100.0)),
            None,
            "the middle of a frame is not the frame — which is what `pick_leaf` \
             skipping an occupied frame exists for, and what widening the chain \
             with `selected_frame_at` would have broken"
        );
        assert_eq!(
            app.frame_edge_at(Point::new(-50.0, -50.0)),
            None,
            "and neither is empty canvas well outside it"
        );

        assert_eq!(
            app.pick_leaf(Point::new(95.0, 95.0)),
            Some(inside),
            "control: the rect inside the frame is picked as itself"
        );
    }

    /// The frame edge is reached through the `pick_at_pointer` chain, not just `frame_edge_at`;
    /// the point comes from `to_screen` so `frame_label_at` cannot answer first.
    /// flip: delete the `.or_else(|| self.frame_edge_at(world))` link → fails at "the border is reached through the chain".
    #[test]
    fn a_frames_edge_is_reached_through_the_shipped_chain() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (app, board, inside) = fixture(&ctx);
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let slop = app.pick_slop();

        let _ = ctx.run_ui(Default::default(), |ui| {
            let edge = Point::new(100.0, slop / 2.0);
            assert_eq!(
                app.pick_at_pointer(ui, app.to_screen(edge, rect, 1.0), edge, rect, 1.0),
                Some(board),
                "the border is reached through the chain, not just through the \
                 helper"
            );
            let middle = Point::new(150.0, 150.0);
            assert_eq!(
                app.pick_at_pointer(ui, app.to_screen(middle, rect, 1.0), middle, rect, 1.0),
                None,
                "control: the interior is still not the frame, so the link did \
                 not widen into `selected_frame_at`'s answer"
            );
            let over = Point::new(95.0, 95.0);
            assert_eq!(
                app.pick_at_pointer(ui, app.to_screen(over, rect, 1.0), over, rect, 1.0),
                Some(inside),
                "control: contents still win their own pixels"
            );
        });
    }

    /// A locked or hidden frame is not selectable by its border (`Selection::set_one`
    /// filters nothing); each case first asserts the border answers.
    /// flip: delete the lock term, keep `shown_visible` → fails on the locked case only.
    #[test]
    fn a_locked_or_hidden_frames_edge_is_not_pickable() {
        for (what, op) in [
            ("locked", |id| Operation::SetLocked { id, locked: true }),
            ("hidden", |id| Operation::SetVisible { id, visible: false }),
        ] as [(&str, fn(NodeId) -> Operation); 2]
        {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let (mut app, board, _inside) = fixture(&ctx);
            let slop = app.pick_slop();
            let edge = Point::new(100.0, slop / 2.0);
            assert_eq!(
                app.frame_edge_at(edge),
                Some(board),
                "control ({what}): the fixture must reach the band before the \
                 flag is set, or this test passes for the wrong reason"
            );
            app.session
                .commit(Transaction(vec![op(board)]))
                .then_some(())
                .expect("the flag is settable");
            assert_eq!(
                app.frame_edge_at(edge),
                None,
                "a {what} frame is not selectable by its border"
            );
        }
    }

    /// A press on the border grabs the frame as a click selects it, and keeps
    /// `entered_group`; the frame starts unselected, or `selected_frame_at` would answer.
    /// flip: delete the fourth link of `begin_select_drag` → fails at the drag-arm assertion.
    #[test]
    fn a_press_on_a_frames_edge_grabs_it_rather_than_starting_a_marquee() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, board, _inside) = fixture(&ctx);
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let slop = app.pick_slop();
        let edge = Point::new(100.0, slop / 2.0);
        assert!(
            !app.session.selection.contains(board),
            "the fixture must reach the state: with the frame already selected, \
             `selected_frame_at` answers and there is nothing to show"
        );
        app.entered_group = Some(board);

        let screen = app.to_screen(edge, rect, 1.0);
        let _ = ctx.run_ui(Default::default(), |ui| {
            app.begin_select_drag(ui, screen, rect, 1.0, &None, false);
        });

        assert!(
            matches!(app.drag, Drag::Move { .. }),
            "a press on the border grabs the frame rather than starting a marquee"
        );
        assert!(
            app.session.selection.contains(board),
            "and selects it, like the click does"
        );
        assert_eq!(
            app.entered_group,
            Some(board),
            "and does not clear the entered group, which only the marquee arm does"
        );
    }
}

#[cfg(test)]
mod pointer_allowance_tests {
    //! A pointer allowance is the same number of points at every display scale.
    use crate::app::OndinApp;
    use ondin_core::kurbo::Point;

    /// The pick band measured on screen through `to_screen` (not `pick_slop * points_per_world`,
    /// which is circular) at 100, 125, 150 and 200%, plus the snap tolerance.
    /// flip: `points_per_world` answers `camera.zoom` alone → fails at 125%.
    #[test]
    fn the_pick_band_is_four_points_at_every_display_scale() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.session.camera.zoom = 1.0;
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            app.canvas_ppp = ppp;
            let slop = app.pick_slop();
            let a = app.to_screen(Point::new(0.0, 0.0), rect, ppp);
            let b = app.to_screen(Point::new(slop, 0.0), rect, ppp);
            let points = (b.x - a.x).abs();
            assert!(
                (points - 4.0).abs() < 1e-3,
                "at {}% the band is {points} points, and it is 4 at every scale",
                ppp * 100.0
            );
            assert_eq!(
                app.snapping(&[]).zoom,
                app.points_per_world(),
                "and snapping's tolerance scale is the same conversion"
            );
        }
    }
}

#[cfg(test)]
mod group_fence_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, Operation, Transaction};

    /// A layer on a card in a row hops between the row's cards and never out of the row;
    /// a loose shape on the page dropped on a card goes in (nothing fences it).
    /// flip: delete the `group_fence` arm of `move_destination` → fails at "onto open page it stays on its card".
    #[test]
    fn a_layer_on_a_card_hops_between_cards_and_never_out_of_the_row() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xFE4CE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (page, row, a, b, icon, loose) = (
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
        );
        let create = |id, parent, kind, at: (f64, f64)| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        };
        let frame = |w, h| NodeKind::Artboard {
            size: Size::new(w, h),
        };
        let square = || NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
        };
        doc.apply(&Transaction(vec![
            create(page, root, frame(1000.0, 1000.0), (0.0, 0.0)),
            create(row, page, NodeKind::Group, (0.0, 0.0)),
            create(a, row, frame(100.0, 100.0), (0.0, 0.0)),
            create(b, row, frame(100.0, 100.0), (200.0, 0.0)),
            create(icon, a, square(), (10.0, 10.0)),
            create(loose, page, square(), (600.0, 600.0)),
        ]))
        .expect("the tree");
        app.session.adopt_document(doc, None);

        assert_eq!(
            app.move_destination(icon, Vec2::new(200.0, 0.0)),
            Some(b),
            "to the next card"
        );
        assert_eq!(
            app.move_destination(icon, Vec2::new(500.0, 500.0)),
            None,
            "onto open page it stays on its card"
        );
        assert_eq!(
            app.move_destination(loose, Vec2::new(-390.0, -590.0)),
            Some(b),
            "a loose shape goes into a card in a row"
        );
    }

    /// A frame nudged inside its holder stays there: the moved frame and its contents are
    /// excluded before `frame_covering` takes the topmost.
    /// flip: filter after `frame_covering` → fails at "a nudge keeps it in its set".
    #[test]
    fn a_frame_nudged_inside_its_parent_frame_stays_in_it() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xFA5E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (set, variant, other) = (ids.mint(), ids.mint(), ids.mint());
        let create = |id, parent, w, at: (f64, f64)| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(w, w),
            },
            transform: Some(Affine::translate(at)),
            name: None,
        };
        doc.apply(&Transaction(vec![
            create(set, root, 300.0, (0.0, 0.0)),
            create(variant, set, 100.0, (10.0, 10.0)),
            create(other, root, 300.0, (1000.0, 0.0)),
        ]))
        .expect("the tree");
        app.session.adopt_document(doc, None);
        assert_eq!(
            app.move_destination(variant, Vec2::new(4.0, 0.0)),
            None,
            "a nudge keeps it in its set"
        );
        assert_eq!(
            app.move_destination(variant, Vec2::new(0.0, 600.0)),
            Some(root),
            "clear of the set, onto the page"
        );
        assert_eq!(
            app.move_destination(variant, Vec2::new(1100.0, 0.0)),
            Some(other),
            "onto another frame, into it"
        );
    }

    /// Two overlapping sibling frames dragged together both stay on the canvas: every selected
    /// frame is excluded as a drop candidate. An Alt-drag keeps the narrow exclusion.
    /// flip: narrow the moving set to `[id]` in `moving_with` → fails at "stacked: A stays on the canvas".
    #[test]
    fn overlapping_frames_dragged_together_stay_on_the_canvas() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xFA5F);
        let root = ids.mint();
        let (a, b) = (ids.mint(), ids.mint());
        let create = |id, w, at: (f64, f64)| Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(w, w),
            },
            transform: Some(Affine::translate(at)),
            name: None,
        };
        let delta = Vec2::new(4.0, 0.0);
        for (case, b_side, b_at) in [
            ("stacked", 100.0, (0.0, 0.0)),
            ("small over large", 300.0, (-100.0, -100.0)),
        ] {
            let mut doc = Document::new(root);
            doc.apply(&Transaction(vec![
                create(b, b_side, b_at),
                create(a, 100.0, (0.0, 0.0)),
            ]))
            .expect("the tree");
            app.session.adopt_document(doc, None);
            app.session.selection.set(vec![a, b]);
            assert_eq!(
                app.move_destination(a, delta),
                None,
                "{case}: A stays on the canvas"
            );
            assert_eq!(
                app.move_destination(b, delta),
                None,
                "{case}: B stays on the canvas"
            );
            let tx = app.move_tx(delta);
            app.session.try_commit(tx).expect("the move commits");
            for id in [a, b] {
                assert_eq!(
                    app.session.doc.get(id).and_then(|n| n.parent()),
                    Some(root),
                    "{case}: both frames are still on the canvas"
                );
            }
        }
    }

    /// No drop destination is a frame the component rules refuse (nested main, cycle, a set
    /// of two kinds): the drop passes to the frame beneath.
    /// flip: `covering_that_holds` returns `frame_covering` unfiltered → fails at "a main over a main stays on the canvas".
    /// flip: disable `can_hold`'s set-kind clause → fails at "a group main over a set of frames stays on the canvas".
    #[test]
    fn a_drop_never_lands_where_the_component_rules_refuse_it() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xFA60);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [k, m, v, f, g, gr] = [(); 6].map(|_| ids.mint());
        let node = |id, parent, kind, at: (f64, f64)| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        };
        let frame = |w| NodeKind::Artboard {
            size: Size::new(w, w),
        };
        doc.apply(&Transaction(vec![
            node(k, root, frame(200.0), (0.0, 0.0)),
            node(m, root, frame(100.0), (500.0, 0.0)),
            node(v, root, frame(100.0), (0.0, 600.0)),
            node(f, root, frame(100.0), (300.0, 600.0)),
            node(g, root, NodeKind::Group, (900.0, 600.0)),
            node(
                gr,
                g,
                NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                (0.0, 0.0),
            ),
        ]))
        .expect("the tree");
        app.session.adopt_document(doc, None);
        for main in [k, m, v, f, g] {
            assert!(
                app.session
                    .commit(Transaction(vec![Operation::SetComponent {
                        id: main,
                        component: true,
                    }]))
            );
        }
        app.session.selection.set_one(m);
        app.duplicate_selection();
        let i = app.session.selection.single().expect("the instance");
        assert_eq!(app.session.doc.get(i).and_then(|n| n.link()), Some(m));
        let at = |app: &OndinApp, id| app.session.resolved.world_bounds(id).unwrap();
        let to =
            Affine::translate((800.0, 0.0)) * Affine::translate(-at(&app, i).origin().to_vec2());
        let placed = to * app.session.doc.get(i).unwrap().transform();
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: i,
                    transform: placed,
                }]))
        );
        app.session.selection.set_one(v);
        app.combine_as_variants();
        let set = app.session.selection.single().expect("the set");
        assert!(
            ondin_core::variant::is_set(&app.session.doc, set),
            "the fixture's set"
        );

        let onto = |app: &OndinApp, id, target| {
            let (from, to) = (at(app, id), at(app, target));
            Vec2::new(to.x0 - from.x0 + 10.0, to.y0 - from.y0 + 10.0)
        };
        let drop = |app: &mut OndinApp, id, target, case: &str| {
            app.session.selection.set_one(id);
            let tx = app.move_tx(onto(app, id, target));
            app.session
                .try_commit(tx)
                .unwrap_or_else(|e| panic!("{case}: the move commits: {e}"));
        };

        let d = onto(&app, m, k);
        app.session.selection.set_one(m);
        assert_eq!(
            app.move_destination(m, d),
            None,
            "a main over a main stays on the canvas"
        );

        app.session.selection.set_one(i);
        assert_eq!(
            app.move_destination(i, onto(&app, i, m)),
            None,
            "an instance over its own main stays on the canvas"
        );
        drop(&mut app, i, m, "instance into its main");
        assert_eq!(app.session.doc.get(i).unwrap().parent(), Some(root));

        drop(&mut app, m, k, "main over main");
        assert_eq!(app.session.doc.get(m).unwrap().parent(), Some(root));

        app.session.selection.set_one(g);
        let delta = onto(&app, g, set);
        assert_eq!(
            app.move_destination(g, delta),
            None,
            "a group main over a set of frames stays on the canvas"
        );
        assert_eq!(
            app.drops_into_a_set_of_another_kind(delta),
            Some(SET_OF_ANOTHER_KIND),
            "and that drop is the one the release explains"
        );
        drop(&mut app, g, set, "group into a set of frames");
        assert_eq!(app.session.doc.get(g).unwrap().parent(), Some(root));

        app.session.selection.set_one(f);
        let delta = onto(&app, f, set);
        assert_eq!(
            app.move_destination(f, delta),
            Some(set),
            "a frame main over the set goes in"
        );
        assert_eq!(app.drops_into_a_set_of_another_kind(delta), None);
    }

    /// Mains of two kinds dragged together into an empty set both stay out and the release
    /// says why; `f` alone goes in. `holds` asks `variant::takes_together` over the whole moving set.
    /// flip: `set_takes_moving` answers `true` always → fails at "the frame main stays out of an empty set beside a group main".
    #[test]
    fn mains_of_two_kinds_dragged_together_stay_out_of_an_empty_set() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xFA61);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [set, f, g, gr] = [(); 4].map(|_| ids.mint());
        let node = |id, parent, kind, at: (f64, f64)| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        };
        let frame = |w| NodeKind::Artboard {
            size: Size::new(w, w),
        };
        doc.apply(&Transaction(vec![
            node(set, root, frame(300.0), (0.0, 0.0)),
            node(f, root, frame(100.0), (500.0, 0.0)),
            node(g, root, NodeKind::Group, (620.0, 0.0)),
            node(
                gr,
                g,
                NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                (0.0, 0.0),
            ),
            Operation::SetVariantSet {
                id: set,
                set: Some(Default::default()),
            },
            Operation::SetComponent {
                id: f,
                component: true,
            },
            Operation::SetComponent {
                id: g,
                component: true,
            },
        ]))
        .expect("the tree");
        app.session.adopt_document(doc, None);
        assert!(ondin_core::variant::is_set(&app.session.doc, set));
        assert!(ondin_core::variant::variants(&app.session.doc, set).is_empty());

        let delta = Vec2::new(-490.0, 10.0);
        app.session.selection.set(vec![f, g]);
        assert_eq!(
            app.move_destination(f, delta),
            None,
            "the frame main stays out of an empty set beside a group main"
        );
        assert_eq!(
            app.move_destination(g, delta),
            None,
            "the group main stays out of an empty set beside a frame main"
        );
        assert_eq!(
            app.drops_into_a_set_of_another_kind(delta),
            Some(SET_OF_MIXED_KINDS),
            "and the release says why"
        );
        let tx = app.move_tx(delta);
        app.session.try_commit(tx).expect("the move commits");
        assert!(ondin_core::variant::variants(&app.session.doc, set).is_empty());

        let nudge = Vec2::new(1.0, 0.0);
        app.session.selection.set_one(f);
        assert_eq!(app.move_destination(f, nudge), Some(set));
        assert_eq!(app.drops_into_a_set_of_another_kind(nudge), None);
    }

    /// An occupied card in a row is a click target; an occupied page is skipped so a
    /// marquee can start on its background.
    /// flip: drop the `group_fence` clause from `pick_leaf`'s filter → fails at "the card's background".
    #[test]
    fn a_card_in_a_row_is_clickable_on_its_background_and_a_page_is_not() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xBACC);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (page, row, card, icon) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let create = |id, parent, kind, at: (f64, f64)| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        };
        doc.apply(&Transaction(vec![
            create(
                page,
                root,
                NodeKind::Artboard {
                    size: Size::new(1000.0, 1000.0),
                },
                (0.0, 0.0),
            ),
            create(row, page, NodeKind::Group, (0.0, 0.0)),
            create(
                card,
                row,
                NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                (0.0, 0.0),
            ),
            create(
                icon,
                card,
                NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                (10.0, 10.0),
            ),
        ]))
        .expect("the tree");
        app.session.adopt_document(doc, None);
        app.session.camera.zoom = 1.0;

        assert_eq!(
            app.pick_leaf(Point::new(15.0, 15.0)),
            Some(icon),
            "the icon"
        );
        assert_eq!(
            app.pick_leaf(Point::new(60.0, 60.0)),
            Some(card),
            "the card's background"
        );
        assert_eq!(
            app.pick_leaf(Point::new(600.0, 600.0)),
            None,
            "the page's background is left to the marquee"
        );
    }
}

#[cfg(test)]
mod flex_drag_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::container::{AlignItems, Display, Flex};
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, Operation, Transaction};

    /// Moving one flex item past its neighbour's centre commits one `Reorder` and no
    /// `SetTransform`; short of it, an empty transaction; well below the frame, it leaves the frame.
    /// flip: delete `move_tx`'s reorder arm → fails at "one reorder" with an empty transaction.
    #[test]
    fn moving_a_flex_item_commits_a_reorder_and_nothing_else() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xF1D);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, a, b, c) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let create = |id, parent, index, kind| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: None,
        };
        let rect = |w| NodeKind::Rect {
            size: Size::new(w, 30.0),
            corner_radii: Default::default(),
        };
        doc.apply(&Transaction(vec![
            create(
                frame,
                root,
                0,
                NodeKind::Artboard {
                    size: Size::new(400.0, 200.0),
                },
            ),
            create(a, frame, 0, rect(40.0)),
            create(b, frame, 1, rect(60.0)),
            create(c, frame, 2, rect(20.0)),
            Operation::SetDisplay {
                id: frame,
                display: Some(Display::Flex(Flex {
                    column_gap: 10.0,
                    padding: [20.0; 4],
                    align_items: AlignItems::Start,
                    ..Default::default()
                })),
            },
        ]))
        .expect("the row");
        app.session.adopt_document(doc, None);

        app.session.selection.set_one(a);
        let tx = app.move_tx(Vec2::new(70.0, 0.0));
        assert!(
            matches!(tx.0.as_slice(), [Operation::Reorder { id, index: 1 }] if *id == a),
            "one reorder: {tx:?}"
        );
        assert!(
            app.move_tx(Vec2::new(20.0, 0.0)).0.is_empty(),
            "short of the neighbour: nothing"
        );

        app.session.selection.set(vec![a, b]);
        let tx = app.move_tx(Vec2::new(70.0, 0.0));
        assert!(
            tx.0.is_empty(),
            "two items dropped short of their neighbour: nothing, and no translation \
             is stored: {tx:?}"
        );

        app.session.selection.set_one(a);
        let tx = app.move_tx(Vec2::new(0.0, 400.0));
        assert!(
            tx.0.iter()
                .any(|op| matches!(op, Operation::Reparent { id, .. } if *id == a)),
            "dragged out of the frame, it leaves: {tx:?}"
        );

        app.session.selection.set_one(a);
        let tx = app
            .move_preview_tx(Vec2::new(70.0, 0.0))
            .expect("a preview");
        assert!(
            tx.0.iter()
                .any(|op| matches!(op, Operation::SetTransform { id, .. } if *id == a))
                && tx
                    .0
                    .iter()
                    .any(|op| matches!(op, Operation::Reorder { .. })),
            "one item: under the hand, and its reorder: {tx:?}"
        );
        app.session.selection.set(vec![a, b]);
        let tx = app
            .move_preview_tx(Vec2::new(70.0, 0.0))
            .expect("a preview");
        assert_eq!(
            tx.0.iter()
                .filter(|op| matches!(op, Operation::SetTransform { .. }))
                .count(),
            2,
            "two items: both under the hand, as a block: {tx:?}"
        );
    }
}

#[cfg(test)]
mod flex_canvas_tests {
    //! The app reads where layout draws a layer: clicks select, snapping measures, and a real
    //! drag through `canvas_ui` reorders the row.

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::container::{AlignItems, Display, Flex};
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource, Operation, Transaction};

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };
    const PPP: f32 = 1.0;

    /// A 400 × 200 frame laid out as a row (padding 20, gap 10) of a 40 × 30, a 60 × 30 and a
    /// 20 × 30, all stored at (300, 150). Drawn: `a` at (20, 20)–(60, 50), `b` at (70, 20)–(130, 50).
    fn app_with_a_row(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xAE);
        let root = ids.mint();
        let (row, a, b, c) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let rect = |id, index, w| Operation::CreateNode {
            id,
            parent: row,
            index,
            kind: NodeKind::Rect {
                size: Size::new(w, 30.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((300.0, 150.0))),
            name: None,
        };
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: row,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 200.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            rect(a, 0, 40.0),
            rect(b, 1, 60.0),
            rect(c, 2, 20.0),
            Operation::SetDisplay {
                id: row,
                display: Some(Display::Flex(Flex {
                    column_gap: 10.0,
                    padding: [20.0; 4],
                    align_items: AlignItems::Start,
                    ..Default::default()
                })),
            },
        ]))
        .expect("the row");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(200.0, 100.0);
        app.session.camera.zoom = 1.0;
        // `to_screen` reads the canvas size, which the canvas learns by drawing.
        for _ in 0..2 {
            frame(ctx, &mut app, Vec::new());
        }
        (app, row, a, b)
    }

    /// One canvas frame with `events`, a tenth of a second after the last (egui tells clicks by the clock).
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let time = ctx.input(|i| i.time) + 0.1;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                app.canvas_ui(ui);
            },
        );
    }

    fn button(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// Press at world `from`, drag to `to` in eight steps, release, each in its own frame.
    fn drag(ctx: &egui::Context, app: &mut OndinApp, from: Point, to: Point) {
        let at = |app: &OndinApp, w: Point| app.to_screen(w, RECT, PPP);
        let start = at(app, from);
        frame(ctx, app, vec![egui::Event::PointerMoved(start)]);
        frame(ctx, app, vec![button(start, true)]);
        for step in 1..=8 {
            let t = step as f64 / 8.0;
            let w = from + (to - from) * t;
            let p = at(app, w);
            frame(ctx, app, vec![egui::Event::PointerMoved(p)]);
        }
        let end = at(app, to);
        frame(ctx, app, vec![button(end, false)]);
        frame(ctx, app, Vec::new());
    }

    /// `drag` with Alt held on every frame from press to release, let go on the frame after.
    fn alt_drag(ctx: &egui::Context, app: &mut OndinApp, from: Point, to: Point) {
        let alt = egui::Modifiers {
            alt: true,
            ..Default::default()
        };
        let held = |ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>| {
            let time = ctx.input(|i| i.time) + 0.1;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(RECT),
                    time: Some(time),
                    modifiers: alt,
                    events,
                    ..Default::default()
                },
                |ui| app.canvas_ui(ui),
            );
        };
        let press = |at: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: alt,
        };
        let at = |app: &OndinApp, w: Point| app.to_screen(w, RECT, PPP);
        let start = at(app, from);
        held(ctx, app, vec![egui::Event::PointerMoved(start)]);
        held(ctx, app, vec![press(start, true)]);
        for step in 1..=8 {
            let w = from + (to - from) * (step as f64 / 8.0);
            let p = at(app, w);
            held(ctx, app, vec![egui::Event::PointerMoved(p)]);
        }
        let end = at(app, to);
        held(ctx, app, vec![press(end, false)]);
        frame(ctx, app, Vec::new());
    }

    /// A click selects a flex item where it is drawn, and nothing at its stored place.
    #[test]
    fn a_click_selects_a_flex_item_where_it_is_drawn() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame_id, a, _b) = app_with_a_row(&ctx);
        let click = |app: &mut OndinApp, w: Point| {
            let p = app.to_screen(w, RECT, PPP);
            frame(&ctx, app, vec![egui::Event::PointerMoved(p)]);
            frame(&ctx, app, vec![button(p, true)]);
            frame(&ctx, app, vec![button(p, false)]);
            frame(&ctx, app, Vec::new());
        };
        click(&mut app, Point::new(40.0, 35.0));
        assert_eq!(app.session.selection.ids(), &[a], "drawn there: selected");
        click(&mut app, Point::new(320.0, 165.0));
        assert!(
            app.session.selection.ids().is_empty(),
            "stored there: nothing — the click is on the frame's empty interior, \
             which an occupied frame does not answer (§15 D22)"
        );
        let _ = frame_id;
    }

    /// A pinned layer is hit where its insets draw it: `b` pinned to the right edge is
    /// drawn at x 340–400, and nothing of it is left in the row's slot.
    #[test]
    fn a_click_selects_a_pinned_layer_where_its_insets_draw_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _frame, _a, b) = app_with_a_row(&ctx);
        app.session.commit(Transaction(vec![Operation::SetInsets {
            id: b,
            insets: ondin_core::Insets {
                right: Some(ondin_core::LengthPct::Px(0.0)),
                ..Default::default()
            },
        }]));
        assert_eq!(
            app.session.resolved.world_bounds(b).map(|r| r.x0),
            Some(340.0),
            "the fixture: pinned to the right edge"
        );
        let click = |app: &mut OndinApp, w: Point| {
            let p = app.to_screen(w, RECT, PPP);
            frame(&ctx, app, vec![egui::Event::PointerMoved(p)]);
            frame(&ctx, app, vec![button(p, true)]);
            frame(&ctx, app, vec![button(p, false)]);
            frame(&ctx, app, Vec::new());
        };
        click(&mut app, Point::new(370.0, 165.0));
        assert_eq!(app.session.selection.ids(), &[b], "where the pin draws it");
        click(&mut app, Point::new(100.0, 35.0));
        assert!(
            app.session.selection.ids().is_empty(),
            "and not in the row's old slot"
        );
    }

    /// `snap::targets` offers a flex item's drawn box, (20, 20)–(60, 50), and nothing at its stored place.
    #[test]
    fn snapping_measures_a_flex_items_drawn_edges() {
        let ctx = egui::Context::default();
        let (app, frame_id, a, b) = app_with_a_row(&ctx);
        let doc = &app.session.doc;
        let res = &app.session.resolved;
        let targets = crate::snap::targets(
            doc,
            res,
            &[b],
            ondin_core::kurbo::Rect::new(-100.0, -100.0, 600.0, 400.0),
            100,
        );
        assert!(
            targets.contains(&ondin_core::kurbo::Rect::new(20.0, 20.0, 60.0, 50.0)),
            "a's drawn box is a target: {targets:?}"
        );
        assert!(
            !targets.iter().any(|r| r.x0 == 300.0 && r.y0 == 150.0),
            "and nothing where it is stored: {targets:?}"
        );
        let _ = (frame_id, a);
    }

    /// A real drag of `b` left past `a` reorders the row as one undo step, its stored
    /// transform untouched; a preview is up mid-drag.
    /// flip: delete `move_tx`'s reorder arm → fails at "b comes first".
    #[test]
    fn a_real_drag_reorders_the_row() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame_id, a, b) = app_with_a_row(&ctx);
        let depth = app.session.history.undo_depth();
        let from = Point::new(100.0, 35.0);
        let to = Point::new(22.0, 35.0);
        let start = app.to_screen(from, RECT, PPP);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(start)]);
        frame(&ctx, &mut app, vec![button(start, true)]);
        for step in 1..=8 {
            let w = from + (to - from) * (step as f64 / 8.0);
            let p = app.to_screen(w, RECT, PPP);
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        }
        assert!(
            app.session.has_gesture_preview(),
            "mid-drag, a preview is up"
        );
        let end = app.to_screen(to, RECT, PPP);
        frame(&ctx, &mut app, vec![button(end, false)]);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            app.session.doc.get(frame_id).unwrap().children()[..2],
            [b, a],
            "b comes first"
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        assert_eq!(
            app.session.doc.get(b).unwrap().transform(),
            Affine::translate((300.0, 150.0)),
            "and its stored transform is untouched"
        );
    }

    /// A hand drag that ends on the new slot draws the drop outline there: with shape snapping
    /// off and the pixel grid on, the grid rounds the move to (90, 20), the reorder's slot.
    /// flip: `RenderOverrides::landing_of` falls back to the document's transform → fails at "the outline is on the slot".
    #[test]
    fn a_drag_ending_on_the_new_slot_outlines_the_slot() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _frame_id, a, _b) = app_with_a_row(&ctx);
        // Shape snapping would claim both axes first.
        app.snap_shapes = false;
        assert!(app.snap_grid, "the default this test is about");
        let press = Point::new(40.0, 35.0);
        let start = app.to_screen(press, RECT, PPP);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(start)]);
        frame(&ctx, &mut app, vec![button(start, true)]);
        let mut anchor = None;
        for step in 1..=10 {
            let p = app.to_screen(press + Vec2::new(2.0 * step as f64, 0.0), RECT, PPP);
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
            if let crate::preview::Drag::Move { anchor: at } = app.drag {
                anchor = Some(at);
                break;
            }
        }
        let anchor = anchor.expect("the move has begun");
        let to = anchor + Vec2::new(70.3, 0.2);
        for step in 1..=8 {
            let w = anchor + (to - anchor) * (step as f64 / 8.0);
            let p = app.to_screen(w, RECT, PPP);
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        }
        let (_, _, ov) = app.session.render_inputs();
        let landing = ov
            .landings()
            .iter()
            .find(|(id, _, _)| *id == a)
            .map(|(_, world, _)| world.translation())
            .expect("a reorder under way, so a landing");
        assert_eq!(
            landing,
            ondin_core::kurbo::Vec2::new(90.0, 20.0),
            "the outline is on the slot"
        );
    }

    /// Two selected items dragged past the third reorder as a block, in order, as one undo
    /// step with no stored transform touched.
    /// flip: delete `move_tx`'s block arm → fails at "the pair lands after the third, in order".
    #[test]
    fn two_items_dragged_together_reorder_as_a_block() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame_id, a, b) = app_with_a_row(&ctx);
        let third = app.session.doc.get(frame_id).unwrap().children()[2];
        app.session.selection.set(vec![a, b]);
        let depth = app.session.history.undo_depth();
        drag(
            &ctx,
            &mut app,
            Point::new(40.0, 35.0),
            Point::new(160.0, 35.0),
        );
        assert_eq!(
            app.session.doc.get(frame_id).unwrap().children(),
            &[third, a, b],
            "the pair lands after the third, in order"
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        for id in [a, b] {
            assert_eq!(
                app.session.doc.get(id).unwrap().transform(),
                Affine::translate((300.0, 150.0)),
                "and no stored transform touched"
            );
        }
    }

    /// A real drag in a grid writes the lines of the cell it is dropped on (`3 / auto`,
    /// `2 / auto`), `b` flows back into the first cell, as one undo step.
    /// flip: `flex_reorder_of` asks `build::flex_reorder` → fails at "a lands in column 3, row 2".
    #[test]
    fn a_real_drag_in_a_grid_writes_the_cell_it_lands_in() {
        use ondin_core::container::{GridLines, GridPlacement};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, a, b) = app_with_a_grid(&ctx);
        let bounds = |app: &OndinApp, id| app.session.resolved.world_bounds(id).unwrap();
        assert_eq!(
            bounds(&app, b).origin(),
            Point::new(100.0, 0.0),
            "the fixture"
        );

        let depth = app.session.history.undo_depth();
        drag(
            &ctx,
            &mut app,
            Point::new(10.0, 10.0),
            Point::new(250.0, 150.0),
        );
        assert_eq!(
            bounds(&app, a).origin(),
            Point::new(200.0, 100.0),
            "a lands in column 3, row 2"
        );
        let item = *app.session.doc.get(a).unwrap().item();
        assert_eq!(
            (item.grid_column.start, item.grid_row.start),
            (GridPlacement::Line(3), GridPlacement::Line(2))
        );
        assert_eq!(item.grid_column.end, GridLines::default().end);
        assert_eq!(bounds(&app, b).origin(), Point::ZERO, "b flows back");
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        assert_eq!(
            app.session.doc.get(a).unwrap().transform(),
            Affine::translate((300.0, 150.0)),
            "and its stored transform is untouched"
        );
    }

    /// Two items of one grid dragged together move as a block: the release writes both
    /// items' lines (`a` in column 3, `b` in column 4 of row 2) and no transform.
    /// flip: `flex_block_reorder_of` asks `build::flex_reorder_many` → fails at "both written" (empty transaction).
    #[test]
    fn two_grid_items_dragged_together_move_as_a_block() {
        use ondin_core::container::GridPlacement;
        let ctx = egui::Context::default();
        let (mut app, a, b) = app_with_a_grid(&ctx);
        app.session.selection.set(vec![a, b]);
        let tx = app.move_tx(Vec2::new(200.0, 100.0));
        assert_eq!(tx.0.len(), 2, "both written: {tx:?}");
        assert!(
            tx.0.iter()
                .all(|op| matches!(op, Operation::SetLayoutItem { .. })),
            "lines, and no transform: {tx:?}"
        );
        app.session.doc.apply(&tx).expect("applies");
        let start = |id| {
            let item = *app.session.doc.get(id).unwrap().item();
            (item.grid_column.start, item.grid_row.start)
        };
        assert_eq!(start(a), (GridPlacement::Line(3), GridPlacement::Line(2)));
        assert_eq!(start(b), (GridPlacement::Line(4), GridPlacement::Line(2)));
    }

    /// A 400 × 200 frame as a four-by-two grid of 100 × 100 cells holding two 20 × 20 rects:
    /// `a` drawn at (0, 0) and `b` at (100, 0), both stored at (300, 150).
    fn app_with_a_grid(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        use ondin_core::container::{Grid, Track, TrackBreadth, TrackSize};
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xBA);
        let root = ids.mint();
        let (grid, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let rect = |id, index| Operation::CreateNode {
            id,
            parent: grid,
            index,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((300.0, 150.0))),
            name: None,
        };
        let cell = || Track::Size(TrackSize::Breadth(TrackBreadth::Px(100.0)));
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: grid,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 200.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            rect(a, 0),
            rect(b, 1),
            Operation::SetDisplay {
                id: grid,
                display: Some(Display::Grid(Grid {
                    columns: vec![cell(), cell(), cell(), cell()],
                    rows: vec![cell(), cell()],
                    ..Default::default()
                })),
            },
        ]))
        .expect("the grid");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(200.0, 100.0);
        app.session.camera.zoom = 1.0;
        for _ in 0..2 {
            frame(ctx, &mut app, Vec::new());
        }
        (app, a, b)
    }

    /// A themed context; one per `held` drag, since a second press with the button down starts nothing.
    fn fresh() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        ctx
    }

    /// Press at world `from` and drag to `to` in eight steps, button still down; returns the last step's shapes.
    fn held(
        ctx: &egui::Context,
        app: &mut OndinApp,
        from: Point,
        to: Point,
    ) -> Vec<egui::epaint::ClippedShape> {
        let start = app.to_screen(from, RECT, PPP);
        frame(ctx, app, vec![egui::Event::PointerMoved(start)]);
        frame(ctx, app, vec![button(start, true)]);
        let mut shapes = Vec::new();
        for step in 1..=8 {
            let p = app.to_screen(from + (to - from) * (step as f64 / 8.0), RECT, PPP);
            let time = ctx.input(|i| i.time) + 0.1;
            shapes = ctx
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(RECT),
                        time: Some(time),
                        events: vec![egui::Event::PointerMoved(p)],
                        ..Default::default()
                    },
                    |ui| app.canvas_ui(ui),
                )
                .shapes;
        }
        shapes
    }

    /// The screen box of the landing outline in `shapes` (`draw_flex_landing`'s 1.5-wide
    /// `SELECT` dashes), or `None`.
    fn landing_outline(shapes: &[egui::epaint::ClippedShape]) -> Option<egui::Rect> {
        shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::LineSegment { points, stroke }
                    if stroke.color == color::SELECT && stroke.width == 1.5 =>
                {
                    Some(egui::Rect::from_two_pos(points[0], points[1]))
                }
                _ => None,
            })
            .reduce(|a, b| a.union(b))
    }

    /// Whether `outline` is the world box `x0, y0, x1, y1` on screen.
    fn outlines(app: &OndinApp, outline: Option<egui::Rect>, [x0, y0, x1, y1]: [f64; 4]) -> bool {
        let want = egui::Rect::from_two_pos(
            app.to_screen(Point::new(x0, y0), RECT, PPP),
            app.to_screen(Point::new(x1, y1), RECT, PPP),
        );
        outline.is_some_and(|o| {
            (o.min - want.min).length() < 0.01 && (o.max - want.max).length() < 0.01
        })
    }

    /// The canvas outlines the slot of an item staying in its row, (90, 20)–(130, 50), and
    /// nothing for one leaving its frame, though the preview records a landing for it.
    /// flip: `draw_flex_landing` outlines every landing, `flex_reorder_of`'s gate dropped → fails at "nothing for an item leaving its frame".
    #[test]
    fn the_landing_outline_is_drawn_for_a_reorder_and_not_for_a_departure() {
        let ctx = fresh();
        let (mut app, _row, _a, _b) = app_with_a_row(&ctx);
        let shapes = held(
            &ctx,
            &mut app,
            Point::new(40.0, 35.0),
            Point::new(125.0, 35.0),
        );
        assert!(
            outlines(&app, landing_outline(&shapes), [90.0, 20.0, 130.0, 50.0]),
            "a's slot after b: {:?}",
            landing_outline(&shapes)
        );

        let ctx = fresh();
        let (mut app, _row, a, _b) = app_with_a_row(&ctx);
        let shapes = held(
            &ctx,
            &mut app,
            Point::new(40.0, 35.0),
            Point::new(520.0, 35.0),
        );
        let (_, _, ov) = app.session.render_inputs();
        assert!(
            ov.landings().iter().any(|(id, _, _)| *id == a),
            "the fixture: the preview still lays a out in the row"
        );
        assert_eq!(
            landing_outline(&shapes),
            None,
            "nothing for an item leaving its frame"
        );
    }

    /// A block of items draws no landing outline in a flex row or a grid, and one grid
    /// item outlines its cell, (200, 100)–(220, 120) for column 3, row 2.
    /// flip: the one above's → fails here at "no outline for a flex block".
    #[test]
    fn a_block_draws_no_landing_outline_and_a_grid_item_its_cell() {
        let ctx = fresh();
        let (mut app, _a, _b) = app_with_a_grid(&ctx);
        let shapes = held(
            &ctx,
            &mut app,
            Point::new(10.0, 10.0),
            Point::new(250.0, 150.0),
        );
        assert!(
            outlines(&app, landing_outline(&shapes), [200.0, 100.0, 220.0, 120.0]),
            "a's cell: {:?}",
            landing_outline(&shapes)
        );

        let ctx = fresh();
        let (mut app, _row, a, b) = app_with_a_row(&ctx);
        app.session.selection.set(vec![a, b]);
        let shapes = held(
            &ctx,
            &mut app,
            Point::new(40.0, 35.0),
            Point::new(160.0, 35.0),
        );
        assert!(app.session.has_gesture_preview(), "the fixture: a drag");
        assert_eq!(
            landing_outline(&shapes),
            None,
            "no outline for a flex block"
        );

        let ctx = fresh();
        let (mut app, a, b) = app_with_a_grid(&ctx);
        app.session.selection.set(vec![a, b]);
        let shapes = held(
            &ctx,
            &mut app,
            Point::new(10.0, 10.0),
            Point::new(250.0, 150.0),
        );
        assert!(app.session.has_gesture_preview(), "the fixture: a drag");
        assert_eq!(landing_outline(&shapes), None, "and none for a grid block");
    }

    /// A mixed selection's move leaves the in-flow item in its slot and moves the free
    /// layer, in the preview and in the commit (`stays_in_flow`).
    /// flip: `stays_in_flow` answers `false` → fails at "the preview leaves a in its slot".
    /// flip: disable `move_tx`'s own `stays_in_flow` arm → fails at "and so does the commit".
    #[test]
    fn a_mixed_selection_moves_the_free_layer_and_keeps_the_item_in_flow() {
        let ctx = fresh();
        let (mut app, row, a, _b) = app_with_a_row(&ctx);
        let free = NodeId::from_wire("ae:99").expect("an id past the row's");
        app.session.commit(Transaction(vec![Operation::CreateNode {
            id: free,
            parent: app.session.doc.root(),
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((500.0, 300.0))),
            name: None,
        }]));
        app.session.selection.set(vec![a, free]);
        let delta = Vec2::new(30.0, 0.0);
        let moved = |tx: &Transaction| -> Vec<NodeId> {
            tx.0.iter()
                .filter_map(|op| match op {
                    Operation::SetTransform { id, .. } => Some(*id),
                    _ => None,
                })
                .collect()
        };
        let preview = app.move_preview_tx(delta).expect("a preview");
        assert_eq!(
            moved(&preview),
            vec![free],
            "the preview leaves a in its slot"
        );
        let tx = app.move_tx(delta);
        assert_eq!(moved(&tx), vec![free], "and so does the commit");
        app.session.commit(tx);
        assert_eq!(
            app.session.doc.get(free).unwrap().transform(),
            Affine::translate((530.0, 300.0))
        );
        assert_eq!(app.session.doc.get(row).unwrap().children()[0], a);
        assert_eq!(
            app.session.resolved.world_bounds(a).unwrap().origin(),
            Point::new(20.0, 20.0),
            "a drawn in its slot"
        );
        assert_eq!(
            app.session.doc.get(a).unwrap().transform(),
            Affine::translate((300.0, 150.0))
        );
    }

    /// A laid group resized by its handle through the canvas sets its box (`120px` ×
    /// `60px`), laid out from the same top-left with its rects unscaled, as one undo step.
    /// flip: delete `resize_layer`'s `laid_group` routing → fails at "the preview follows the handle".
    #[test]
    fn a_laid_group_resized_through_the_canvas_sets_its_box() {
        use ondin_core::container::Dimension;
        let ctx = fresh();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xAF);
        let root = ids.mint();
        let (group, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let square = |id, index| Operation::CreateNode {
            id,
            parent: group,
            index,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((100.0, 100.0))),
                name: None,
            },
            square(a, 0),
            square(b, 1),
            Operation::SetDisplay {
                id: group,
                display: Some(Display::Flex(Flex {
                    column_gap: 10.0,
                    padding: [5.0; 4],
                    ..Default::default()
                })),
            },
        ]))
        .expect("the group");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(200.0, 100.0);
        app.session.camera.zoom = 1.0;
        app.session.selection.set(vec![group]);
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new());
        }
        assert_eq!(
            app.session.resolved.world_bounds(group),
            Some(ondin_core::kurbo::Rect::new(100.0, 100.0, 160.0, 130.0)),
            "the fixture hugs"
        );
        let depth = app.session.history.undo_depth();
        let (from, to) = (Point::new(160.0, 130.0), Point::new(220.0, 160.0));
        let start = app.to_screen(from, RECT, PPP);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(start)]);
        frame(&ctx, &mut app, vec![button(start, true)]);
        for step in 1..=8 {
            let w = from + (to - from) * (step as f64 / 8.0);
            let p = app.to_screen(w, RECT, PPP);
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        }
        let seen = app.session.preview_world_bounds(group).expect("previewed");
        assert_eq!(
            (seen.x0, seen.y0, seen.x1, seen.y1),
            (100.0, 100.0, 220.0, 160.0),
            "the preview follows the handle"
        );
        let end = app.to_screen(to, RECT, PPP);
        frame(&ctx, &mut app, vec![button(end, false)]);
        frame(&ctx, &mut app, Vec::new());
        let item = *app.session.doc.get(group).unwrap().item();
        assert_eq!(
            (item.width, item.height),
            (Dimension::Px(120.0), Dimension::Px(60.0)),
            "the box"
        );
        assert_eq!(
            app.session.resolved.world_bounds(group),
            Some(ondin_core::kurbo::Rect::new(100.0, 100.0, 220.0, 160.0)),
            "laid out at it, from the same top-left"
        );
        let NodeKind::Rect { size, .. } = app.session.doc.get(a).unwrap().kind() else {
            panic!("a rect");
        };
        assert_eq!(*size, Size::new(20.0, 20.0), "the rects unscaled");
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
    }

    /// A block in a grid whose rows overflow its frame moves whole while the pointer is in
    /// the frame; the half-area rule alone would have `b` leaving by its box.
    /// flip: drop `move_destination`'s pointer test → fails at "b stays in the grid".
    /// flip: `finish_drag` builds the commit with the drag already cleared → same assertion.
    #[test]
    fn a_block_whose_rows_overflow_the_frame_moves_whole() {
        use ondin_core::container::{Grid, GridPlacement};
        let ctx = fresh();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xB0);
        let root = ids.mint();
        let grid = ids.mint();
        let items: Vec<NodeId> = (0..4).map(|_| ids.mint()).collect();
        let mut ops = vec![Operation::CreateNode {
            id: grid,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(300.0, 200.0),
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        }];
        for (index, id) in items.iter().enumerate() {
            ops.push(Operation::CreateNode {
                id: *id,
                parent: grid,
                index,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 150.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
        }
        ops.push(Operation::SetDisplay {
            id: grid,
            display: Some(Display::Grid(Grid {
                columns: ondin_core::container::parse_tracks("1fr 1fr 1fr").unwrap(),
                ..Default::default()
            })),
        });
        let mut doc = Document::new(root);
        doc.apply(&Transaction(ops)).expect("the grid");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(150.0, 150.0);
        app.session.camera.zoom = 1.0;
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new());
        }
        let (a, b) = (items[0], items[3]);
        let bounds = |app: &OndinApp, id| app.session.resolved.world_bounds(id).unwrap();
        assert_eq!(
            bounds(&app, b).y0,
            150.0,
            "the fixture: b in row 2, mostly clipped"
        );
        app.session.selection.set(vec![a, b]);
        drag(
            &ctx,
            &mut app,
            Point::new(10.0, 75.0),
            Point::new(125.0, 95.0),
        );
        assert_eq!(
            app.session.doc.get(b).unwrap().parent(),
            Some(grid),
            "b stays in the grid"
        );
        for id in [a, b] {
            assert_eq!(
                app.session.doc.get(id).unwrap().item().grid_column.start,
                GridPlacement::Line(2),
                "both in column 2"
            );
        }
        assert_eq!(bounds(&app, a).origin(), Point::new(100.0, 0.0));
        assert_eq!(bounds(&app, b).origin(), Point::new(100.0, 150.0));
    }

    /// A headless app over `ops` on a fresh document, camera at `centre`, zoom 1, settled.
    fn canvas_over(
        ctx: &egui::Context,
        root: NodeId,
        ops: Vec<Operation>,
        centre: Point,
    ) -> OndinApp {
        let mut app = OndinApp::headless(ctx);
        let mut doc = Document::new(root);
        doc.apply(&Transaction(ops)).expect("the fixture");
        app.session.adopt_document(doc, None);
        app.session.camera.center = centre;
        app.session.camera.zoom = 1.0;
        for _ in 0..2 {
            frame(ctx, &mut app, Vec::new());
        }
        app
    }

    fn rect_op(id: NodeId, parent: NodeId, index: usize, w: f64, h: f64) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(w, h),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }
    }

    fn frame_op(id: NodeId, parent: NodeId, index: usize, w: f64, h: f64, at: Affine) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(w, h),
            },
            transform: Some(at),
            name: None,
        }
    }

    /// An in-flow item dragged over a frame in its own container drops into it, though
    /// the pointer over the card is also in the row.
    /// flip: delete the nested-frame arm → fails at "into the card".
    #[test]
    fn an_in_flow_item_drops_into_a_frame_in_its_own_row() {
        let ctx = fresh();
        let mut ids = IdSource::new(0xBA);
        let (root, row, card, badge) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let mut app = canvas_over(
            &ctx,
            root,
            vec![
                frame_op(row, root, 0, 400.0, 200.0, Affine::IDENTITY),
                frame_op(card, row, 0, 150.0, 150.0, Affine::IDENTITY),
                rect_op(badge, row, 1, 30.0, 30.0),
                Operation::SetDisplay {
                    id: row,
                    display: Some(Display::Flex(Flex {
                        column_gap: 10.0,
                        padding: [20.0; 4],
                        align_items: AlignItems::Start,
                        ..Default::default()
                    })),
                },
            ],
            Point::new(200.0, 100.0),
        );
        let at = app.session.resolved.world_bounds(badge).unwrap();
        assert_eq!(at.origin(), Point::new(180.0, 20.0), "the fixture");
        app.session.selection.set(vec![badge]);
        drag(
            &ctx,
            &mut app,
            Point::new(195.0, 35.0),
            Point::new(95.0, 95.0),
        );
        assert_eq!(
            app.session.doc.get(badge).unwrap().parent(),
            Some(card),
            "into the card"
        );
    }

    /// An item pressed on the part that overflows its container stays in it for a small
    /// drag (the pointer was never in the container); dragged clear of the frame it leaves.
    /// flip: delete the pressed-outside clause → fails at "b stays".
    #[test]
    fn an_overflowing_item_pressed_outside_its_container_stays_for_a_nudge() {
        let ctx = fresh();
        let mut ids = IdSource::new(0xBB);
        let (root, row, a, b) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let build = |ctx: &egui::Context| {
            canvas_over(
                ctx,
                root,
                vec![
                    frame_op(row, root, 0, 200.0, 100.0, Affine::IDENTITY),
                    rect_op(a, row, 0, 150.0, 30.0),
                    rect_op(b, row, 1, 150.0, 30.0),
                    Operation::SetDisplay {
                        id: row,
                        display: Some(Display::Flex(Flex {
                            align_items: AlignItems::Start,
                            ..Default::default()
                        })),
                    },
                ],
                Point::new(150.0, 100.0),
            )
        };
        let mut app = build(&ctx);
        assert_eq!(
            app.session.resolved.world_bounds(b).unwrap().x0,
            150.0,
            "the fixture: b overflows"
        );
        app.session.selection.set(vec![b]);
        drag(
            &ctx,
            &mut app,
            Point::new(250.0, 15.0),
            Point::new(250.0, 35.0),
        );
        assert_eq!(
            app.session.doc.get(b).unwrap().parent(),
            Some(row),
            "b stays"
        );

        let ctx = fresh();
        let mut app = build(&ctx);
        app.session.selection.set(vec![b]);
        drag(
            &ctx,
            &mut app,
            Point::new(250.0, 15.0),
            Point::new(250.0, 215.0),
        );
        assert_eq!(
            app.session.doc.get(b).unwrap().parent(),
            Some(root),
            "the control: dragged clear of the frame, it leaves"
        );
    }

    /// A rotated container answers for its turned box: (138, 1) is inside the upright
    /// bounds but outside the box, so not in the container.
    /// flip: `pointer_in` asks the upright world bounds → fails at "the corner outside the turned box is not in the container".
    #[test]
    fn a_rotated_container_answers_for_its_turned_box() {
        let ctx = fresh();
        let mut ids = IdSource::new(0xBC);
        let (root, row, a) = (ids.mint(), ids.mint(), ids.mint());
        let turned = Affine::rotate(std::f64::consts::FRAC_PI_4);
        let build = |ctx: &egui::Context| {
            canvas_over(
                ctx,
                root,
                vec![
                    frame_op(row, root, 0, 200.0, 100.0, turned),
                    rect_op(a, row, 0, 20.0, 20.0),
                    Operation::SetDisplay {
                        id: row,
                        display: Some(Display::Flex(Flex {
                            padding: [10.0; 4],
                            align_items: AlignItems::Start,
                            ..Default::default()
                        })),
                    },
                ],
                Point::new(0.0, 100.0),
            )
        };
        let press = turned * Point::new(20.0, 20.0);
        let inside = turned * Point::new(100.0, 50.0);
        let corner = Point::new(138.0, 1.0);
        assert!(
            !ondin_core::kurbo::Rect::new(0.0, 0.0, 200.0, 100.0)
                .contains(turned.inverse() * corner),
            "the fixture: the corner is outside the turned box"
        );
        let mut app = build(&ctx);
        app.session.selection.set(vec![a]);
        drag(&ctx, &mut app, press, inside);
        assert_eq!(app.session.doc.get(a).unwrap().parent(), Some(row), "kept");

        // Asked directly: move snapping adjusts a drag's delta, so a drag cannot hit an exact point.
        assert!(
            !app.pointer_in(row, corner),
            "the corner outside the turned box is not in the container"
        );
        assert!(app.pointer_in(row, inside), "the inside is");

        // A 20 × 10 box at (120, 0) is inside the upright bounds but past the turned top edge, y = x.
        // flip: `box_meets` answers from the upright bounds → fails at "a box past the turned edge does not meet it".
        let past = ondin_core::kurbo::Rect::new(120.0, 0.0, 140.0, 10.0);
        assert!(
            app.session
                .resolved
                .world_bounds(row)
                .unwrap()
                .overlaps(past),
            "the fixture: inside the upright bounds"
        );
        assert!(
            !app.box_meets(row, past),
            "a box past the turned edge does not meet it"
        );
        assert!(
            app.box_meets(row, ondin_core::kurbo::Rect::new(60.0, 80.0, 70.0, 90.0)),
            "a box inside it does"
        );
        assert!(
            app.box_meets(
                row,
                ondin_core::kurbo::Rect::new(-200.0, -200.0, 400.0, 400.0)
            ),
            "and one around it all"
        );
    }

    /// A 200 × 100 row at the origin holding two 50 × 30 rects, `a` then `b`; `a` selected.
    fn two_in_a_row(ctx: &egui::Context, seed: u64) -> (OndinApp, NodeId, NodeId, NodeId, NodeId) {
        let mut ids = IdSource::new(seed);
        let (root, row, a, b) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let mut app = canvas_over(
            ctx,
            root,
            vec![
                frame_op(row, root, 0, 200.0, 100.0, Affine::IDENTITY),
                rect_op(a, row, 0, 50.0, 30.0),
                rect_op(b, row, 1, 50.0, 30.0),
                Operation::SetDisplay {
                    id: row,
                    display: Some(Display::Flex(Flex {
                        align_items: AlignItems::Start,
                        ..Default::default()
                    })),
                },
            ],
            Point::new(100.0, 100.0),
        );
        app.session.selection.set(vec![a]);
        (app, root, row, a, b)
    }

    /// A lone in-flow item pressed inside its container leaves it when the pointer does,
    /// and stays while the pointer is in.
    /// flip: `pointer_in` answers `true` always → fails at "dragged clear, it leaves".
    #[test]
    fn a_lone_item_pressed_inside_leaves_when_the_pointer_does() {
        let ctx = fresh();
        let (mut app, _root, row, a, b) = two_in_a_row(&ctx, 0xBD);
        drag(
            &ctx,
            &mut app,
            Point::new(25.0, 15.0),
            Point::new(25.0, 60.0),
        );
        assert_eq!(
            app.session.doc.get(row).unwrap().children(),
            &[a, b],
            "pointer inside: it stays, first"
        );

        let ctx = fresh();
        let (mut app, root, _row, a, _b) = two_in_a_row(&ctx, 0xBD);
        drag(
            &ctx,
            &mut app,
            Point::new(25.0, 15.0),
            Point::new(100.0, 250.0),
        );
        assert_eq!(
            app.session.doc.get(a).unwrap().parent(),
            Some(root),
            "dragged clear, it leaves"
        );
    }

    /// An Alt-drag of an in-flow item leaves the original in its slot, the copy where a move would go.
    /// flip: `clone_tx` places every copy in its source's parent → fails at "the original keeps its slot".
    #[test]
    fn an_alt_drag_copies_an_in_flow_item_where_a_move_would_put_it() {
        let ctx = fresh();
        let (mut app, root, row, a, b) = two_in_a_row(&ctx, 0xBE);
        alt_drag(
            &ctx,
            &mut app,
            Point::new(25.0, 15.0),
            Point::new(25.0, 60.0),
        );
        let children = app.session.doc.get(row).unwrap().children().to_vec();
        assert_eq!(children.len(), 3, "a copy in the row: {children:?}");
        assert_eq!(
            (children[0], children[2]),
            (a, b),
            "the copy just after a, a in its slot"
        );
        assert_eq!(
            app.session.doc.get(root).unwrap().children(),
            &[row],
            "nothing on the root"
        );

        let ctx = fresh();
        let (mut app, root, row, a, b) = two_in_a_row(&ctx, 0xBE);
        alt_drag(
            &ctx,
            &mut app,
            Point::new(25.0, 15.0),
            Point::new(100.0, 250.0),
        );
        assert_eq!(
            app.session.doc.get(row).unwrap().children(),
            &[a, b],
            "the original keeps its slot"
        );
        assert_eq!(
            app.session.doc.get(root).unwrap().children().len(),
            2,
            "the copy dragged clear lands on the root"
        );
    }

    /// An Alt-drag's copy kept in a grid is auto-placed into the next cell on release
    /// (drawn from x 100, not where it was dropped), since a ghost is not in the layout's tree.
    /// flip: force `clone_tx`'s insert index to 0 → fails at "a first".
    #[test]
    fn an_alt_drag_copy_kept_in_a_grid_is_laid_into_a_cell_on_release() {
        let ctx = fresh();
        let mut ids = IdSource::new(0xBF);
        let (root, grid, a) = (ids.mint(), ids.mint(), ids.mint());
        let mut app = canvas_over(
            &ctx,
            root,
            vec![
                frame_op(grid, root, 0, 300.0, 100.0, Affine::IDENTITY),
                rect_op(a, grid, 0, 40.0, 40.0),
                Operation::SetDisplay {
                    id: grid,
                    display: Some(Display::Grid(ondin_core::container::Grid {
                        columns: ondin_core::container::parse_tracks("100px 100px 100px").unwrap(),
                        ..Default::default()
                    })),
                },
            ],
            Point::new(150.0, 50.0),
        );
        app.session.selection.set(vec![a]);
        alt_drag(
            &ctx,
            &mut app,
            Point::new(20.0, 20.0),
            Point::new(40.0, 30.0),
        );
        let children = app.session.doc.get(grid).unwrap().children().to_vec();
        assert_eq!(children.len(), 2, "the copy in the grid: {children:?}");
        assert_eq!(children[0], a, "a first");
        let copy = children[1];
        let at = app.session.resolved.world_bounds(copy).unwrap();
        assert_eq!(
            (at.x0, at.y0),
            (100.0, 0.0),
            "auto-placed after a, in the second cell"
        );
        assert_eq!(
            app.session.resolved.world_bounds(a).unwrap().x0,
            0.0,
            "a where it was"
        );
    }
}

#[cfg(test)]
mod string_binding_session_tests {
    //! A text bound to a string variable, edited on the canvas: the session opens on the
    //! string drawn, closing it unedited keeps the binding, typing detaches it.

    use super::*;
    use ondin_core::{Cell, Mode, VarKind, Variable};

    /// A text holding *Hello*, its content bound to `copy/cta` (*Sign in*).
    fn app_with_bound_text(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (text, mode, var) = (ids.mint(), ids.mint_mode(), ids.mint_var());
        let parts = crate::fonts::FontService::default_text_parts();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: text,
                parent: root,
                index: 0,
                kind: NodeKind::Text {
                    content: "Hello".into(),
                    style: Box::new(parts.style),
                    spans: parts.spans,
                    para_spans: parts.para_spans,
                    paragraph: parts.paragraph,
                    block: parts.block,
                    sizing: parts.sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: Some(Affine::translate((400.0, 400.0))),
                name: None,
            },
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Mode 1".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "copy/cta".into(),
                    kind: VarKind::String,
                    cells: [(mode, Cell::String("Sign in".into()))].into(),
                    origin: None,
                    forked: None,
                },
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(text);
        app.canvas_px = (800, 600);
        let key = crate::panels::binding::FieldKey::Content { id: text };
        app.bind_field(key, var);
        assert_eq!(app.field_var(key), Some(var), "bound through the app");
        (app, text)
    }

    /// The session opens on the string drawn and leaves the binding alone if nothing is
    /// typed; typing detaches it to what was typed.
    /// flip: `begin_edit_text` reads the stored kind (`doc.get(id)`) → fails at "the editor holds the bound string".
    #[test]
    fn a_session_on_a_bound_text_edits_what_it_draws() {
        let ctx = egui::Context::default();
        let (mut app, text) = app_with_bound_text(&ctx);
        app.begin_edit_text(Some(text), None);
        assert_eq!(
            app.text.as_ref().map(|s| s.editor.content().to_string()),
            Some("Sign in".to_string()),
            "the editor holds the bound string"
        );
        app.finish_text_edit();
        assert!(
            app.session.doc.get(text).unwrap().text_vars()[0].is_some(),
            "closed unedited, still bound"
        );

        app.begin_edit_text(Some(text), None);
        if let Some(s) = app.text.as_mut() {
            s.editor.select_all();
            s.editor.insert("Log in");
        }
        app.finish_text_edit();
        let n = app.session.doc.get(text).unwrap();
        let NodeKind::Text { content, .. } = n.kind() else {
            unreachable!()
        };
        assert_eq!(
            (content.as_str(), n.text_vars()[0]),
            ("Log in", None),
            "typing detaches the content to what was typed"
        );
    }
}

#[cfg(test)]
mod bound_resize_tests {
    //! A resize handle dragged over a bound W detaches it and says so: a real drag through
    //! `canvas_ui`, `keep_size_bindings` cutting the binding, the floating status naming the variable.

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Cell, Document, IdSource, Mode, Operation, Transaction, VarKind, Variable};

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };
    const PPP: f32 = 1.0;

    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let time = ctx.input(|i| i.time) + 0.1;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| app.canvas_ui(ui),
        );
    }

    fn button(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// A 100 × 60 rect with W bound to `size/w` (100), dragged by its right handle, detaches W and says so.
    /// flip: `variables_detached` reads from propagate's mark to the end → fails at "and said so".
    #[test]
    fn dragging_a_bound_width_detaches_it_and_says_so() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0x2E8A);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (id, mode, var) = (ids.mint(), ids.mint_mode(), ids.mint_var());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 60.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Mode 1".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "size/w".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(100.0))].into(),
                    origin: None,
                    forked: None,
                },
            },
        ]))
        .expect("a rect and a number variable");
        let bind = ondin_core::build::bind_size_ops(&doc, id, [Some(var), None]);
        doc.apply(&Transaction(bind)).expect("W bound");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.session.camera.center = Point::new(50.0, 30.0);
        app.session.camera.zoom = 1.0;
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new());
        }
        assert_eq!(app.session.doc.get(id).unwrap().size_vars()[0], Some(var));

        let at = |app: &OndinApp, w: Point| app.to_screen(w, RECT, PPP);
        let (from, to) = (Point::new(100.0, 30.0), Point::new(140.0, 30.0));
        let start = at(&app, from);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(start)]);
        frame(&ctx, &mut app, vec![button(start, true)]);
        for step in 1..=8 {
            let p = at(&app, from + (to - from) * (step as f64 / 8.0));
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        }
        let end = at(&app, to);
        frame(&ctx, &mut app, vec![button(end, false)]);
        frame(&ctx, &mut app, Vec::new());

        let doc = &app.session.doc;
        let w = match doc.get(id).unwrap().kind() {
            NodeKind::Rect { size, .. } => size.width,
            _ => unreachable!("a rect"),
        };
        assert!((w - 140.0).abs() < 1e-6, "resized to 140, got {w}");
        assert_eq!(doc.get(id).unwrap().size_vars()[0], None, "W detached");
        assert_eq!(app.session.status().text, "Detached size/w", "and said so");
    }
}
